//! Compile-time-embedded mirror of Warp's GraphQL schema.

/// Raw SDL of Warp's GraphQL schema.
///
/// Loaded at compile time from `schema_mirror/schema.graphql`. This is a
/// verbatim copy of upstream Warp; modifications must follow the procedure
/// in `schema_mirror/UPSTREAM.md`.
///
/// # Example
///
/// ```
/// use lifegw::warp_graphql::schema_mirror::SDL;
/// assert!(SDL.contains("type RootQuery"));
/// assert!(SDL.contains("warpDriveUpdates"));
/// assert!(SDL.len() > 100_000); // ~4,133 lines
/// ```
pub const SDL: &str = include_str!("schema_mirror/schema.graphql");
