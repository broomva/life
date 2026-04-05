use spacetimedb::{reducer, Identity, ReducerContext, Table};

use crate::tables::{
    direct_conversation, direct_message, user_profile, DirectConversation, DirectMessage,
};
use crate::validation;

/// Normalize two identities so the lexicographically smaller hex comes first.
/// This ensures a unique conversation per pair regardless of who initiates.
fn normalize_participants(a: Identity, b: Identity) -> (Identity, Identity) {
    if a.to_hex().to_string() <= b.to_hex().to_string() {
        (a, b)
    } else {
        (b, a)
    }
}

/// Find an existing conversation between two participants.
fn find_conversation(ctx: &ReducerContext, a: Identity, b: Identity) -> Option<DirectConversation> {
    let (norm_a, norm_b) = normalize_participants(a, b);
    ctx.db
        .direct_conversation()
        .iter()
        .find(|c| c.participant_a == norm_a && c.participant_b == norm_b)
}

#[reducer]
pub fn send_direct_message(
    ctx: &ReducerContext,
    recipient: Identity,
    content: String,
) -> Result<(), String> {
    let sender = ctx.sender();

    validation::validate_dm_not_self(
        &sender.to_hex().to_string(),
        &recipient.to_hex().to_string(),
    )?;
    validation::validate_message_content(&content)?;

    // Verify both users exist
    ctx.db
        .user_profile()
        .identity()
        .find(sender)
        .ok_or("Sender profile not found. Connect first.")?;
    ctx.db
        .user_profile()
        .identity()
        .find(recipient)
        .ok_or("Recipient not found")?;

    // Find or create conversation
    let conversation = if let Some(conv) = find_conversation(ctx, sender, recipient) {
        // Update last_message_at
        ctx.db
            .direct_conversation()
            .id()
            .update(DirectConversation {
                last_message_at: ctx.timestamp,
                ..conv
            })
    } else {
        let (norm_a, norm_b) = normalize_participants(sender, recipient);
        ctx.db.direct_conversation().insert(DirectConversation {
            id: 0,
            participant_a: norm_a,
            participant_b: norm_b,
            created_at: ctx.timestamp,
            last_message_at: ctx.timestamp,
        })
    };

    ctx.db.direct_message().insert(DirectMessage {
        id: 0,
        conversation_id: conversation.id,
        sender,
        content,
        created_at: ctx.timestamp,
        edited_at: None,
        read: false,
    });

    Ok(())
}

#[reducer]
pub fn edit_direct_message(
    ctx: &ReducerContext,
    message_id: u64,
    new_content: String,
) -> Result<(), String> {
    validation::validate_message_content(&new_content)?;

    let msg = ctx
        .db
        .direct_message()
        .id()
        .find(message_id)
        .ok_or("Direct message not found")?;

    validation::guard_message_owner(msg.sender == ctx.sender())?;

    ctx.db.direct_message().id().update(DirectMessage {
        content: new_content,
        edited_at: Some(ctx.timestamp),
        ..msg
    });

    Ok(())
}

#[reducer]
pub fn delete_direct_message(ctx: &ReducerContext, message_id: u64) -> Result<(), String> {
    let msg = ctx
        .db
        .direct_message()
        .id()
        .find(message_id)
        .ok_or("Direct message not found")?;

    validation::guard_message_owner(msg.sender == ctx.sender())?;

    ctx.db.direct_message().id().delete(message_id);
    Ok(())
}

#[reducer]
pub fn mark_dm_read(ctx: &ReducerContext, conversation_id: u64) -> Result<(), String> {
    let conversation = ctx
        .db
        .direct_conversation()
        .id()
        .find(conversation_id)
        .ok_or("Conversation not found")?;

    // Verify the caller is a participant
    let sender = ctx.sender();
    if conversation.participant_a != sender && conversation.participant_b != sender {
        return Err("You are not a participant in this conversation".to_string());
    }

    // Mark all unread messages from the other participant as read
    let messages: Vec<_> = ctx
        .db
        .direct_message()
        .conversation_id()
        .filter(&conversation_id)
        .filter(|m| m.sender != sender && !m.read)
        .collect();

    for msg in messages {
        ctx.db
            .direct_message()
            .id()
            .update(DirectMessage { read: true, ..msg });
    }

    Ok(())
}
