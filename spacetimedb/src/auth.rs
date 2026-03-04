use spacetimedb::{Identity, ReducerContext};

use crate::tables::{channel, server, server_member, Channel, Server, ServerMember};
use crate::types::MemberRole;

/// Get a user's membership in a server, if any.
pub fn get_membership(
    ctx: &ReducerContext,
    server_id: u64,
    identity: Identity,
) -> Option<ServerMember> {
    ctx.db
        .server_member()
        .server_id()
        .filter(&server_id)
        .find(|m| m.identity == identity)
}

/// Get the role of a user in a server.
pub fn get_member_role(
    ctx: &ReducerContext,
    server_id: u64,
    identity: Identity,
) -> Option<MemberRole> {
    get_membership(ctx, server_id, identity).map(|m| m.role)
}

/// Require that the caller is a member of the server. Returns the membership row.
pub fn require_member(ctx: &ReducerContext, server_id: u64) -> Result<ServerMember, String> {
    get_membership(ctx, server_id, ctx.sender())
        .ok_or_else(|| "You are not a member of this server".to_string())
}

/// Require that the caller has at least the given role level.
pub fn require_role(
    ctx: &ReducerContext,
    server_id: u64,
    min_role: MemberRole,
) -> Result<ServerMember, String> {
    let member = require_member(ctx, server_id)?;
    if member.role.level() >= min_role.level() {
        Ok(member)
    } else {
        Err(format!(
            "Insufficient permissions: requires {:?} or higher",
            min_role
        ))
    }
}

/// Look up a channel and its parent server. Returns (Channel, Server).
pub fn get_server_for_channel(
    ctx: &ReducerContext,
    channel_id: u64,
) -> Result<(Channel, Server), String> {
    let channel = ctx
        .db
        .channel()
        .id()
        .find(channel_id)
        .ok_or("Channel not found")?;
    let server = ctx
        .db
        .server()
        .id()
        .find(channel.server_id)
        .ok_or("Server not found for channel")?;
    Ok((channel, server))
}
