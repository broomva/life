use spacetimedb::{reducer, ReducerContext, Table};

use crate::auth;
use crate::tables::{
    channel_read_state, message, reaction, thread, typing_indicator, ChannelReadState, Message,
    Reaction, Thread, TypingIndicator,
};
use crate::types::{MemberRole, MessageType};
use crate::validation;

#[reducer]
pub fn send_message(
    ctx: &ReducerContext,
    channel_id: u64,
    content: String,
    thread_id: Option<u64>,
    reply_to_id: Option<u64>,
) -> Result<(), String> {
    validation::validate_message_content(&content)?;

    let (channel, _server) = auth::get_server_for_channel(ctx, channel_id)?;
    auth::require_member(ctx, channel.server_id)?;

    // Validate thread exists if specified
    if let Some(tid) = thread_id {
        let thread = ctx.db.thread().id().find(tid).ok_or("Thread not found")?;
        validation::guard_thread_not_archived(thread.archived)?;
        validation::guard_thread_channel(thread.channel_id, channel_id)?;
        // Update thread's last_message_at
        ctx.db.thread().id().update(Thread {
            last_message_at: ctx.timestamp,
            ..thread
        });
    }

    // Validate reply target exists if specified
    if let Some(rid) = reply_to_id {
        ctx.db
            .message()
            .id()
            .find(rid)
            .ok_or("Reply target message not found")?;
    }

    ctx.db.message().insert(Message {
        id: 0,
        channel_id,
        thread_id,
        sender: ctx.sender(),
        content,
        message_type: MessageType::Text,
        created_at: ctx.timestamp,
        edited_at: None,
        reply_to_id,
    });

    Ok(())
}

#[reducer]
pub fn edit_message(
    ctx: &ReducerContext,
    message_id: u64,
    new_content: String,
) -> Result<(), String> {
    validation::validate_message_content(&new_content)?;

    let msg = ctx
        .db
        .message()
        .id()
        .find(message_id)
        .ok_or("Message not found")?;

    validation::guard_message_owner(msg.sender == ctx.sender())?;

    ctx.db.message().id().update(Message {
        content: new_content,
        edited_at: Some(ctx.timestamp),
        ..msg
    });

    Ok(())
}

#[reducer]
pub fn delete_message(ctx: &ReducerContext, message_id: u64) -> Result<(), String> {
    let msg = ctx
        .db
        .message()
        .id()
        .find(message_id)
        .ok_or("Message not found")?;

    let is_sender = msg.sender == ctx.sender();

    // Check if admin+ in the server
    let (_channel, _server) = auth::get_server_for_channel(ctx, msg.channel_id)?;
    let is_admin = auth::get_member_role(ctx, _channel.server_id, ctx.sender())
        .map(|r| r.level() >= MemberRole::Admin.level())
        .unwrap_or(false);

    validation::guard_delete_permission(is_sender, is_admin)?;

    // Delete reactions first
    let reactions: Vec<_> = ctx.db.reaction().message_id().filter(&message_id).collect();
    for r in &reactions {
        ctx.db.reaction().id().delete(r.id);
    }

    ctx.db.message().id().delete(message_id);
    Ok(())
}

#[reducer]
pub fn add_reaction(ctx: &ReducerContext, message_id: u64, emoji: String) -> Result<(), String> {
    validation::validate_emoji(&emoji)?;

    let msg = ctx
        .db
        .message()
        .id()
        .find(message_id)
        .ok_or("Message not found")?;

    // Verify membership
    let (channel, _server) = auth::get_server_for_channel(ctx, msg.channel_id)?;
    auth::require_member(ctx, channel.server_id)?;

    // Check for duplicate reaction
    let already = ctx
        .db
        .reaction()
        .message_id()
        .filter(&message_id)
        .any(|r| r.identity == ctx.sender() && r.emoji == emoji);
    if already {
        return Err("You already reacted with this emoji".to_string());
    }

    ctx.db.reaction().insert(Reaction {
        id: 0,
        message_id,
        identity: ctx.sender(),
        emoji,
    });

    Ok(())
}

#[reducer]
pub fn remove_reaction(ctx: &ReducerContext, message_id: u64, emoji: String) -> Result<(), String> {
    let reaction = ctx
        .db
        .reaction()
        .message_id()
        .filter(&message_id)
        .find(|r| r.identity == ctx.sender() && r.emoji == emoji)
        .ok_or("Reaction not found")?;

    ctx.db.reaction().id().delete(reaction.id);
    Ok(())
}

#[reducer]
pub fn start_typing(ctx: &ReducerContext, channel_id: u64) -> Result<(), String> {
    let (channel, _server) = auth::get_server_for_channel(ctx, channel_id)?;
    auth::require_member(ctx, channel.server_id)?;

    ctx.db.typing_indicator().insert(TypingIndicator {
        id: 0,
        channel_id,
        identity: ctx.sender(),
    });

    Ok(())
}

#[reducer]
pub fn mark_read(ctx: &ReducerContext, channel_id: u64, message_id: u64) -> Result<(), String> {
    // Find existing read state for this user+channel
    let existing = ctx
        .db
        .channel_read_state()
        .identity()
        .filter(&ctx.sender())
        .find(|rs| rs.channel_id == channel_id);

    if let Some(rs) = existing {
        ctx.db.channel_read_state().id().update(ChannelReadState {
            last_read_msg_id: message_id,
            ..rs
        });
    } else {
        ctx.db.channel_read_state().insert(ChannelReadState {
            id: 0,
            identity: ctx.sender(),
            channel_id,
            last_read_msg_id: message_id,
        });
    }

    Ok(())
}
