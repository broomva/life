// WASM-dependent modules are gated behind the "module" feature so that
// `cargo test --no-default-features` can compile and test pure logic natively.
#[cfg(feature = "module")]
pub mod auth;
#[cfg(feature = "module")]
pub mod reducers;
#[cfg(feature = "module")]
pub mod tables;
#[cfg(feature = "module")]
pub mod types;
pub mod validation;

#[cfg(feature = "module")]
mod lifecycle {
    use spacetimedb::{ReducerContext, Table};

    use crate::tables::{
        channel, server, server_member, user_presence, user_profile, Channel, Server, ServerMember,
        UserPresence, UserProfile,
    };
    use crate::types::{ChannelType, MemberRole};

    #[spacetimedb::reducer(init)]
    pub fn init(ctx: &ReducerContext) {
        // Create the default "Spaces Hub" server
        let hub = ctx.db.server().insert(Server {
            id: 0,
            name: "Spaces Hub".to_string(),
            description: Some("Default server for the Spaces network".to_string()),
            icon_url: None,
            owner: ctx.sender(),
            created_at: ctx.timestamp,
        });

        // Create default channels
        ctx.db.channel().insert(Channel {
            id: 0,
            server_id: hub.id,
            name: "general".to_string(),
            topic: Some("General discussion".to_string()),
            channel_type: ChannelType::Text,
            position: 0,
            created_at: ctx.timestamp,
        });

        ctx.db.channel().insert(Channel {
            id: 0,
            server_id: hub.id,
            name: "agent-logs".to_string(),
            topic: Some("Agent activity logs".to_string()),
            channel_type: ChannelType::AgentLog,
            position: 1,
            created_at: ctx.timestamp,
        });

        ctx.db.channel().insert(Channel {
            id: 0,
            server_id: hub.id,
            name: "system".to_string(),
            topic: Some("System notifications".to_string()),
            channel_type: ChannelType::Announcement,
            position: 2,
            created_at: ctx.timestamp,
        });

        log::info!("Spaces Hub initialized with id={}", hub.id);
    }

    #[spacetimedb::reducer(client_connected)]
    pub fn client_connected(ctx: &ReducerContext) {
        let sender = ctx.sender();

        // Upsert UserPresence
        if let Some(presence) = ctx.db.user_presence().identity().find(sender) {
            ctx.db.user_presence().identity().update(UserPresence {
                online: true,
                last_seen: ctx.timestamp,
                ..presence
            });
        } else {
            ctx.db.user_presence().insert(UserPresence {
                identity: sender,
                online: true,
                status_text: None,
                last_seen: ctx.timestamp,
            });
        }

        // Create UserProfile if new
        if ctx.db.user_profile().identity().find(sender).is_none() {
            let short_id = sender.to_hex().to_string();
            let username = format!("user_{}", &short_id[..8.min(short_id.len())]);

            ctx.db.user_profile().insert(UserProfile {
                identity: sender,
                username,
                display_name: None,
                avatar_url: None,
                bio: None,
                is_agent: false,
                created_at: ctx.timestamp,
            });

            // Auto-join the Spaces Hub (first server, id=1 typically)
            // Find the Spaces Hub by name
            if let Some(hub) = ctx.db.server().iter().find(|s| s.name == "Spaces Hub") {
                // Only join if not already a member
                let already_member = ctx
                    .db
                    .server_member()
                    .server_id()
                    .filter(&hub.id)
                    .any(|m| m.identity == sender);
                if !already_member {
                    ctx.db.server_member().insert(ServerMember {
                        id: 0,
                        server_id: hub.id,
                        identity: sender,
                        role: MemberRole::Member,
                        joined_at: ctx.timestamp,
                    });
                }
            }
        }

        log::info!("Client connected: {:?}", sender);
    }

    #[spacetimedb::reducer(client_disconnected)]
    pub fn client_disconnected(ctx: &ReducerContext) {
        if let Some(presence) = ctx.db.user_presence().identity().find(ctx.sender()) {
            ctx.db.user_presence().identity().update(UserPresence {
                online: false,
                last_seen: ctx.timestamp,
                ..presence
            });
        }
        log::info!("Client disconnected: {:?}", ctx.sender());
    }
}
