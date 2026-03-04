//! Pure validation functions extracted from reducers.
//!
//! These functions contain no SpacetimeDB imports and can be tested
//! with standard `cargo test`. Reducers call into this module for
//! all input validation, role hierarchy checks, and state guards.

// ---------------------------------------------------------------------------
// String validation
// ---------------------------------------------------------------------------

pub fn validate_username(s: &str) -> Result<(), String> {
    if s.is_empty() {
        return Err("Username cannot be empty".to_string());
    }
    if s.len() > 32 {
        return Err("Username must be 32 characters or fewer".to_string());
    }
    Ok(())
}

pub fn validate_name(s: &str, entity: &str) -> Result<(), String> {
    if s.is_empty() {
        return Err(format!("{} name cannot be empty", entity));
    }
    if s.len() > 100 {
        return Err(format!("{} name must be 100 characters or fewer", entity));
    }
    Ok(())
}

pub fn validate_message_content(s: &str) -> Result<(), String> {
    if s.is_empty() {
        return Err("Message content cannot be empty".to_string());
    }
    if s.len() > 4000 {
        return Err("Message must be 4000 characters or fewer".to_string());
    }
    Ok(())
}

pub fn validate_emoji(s: &str) -> Result<(), String> {
    if s.is_empty() {
        return Err("Emoji cannot be empty".to_string());
    }
    Ok(())
}

pub fn validate_agent_name(s: &str) -> Result<(), String> {
    if s.is_empty() {
        return Err("Agent name cannot be empty".to_string());
    }
    Ok(())
}

pub fn validate_thread_name(s: &str) -> Result<(), String> {
    if s.is_empty() {
        return Err("Thread name cannot be empty".to_string());
    }
    Ok(())
}

pub fn validate_dm_not_self(sender_hex: &str, recipient_hex: &str) -> Result<(), String> {
    if sender_hex == recipient_hex {
        Err("Cannot send a direct message to yourself".to_string())
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Role hierarchy
// ---------------------------------------------------------------------------

pub const ROLE_LEVEL_OWNER: u8 = 4;
pub const ROLE_LEVEL_ADMIN: u8 = 3;
pub const ROLE_LEVEL_MODERATOR: u8 = 2;
pub const ROLE_LEVEL_MEMBER: u8 = 1;
pub const ROLE_LEVEL_AGENT: u8 = 1;

/// Check whether `actor` can change `target`'s role to `new_role`.
///
/// Rules:
/// - Cannot change the owner's role
/// - Cannot assign the Owner role
/// - Actor must outrank the target's current role
/// - Actor must outrank the new role
pub fn can_modify_role(
    actor_level: u8,
    target_level: u8,
    new_role_level: u8,
    target_is_owner: bool,
    new_role_is_owner: bool,
) -> Result<(), String> {
    if target_is_owner {
        return Err("Cannot change the owner's role".to_string());
    }
    if new_role_is_owner {
        return Err("Cannot assign Owner role. Use transfer ownership instead.".to_string());
    }
    if actor_level <= target_level {
        return Err("Cannot modify role of someone with equal or higher rank".to_string());
    }
    if actor_level <= new_role_level {
        return Err("Cannot assign a role equal to or higher than your own".to_string());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// State guards
// ---------------------------------------------------------------------------

pub fn guard_thread_not_archived(archived: bool) -> Result<(), String> {
    if archived {
        Err("Cannot post in an archived thread".to_string())
    } else {
        Ok(())
    }
}

pub fn guard_thread_channel(thread_channel_id: u64, expected: u64) -> Result<(), String> {
    if thread_channel_id != expected {
        Err("Thread does not belong to this channel".to_string())
    } else {
        Ok(())
    }
}

pub fn guard_not_already_agent(is_agent: bool) -> Result<(), String> {
    if is_agent {
        Err("Already registered as an agent".to_string())
    } else {
        Ok(())
    }
}

pub fn guard_message_owner(sender_matches: bool) -> Result<(), String> {
    if sender_matches {
        Ok(())
    } else {
        Err("You can only edit your own messages".to_string())
    }
}

pub fn guard_delete_permission(is_sender: bool, is_admin: bool) -> Result<(), String> {
    if is_sender || is_admin {
        Ok(())
    } else {
        Err("You can only delete your own messages, or be an admin".to_string())
    }
}

pub fn guard_archive_permission(is_creator: bool, is_admin: bool) -> Result<(), String> {
    if is_creator || is_admin {
        Ok(())
    } else {
        Err("Only the thread creator or an admin can archive a thread".to_string())
    }
}

pub fn guard_owner_cannot_leave(is_owner: bool) -> Result<(), String> {
    if is_owner {
        Err("Server owner cannot leave. Transfer ownership or delete the server.".to_string())
    } else {
        Ok(())
    }
}

pub fn guard_only_owner_deletes(is_owner: bool) -> Result<(), String> {
    if is_owner {
        Ok(())
    } else {
        Err("Only the server owner can delete the server".to_string())
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // String validation
    // -----------------------------------------------------------------------

    #[test]
    fn username_valid() {
        assert!(validate_username("alice").is_ok());
    }

    #[test]
    fn username_at_limit() {
        let s = "a".repeat(32);
        assert!(validate_username(&s).is_ok());
    }

    #[test]
    fn username_empty() {
        assert!(validate_username("").is_err());
    }

    #[test]
    fn username_over_limit() {
        let s = "a".repeat(33);
        assert!(validate_username(&s).is_err());
    }

    #[test]
    fn name_valid_server() {
        assert!(validate_name("My Server", "Server").is_ok());
    }

    #[test]
    fn name_valid_channel() {
        assert!(validate_name("general", "Channel").is_ok());
    }

    #[test]
    fn name_at_limit() {
        let s = "x".repeat(100);
        assert!(validate_name(&s, "Server").is_ok());
    }

    #[test]
    fn name_empty() {
        let err = validate_name("", "Server").unwrap_err();
        assert!(err.contains("Server"));
    }

    #[test]
    fn name_over_limit() {
        let s = "x".repeat(101);
        let err = validate_name(&s, "Channel").unwrap_err();
        assert!(err.contains("Channel"));
    }

    #[test]
    fn message_content_valid() {
        assert!(validate_message_content("hello world").is_ok());
    }

    #[test]
    fn message_content_at_limit() {
        let s = "m".repeat(4000);
        assert!(validate_message_content(&s).is_ok());
    }

    #[test]
    fn message_content_empty() {
        assert!(validate_message_content("").is_err());
    }

    #[test]
    fn message_content_over_limit() {
        let s = "m".repeat(4001);
        assert!(validate_message_content(&s).is_err());
    }

    #[test]
    fn emoji_valid() {
        assert!(validate_emoji("👍").is_ok());
    }

    #[test]
    fn emoji_empty() {
        assert!(validate_emoji("").is_err());
    }

    #[test]
    fn agent_name_valid() {
        assert!(validate_agent_name("bot-001").is_ok());
    }

    #[test]
    fn agent_name_empty() {
        assert!(validate_agent_name("").is_err());
    }

    #[test]
    fn thread_name_valid() {
        assert!(validate_thread_name("Discussion").is_ok());
    }

    #[test]
    fn thread_name_empty() {
        assert!(validate_thread_name("").is_err());
    }

    #[test]
    fn dm_not_self_ok() {
        assert!(validate_dm_not_self("aaa", "bbb").is_ok());
    }

    #[test]
    fn dm_to_self_err() {
        assert!(validate_dm_not_self("aaa", "aaa").is_err());
    }

    // -----------------------------------------------------------------------
    // Role hierarchy constants
    // -----------------------------------------------------------------------

    #[test]
    fn role_level_ordering() {
        assert!(ROLE_LEVEL_OWNER > ROLE_LEVEL_ADMIN);
        assert!(ROLE_LEVEL_ADMIN > ROLE_LEVEL_MODERATOR);
        assert!(ROLE_LEVEL_MODERATOR > ROLE_LEVEL_MEMBER);
        assert_eq!(ROLE_LEVEL_MEMBER, ROLE_LEVEL_AGENT);
    }

    // -----------------------------------------------------------------------
    // can_modify_role
    // -----------------------------------------------------------------------

    #[test]
    fn modify_role_owner_demotes_admin() {
        // Owner (4) demoting Admin (3) to Member (1) — allowed
        assert!(can_modify_role(4, 3, 1, false, false).is_ok());
    }

    #[test]
    fn modify_role_admin_promotes_member_to_moderator() {
        // Admin (3) promoting Member (1) to Moderator (2) — allowed
        assert!(can_modify_role(3, 1, 2, false, false).is_ok());
    }

    #[test]
    fn modify_role_cannot_change_owner() {
        assert!(can_modify_role(4, 4, 3, true, false).is_err());
    }

    #[test]
    fn modify_role_cannot_assign_owner() {
        assert!(can_modify_role(4, 1, 4, false, true).is_err());
    }

    #[test]
    fn modify_role_cannot_outrank_target() {
        // Admin (3) trying to change Admin (3) — equal rank, rejected
        assert!(can_modify_role(3, 3, 1, false, false).is_err());
    }

    #[test]
    fn modify_role_cannot_assign_equal_rank() {
        // Admin (3) trying to promote to Admin (3) — equal to own, rejected
        assert!(can_modify_role(3, 1, 3, false, false).is_err());
    }

    #[test]
    fn modify_role_moderator_cannot_change_admin() {
        // Moderator (2) trying to change Admin (3) — outranked
        assert!(can_modify_role(2, 3, 1, false, false).is_err());
    }

    #[test]
    fn modify_role_member_cannot_change_anyone() {
        // Member (1) trying to change Member (1) — equal rank
        assert!(can_modify_role(1, 1, 1, false, false).is_err());
    }

    // -----------------------------------------------------------------------
    // State guards
    // -----------------------------------------------------------------------

    #[test]
    fn thread_not_archived_ok() {
        assert!(guard_thread_not_archived(false).is_ok());
    }

    #[test]
    fn thread_archived_err() {
        assert!(guard_thread_not_archived(true).is_err());
    }

    #[test]
    fn thread_channel_match() {
        assert!(guard_thread_channel(42, 42).is_ok());
    }

    #[test]
    fn thread_channel_mismatch() {
        assert!(guard_thread_channel(42, 99).is_err());
    }

    #[test]
    fn not_already_agent_ok() {
        assert!(guard_not_already_agent(false).is_ok());
    }

    #[test]
    fn already_agent_err() {
        assert!(guard_not_already_agent(true).is_err());
    }

    #[test]
    fn message_owner_ok() {
        assert!(guard_message_owner(true).is_ok());
    }

    #[test]
    fn message_not_owner_err() {
        assert!(guard_message_owner(false).is_err());
    }

    #[test]
    fn delete_by_sender() {
        assert!(guard_delete_permission(true, false).is_ok());
    }

    #[test]
    fn delete_by_admin() {
        assert!(guard_delete_permission(false, true).is_ok());
    }

    #[test]
    fn delete_denied() {
        assert!(guard_delete_permission(false, false).is_err());
    }

    #[test]
    fn archive_by_creator() {
        assert!(guard_archive_permission(true, false).is_ok());
    }

    #[test]
    fn archive_by_admin() {
        assert!(guard_archive_permission(false, true).is_ok());
    }

    #[test]
    fn archive_denied() {
        assert!(guard_archive_permission(false, false).is_err());
    }

    #[test]
    fn owner_cannot_leave() {
        assert!(guard_owner_cannot_leave(true).is_err());
    }

    #[test]
    fn non_owner_can_leave() {
        assert!(guard_owner_cannot_leave(false).is_ok());
    }

    #[test]
    fn only_owner_deletes_ok() {
        assert!(guard_only_owner_deletes(true).is_ok());
    }

    #[test]
    fn non_owner_cannot_delete_server() {
        assert!(guard_only_owner_deletes(false).is_err());
    }
}
