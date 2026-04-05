use spacetimedb::{reducer, ReducerContext, Table};

use crate::auth;
use crate::tables::{channel, message, reaction, thread, Channel};
use crate::types::{ChannelType, MemberRole};
use crate::validation;

#[reducer]
pub fn create_channel(
    ctx: &ReducerContext,
    server_id: u64,
    name: String,
    channel_type: ChannelType,
) -> Result<(), String> {
    validation::validate_name(&name, "Channel")?;

    auth::require_role(ctx, server_id, MemberRole::Admin)?;

    // Determine next position
    let max_pos = ctx
        .db
        .channel()
        .server_id()
        .filter(&server_id)
        .map(|c| c.position)
        .max()
        .unwrap_or(0);

    ctx.db.channel().insert(Channel {
        id: 0,
        server_id,
        name,
        topic: None,
        channel_type,
        position: max_pos + 1,
        created_at: ctx.timestamp,
    });

    Ok(())
}

#[reducer]
pub fn update_channel(
    ctx: &ReducerContext,
    channel_id: u64,
    name: Option<String>,
    topic: Option<String>,
) -> Result<(), String> {
    let (channel, _server) = auth::get_server_for_channel(ctx, channel_id)?;
    auth::require_role(ctx, channel.server_id, MemberRole::Admin)?;

    let new_name = name.unwrap_or(channel.name.clone());
    validation::validate_name(&new_name, "Channel")?;

    ctx.db.channel().id().update(Channel {
        name: new_name,
        topic: topic.or(channel.topic.clone()),
        ..channel
    });

    Ok(())
}

#[reducer]
pub fn delete_channel(ctx: &ReducerContext, channel_id: u64) -> Result<(), String> {
    let (channel, _server) = auth::get_server_for_channel(ctx, channel_id)?;
    auth::require_role(ctx, channel.server_id, MemberRole::Admin)?;

    // Cascade delete messages and reactions
    let messages: Vec<_> = ctx.db.message().channel_id().filter(&channel_id).collect();
    for msg in &messages {
        let reactions: Vec<_> = ctx.db.reaction().message_id().filter(&msg.id).collect();
        for r in &reactions {
            ctx.db.reaction().id().delete(r.id);
        }
        ctx.db.message().id().delete(msg.id);
    }

    // Delete threads
    let threads: Vec<_> = ctx.db.thread().channel_id().filter(&channel_id).collect();
    for t in &threads {
        ctx.db.thread().id().delete(t.id);
    }

    ctx.db.channel().id().delete(channel_id);
    Ok(())
}
