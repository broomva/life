use spacetimedb::{Identity, Timestamp};

use crate::types::{ChannelType, MemberRole, MessageType};

// --- User tables ---

#[spacetimedb::table(accessor = user_profile, public)]
pub struct UserProfile {
    #[primary_key]
    pub identity: Identity,
    #[unique]
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub bio: Option<String>,
    pub is_agent: bool,
    pub created_at: Timestamp,
}

#[spacetimedb::table(accessor = user_presence, public)]
pub struct UserPresence {
    #[primary_key]
    pub identity: Identity,
    pub online: bool,
    pub status_text: Option<String>,
    pub last_seen: Timestamp,
}

// --- Server tables ---

#[spacetimedb::table(accessor = server, public)]
pub struct Server {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub name: String,
    pub description: Option<String>,
    pub icon_url: Option<String>,
    pub owner: Identity,
    pub created_at: Timestamp,
}

#[spacetimedb::table(accessor = server_member, public)]
pub struct ServerMember {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[index(btree)]
    pub server_id: u64,
    pub identity: Identity,
    pub role: MemberRole,
    pub joined_at: Timestamp,
}

// --- Channel tables ---

#[spacetimedb::table(accessor = channel, public)]
pub struct Channel {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[index(btree)]
    pub server_id: u64,
    pub name: String,
    pub topic: Option<String>,
    pub channel_type: ChannelType,
    pub position: u32,
    pub created_at: Timestamp,
}

// --- Thread tables ---

#[spacetimedb::table(accessor = thread, public)]
pub struct Thread {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[index(btree)]
    pub channel_id: u64,
    pub name: String,
    pub creator: Identity,
    pub created_at: Timestamp,
    pub archived: bool,
    pub last_message_at: Timestamp,
}

// --- Message tables ---

#[spacetimedb::table(accessor = message, public)]
pub struct Message {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[index(btree)]
    pub channel_id: u64,
    pub thread_id: Option<u64>,
    pub sender: Identity,
    pub content: String,
    pub message_type: MessageType,
    pub created_at: Timestamp,
    pub edited_at: Option<Timestamp>,
    pub reply_to_id: Option<u64>,
}

#[spacetimedb::table(accessor = reaction, public)]
pub struct Reaction {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[index(btree)]
    pub message_id: u64,
    pub identity: Identity,
    pub emoji: String,
}

// --- Read state ---

#[spacetimedb::table(accessor = channel_read_state, public)]
pub struct ChannelReadState {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[index(btree)]
    pub identity: Identity,
    pub channel_id: u64,
    pub last_read_msg_id: u64,
}

// --- Direct messaging tables ---

/// A direct conversation between two participants.
/// Participants are normalized so participant_a < participant_b (by hex)
/// to prevent duplicate conversations.
#[spacetimedb::table(accessor = direct_conversation, public)]
pub struct DirectConversation {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub participant_a: Identity,
    pub participant_b: Identity,
    pub created_at: Timestamp,
    pub last_message_at: Timestamp,
}

#[spacetimedb::table(accessor = direct_message, public)]
pub struct DirectMessage {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    #[index(btree)]
    pub conversation_id: u64,
    pub sender: Identity,
    pub content: String,
    pub created_at: Timestamp,
    pub edited_at: Option<Timestamp>,
    pub read: bool,
}

// --- Event tables (transient) ---

#[spacetimedb::table(accessor = typing_indicator, public)]
pub struct TypingIndicator {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub channel_id: u64,
    pub identity: Identity,
}

#[spacetimedb::table(accessor = system_notification, public)]
pub struct SystemNotification {
    #[primary_key]
    #[auto_inc]
    pub id: u64,
    pub target: Identity,
    pub title: String,
    pub body: String,
}
