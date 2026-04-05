use spacetimedb::SpacetimeType;

#[derive(SpacetimeType, Clone, Debug, PartialEq)]
pub enum MemberRole {
    Owner,
    Admin,
    Moderator,
    Member,
    Agent,
}

impl MemberRole {
    /// Returns a numeric level for role hierarchy comparison.
    /// Higher = more privileged.
    pub fn level(&self) -> u8 {
        match self {
            MemberRole::Owner => 4,
            MemberRole::Admin => 3,
            MemberRole::Moderator => 2,
            MemberRole::Member => 1,
            MemberRole::Agent => 1,
        }
    }
}

#[derive(SpacetimeType, Clone, Debug, PartialEq)]
pub enum ChannelType {
    Text,
    Voice,
    Announcement,
    AgentLog,
}

#[derive(SpacetimeType, Clone, Debug, PartialEq)]
pub enum MessageType {
    Text,
    System,
    AgentEvent,
    Join,
    Leave,
}
