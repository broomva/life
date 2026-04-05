use spacetimedb::{reducer, ReducerContext, Table};

use crate::auth;
use crate::tables::{
    channel, message, reaction, server, server_member, thread, user_profile, Channel, Message,
    Server, ServerMember,
};
use crate::types::{ChannelType, MemberRole, MessageType};
use crate::validation;

#[reducer]
pub fn create_server(
    ctx: &ReducerContext,
    name: String,
    description: Option<String>,
) -> Result<(), String> {
    validation::validate_name(&name, "Server")?;

    // Ensure user has a profile
    ctx.db
        .user_profile()
        .identity()
        .find(ctx.sender())
        .ok_or("Must be connected to create a server")?;

    let server = ctx.db.server().insert(Server {
        id: 0,
        name,
        description,
        icon_url: None,
        owner: ctx.sender(),
        created_at: ctx.timestamp,
    });

    // Add owner as member
    ctx.db.server_member().insert(ServerMember {
        id: 0,
        server_id: server.id,
        identity: ctx.sender(),
        role: MemberRole::Owner,
        joined_at: ctx.timestamp,
    });

    // Create default #general channel
    ctx.db.channel().insert(Channel {
        id: 0,
        server_id: server.id,
        name: "general".to_string(),
        topic: Some("General discussion".to_string()),
        channel_type: ChannelType::Text,
        position: 0,
        created_at: ctx.timestamp,
    });

    log::info!(
        "Server '{}' (id={}) created by {:?}",
        server.name,
        server.id,
        ctx.sender()
    );
    Ok(())
}

#[reducer]
pub fn update_server(
    ctx: &ReducerContext,
    server_id: u64,
    name: Option<String>,
    description: Option<String>,
) -> Result<(), String> {
    auth::require_role(ctx, server_id, MemberRole::Admin)?;

    let server = ctx
        .db
        .server()
        .id()
        .find(server_id)
        .ok_or("Server not found")?;

    let new_name = name.unwrap_or(server.name.clone());
    validation::validate_name(&new_name, "Server")?;

    ctx.db.server().id().update(Server {
        name: new_name,
        description: description.or(server.description.clone()),
        ..server
    });

    Ok(())
}

#[reducer]
pub fn delete_server(ctx: &ReducerContext, server_id: u64) -> Result<(), String> {
    let server = ctx
        .db
        .server()
        .id()
        .find(server_id)
        .ok_or("Server not found")?;

    validation::guard_only_owner_deletes(server.owner == ctx.sender())?;

    // Cascade delete: messages, channels, members
    let channels: Vec<_> = ctx.db.channel().server_id().filter(&server_id).collect();
    for ch in &channels {
        let messages: Vec<_> = ctx.db.message().channel_id().filter(&ch.id).collect();
        for msg in &messages {
            // Delete reactions for this message
            let reactions: Vec<_> = ctx.db.reaction().message_id().filter(&msg.id).collect();
            for r in &reactions {
                ctx.db.reaction().id().delete(r.id);
            }
            ctx.db.message().id().delete(msg.id);
        }
        // Delete threads
        let threads: Vec<_> = ctx.db.thread().channel_id().filter(&ch.id).collect();
        for t in &threads {
            ctx.db.thread().id().delete(t.id);
        }
        ctx.db.channel().id().delete(ch.id);
    }

    // Delete members
    let members: Vec<_> = ctx
        .db
        .server_member()
        .server_id()
        .filter(&server_id)
        .collect();
    for m in &members {
        ctx.db.server_member().id().delete(m.id);
    }

    ctx.db.server().id().delete(server_id);
    log::info!("Server {} deleted by {:?}", server_id, ctx.sender());
    Ok(())
}

#[reducer]
pub fn join_server(ctx: &ReducerContext, server_id: u64) -> Result<(), String> {
    ctx.db
        .server()
        .id()
        .find(server_id)
        .ok_or("Server not found")?;

    ctx.db
        .user_profile()
        .identity()
        .find(ctx.sender())
        .ok_or("Must be connected to join a server")?;

    // Check not already a member
    if auth::get_membership(ctx, server_id, ctx.sender()).is_some() {
        return Err("Already a member of this server".to_string());
    }

    let is_agent = ctx
        .db
        .user_profile()
        .identity()
        .find(ctx.sender())
        .map(|p| p.is_agent)
        .unwrap_or(false);

    let role = if is_agent {
        MemberRole::Agent
    } else {
        MemberRole::Member
    };

    ctx.db.server_member().insert(ServerMember {
        id: 0,
        server_id,
        identity: ctx.sender(),
        role,
        joined_at: ctx.timestamp,
    });

    // Post a system join message in general channel
    if let Some(general) = ctx
        .db
        .channel()
        .server_id()
        .filter(&server_id)
        .find(|c| c.name == "general")
    {
        let profile = ctx.db.user_profile().identity().find(ctx.sender());
        let name = profile
            .map(|p| p.username)
            .unwrap_or_else(|| "Unknown".to_string());
        ctx.db.message().insert(Message {
            id: 0,
            channel_id: general.id,
            thread_id: None,
            sender: ctx.sender(),
            content: format!("{} joined the server", name),
            message_type: MessageType::Join,
            created_at: ctx.timestamp,
            edited_at: None,
            reply_to_id: None,
        });
    }

    Ok(())
}

#[reducer]
pub fn leave_server(ctx: &ReducerContext, server_id: u64) -> Result<(), String> {
    let server = ctx
        .db
        .server()
        .id()
        .find(server_id)
        .ok_or("Server not found")?;

    validation::guard_owner_cannot_leave(server.owner == ctx.sender())?;

    let member = auth::require_member(ctx, server_id)?;
    ctx.db.server_member().id().delete(member.id);

    // Post a system leave message
    if let Some(general) = ctx
        .db
        .channel()
        .server_id()
        .filter(&server_id)
        .find(|c| c.name == "general")
    {
        let profile = ctx.db.user_profile().identity().find(ctx.sender());
        let name = profile
            .map(|p| p.username)
            .unwrap_or_else(|| "Unknown".to_string());
        ctx.db.message().insert(Message {
            id: 0,
            channel_id: general.id,
            thread_id: None,
            sender: ctx.sender(),
            content: format!("{} left the server", name),
            message_type: MessageType::Leave,
            created_at: ctx.timestamp,
            edited_at: None,
            reply_to_id: None,
        });
    }

    Ok(())
}

#[reducer]
pub fn update_member_role(
    ctx: &ReducerContext,
    server_id: u64,
    target_identity: spacetimedb::Identity,
    new_role: MemberRole,
) -> Result<(), String> {
    let actor = auth::require_member(ctx, server_id)?;
    let target = auth::get_membership(ctx, server_id, target_identity)
        .ok_or("Target is not a member of this server")?;

    validation::can_modify_role(
        actor.role.level(),
        target.role.level(),
        new_role.level(),
        target.role == MemberRole::Owner,
        new_role == MemberRole::Owner,
    )?;

    ctx.db.server_member().id().update(ServerMember {
        role: new_role,
        ..target
    });

    Ok(())
}
