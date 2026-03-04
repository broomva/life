use spacetimedb::{reducer, ReducerContext};

use crate::tables::{user_profile, UserProfile};
use crate::validation;

#[reducer]
pub fn set_profile(
    ctx: &ReducerContext,
    username: String,
    display_name: Option<String>,
    avatar_url: Option<String>,
    bio: Option<String>,
) -> Result<(), String> {
    validation::validate_username(&username)?;

    let profile = ctx
        .db
        .user_profile()
        .identity()
        .find(ctx.sender())
        .ok_or("User profile not found. Connect first.")?;

    // Check username uniqueness (if changing)
    if profile.username != username && ctx.db.user_profile().username().find(&username).is_some() {
        return Err(format!("Username '{}' is already taken", username));
    }

    ctx.db.user_profile().identity().update(UserProfile {
        username,
        display_name,
        avatar_url,
        bio,
        ..profile
    });

    Ok(())
}

#[reducer]
pub fn register_agent(
    ctx: &ReducerContext,
    agent_name: String,
    description: String,
) -> Result<(), String> {
    validation::validate_agent_name(&agent_name)?;

    let profile = ctx
        .db
        .user_profile()
        .identity()
        .find(ctx.sender())
        .ok_or("User profile not found. Connect first.")?;

    validation::guard_not_already_agent(profile.is_agent)?;

    // Check username uniqueness if changing
    if profile.username != agent_name
        && ctx.db.user_profile().username().find(&agent_name).is_some()
    {
        return Err(format!("Name '{}' is already taken", agent_name));
    }

    ctx.db.user_profile().identity().update(UserProfile {
        username: agent_name,
        bio: Some(description),
        is_agent: true,
        ..profile
    });

    Ok(())
}
