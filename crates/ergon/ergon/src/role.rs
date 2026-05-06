//! Role overlays — call/session/agent precedence, never persisted.
//!
//! A [`Role`] is a lightweight system-prompt overlay. Three scopes exist
//! ([`RoleScope::Call`], [`RoleScope::Session`], [`RoleScope::Agent`]) with
//! strict precedence: **call > session > agent**.
//!
//! ## Critical invariant
//!
//! Roles are NEVER inserted into [`crate::SessionId`]-keyed history. They are
//! applied at `ModelRequest` build time (in step.rs's autonomous loop, when
//! that lands). Inserting a role into history is a bug — the precedence rule
//! becomes meaningless if roles get baked in.
//!
//! ## Merge semantics
//!
//! Roles compose cumulatively for `instructions` (concatenated in
//! agent → session → call order so call-scope instructions appear *last*
//! and dominate the model's most-recent attention) and override-style for
//! `identity` (highest scope wins outright).

use serde::{Deserialize, Serialize};

/// Precedence scope for a [`Role`] overlay.
///
/// **Order**: [`Self::Call`] > [`Self::Session`] > [`Self::Agent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum RoleScope {
    /// Workflow / agent default. Lowest precedence.
    Agent,
    /// Bound for the lifetime of a session. Middle precedence.
    Session,
    /// Bound for a single call. Highest precedence.
    Call,
}

impl RoleScope {
    /// Numeric precedence for ordering — higher value dominates.
    pub fn precedence(self) -> u8 {
        match self {
            Self::Agent => 0,
            Self::Session => 1,
            Self::Call => 2,
        }
    }
}

/// A role overlay — a system-prompt fragment scoped to a specific lifetime.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Role {
    /// Optional identity assertion (e.g., `"You are the bookkeeping judge."`).
    pub identity: Option<String>,
    /// Ordered list of instruction fragments. Merged cumulatively across
    /// scopes in the order agent → session → call.
    pub instructions: Vec<String>,
    /// Which scope this overlay binds to.
    pub scope: RoleScope,
}

impl Default for Role {
    /// Default role: no identity, no instructions, agent-scope.
    fn default() -> Self {
        Self {
            identity: None,
            instructions: Vec::new(),
            scope: RoleScope::Agent,
        }
    }
}

impl Role {
    /// Construct a new agent-scope role with the given identity assertion.
    pub fn agent(identity: impl Into<String>) -> Self {
        Self {
            identity: Some(identity.into()),
            instructions: Vec::new(),
            scope: RoleScope::Agent,
        }
    }

    /// Append an instruction fragment.
    pub fn with_instruction(mut self, line: impl Into<String>) -> Self {
        self.instructions.push(line.into());
        self
    }

    /// Set the scope of this role.
    pub fn with_scope(mut self, scope: RoleScope) -> Self {
        self.scope = scope;
        self
    }

    /// Merge agent / session / call roles per the **call > session > agent**
    /// precedence rule.
    ///
    /// - `identity`: highest-precedence non-None wins.
    /// - `instructions`: agent first, then session, then call (so the model
    ///   sees the most-specific instruction last and weights it most).
    /// - `scope`: the result has [`RoleScope::Call`] if any call-scope
    ///   override was provided, otherwise [`RoleScope::Session`] if a
    ///   session role was provided, otherwise [`RoleScope::Agent`].
    pub fn merge(agent: &Self, session: Option<&Self>, call: Option<&Self>) -> Self {
        let identity = call
            .and_then(|r| r.identity.clone())
            .or_else(|| session.and_then(|r| r.identity.clone()))
            .or_else(|| agent.identity.clone());

        let mut instructions = Vec::with_capacity(
            agent.instructions.len()
                + session.map_or(0, |r| r.instructions.len())
                + call.map_or(0, |r| r.instructions.len()),
        );
        instructions.extend(agent.instructions.iter().cloned());
        if let Some(s) = session {
            instructions.extend(s.instructions.iter().cloned());
        }
        if let Some(c) = call {
            instructions.extend(c.instructions.iter().cloned());
        }

        let scope = if call.is_some() {
            RoleScope::Call
        } else if session.is_some() {
            RoleScope::Session
        } else {
            RoleScope::Agent
        };

        Self {
            identity,
            instructions,
            scope,
        }
    }

    /// Render the merged role into a single system-prompt string.
    ///
    /// Returns `None` if the role contributes no content (no identity and
    /// no instructions). The exact text format is:
    ///
    /// ```text
    /// <identity>
    ///
    /// <instruction 1>
    /// <instruction 2>
    /// ...
    /// ```
    ///
    /// with the blank line elided when either section is empty.
    pub fn render(&self) -> Option<String> {
        let has_identity = self.identity.as_ref().is_some_and(|s| !s.is_empty());
        let has_instructions = !self.instructions.is_empty();
        if !has_identity && !has_instructions {
            return None;
        }
        let mut out = String::new();
        if let Some(id) = &self.identity {
            out.push_str(id);
        }
        if has_identity && has_instructions {
            out.push_str("\n\n");
        }
        for (i, line) in self.instructions.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(line);
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_order_is_call_session_agent() {
        assert!(RoleScope::Call.precedence() > RoleScope::Session.precedence());
        assert!(RoleScope::Session.precedence() > RoleScope::Agent.precedence());
    }

    #[test]
    fn default_role_renders_to_none() {
        assert!(Role::default().render().is_none());
    }

    #[test]
    fn merge_with_only_agent_returns_agent_scope() {
        let agent = Role::agent("be helpful");
        let merged = Role::merge(&agent, None, None);
        assert_eq!(merged.scope, RoleScope::Agent);
        assert_eq!(merged.identity.as_deref(), Some("be helpful"));
    }

    #[test]
    fn merge_call_overrides_identity() {
        let agent = Role::agent("agent persona");
        let call = Role::agent("call persona").with_scope(RoleScope::Call);
        let merged = Role::merge(&agent, None, Some(&call));
        assert_eq!(merged.identity.as_deref(), Some("call persona"));
        assert_eq!(merged.scope, RoleScope::Call);
    }

    #[test]
    fn merge_session_overrides_agent_when_no_call() {
        let agent = Role::agent("agent persona");
        let session = Role::agent("session persona").with_scope(RoleScope::Session);
        let merged = Role::merge(&agent, Some(&session), None);
        assert_eq!(merged.identity.as_deref(), Some("session persona"));
        assert_eq!(merged.scope, RoleScope::Session);
    }

    #[test]
    fn merge_concatenates_instructions_agent_session_call() {
        let agent = Role::default()
            .with_instruction("agent-1")
            .with_instruction("agent-2");
        let session = Role::default()
            .with_scope(RoleScope::Session)
            .with_instruction("session-1");
        let call = Role::default()
            .with_scope(RoleScope::Call)
            .with_instruction("call-1");
        let merged = Role::merge(&agent, Some(&session), Some(&call));
        assert_eq!(
            merged.instructions,
            vec!["agent-1", "agent-2", "session-1", "call-1"]
        );
    }

    #[test]
    fn render_includes_identity_then_instructions() {
        let role = Role::agent("You are the judge.")
            .with_instruction("Be terse.")
            .with_instruction("Cite sources.");
        let rendered = role.render().expect("non-empty");
        assert_eq!(rendered, "You are the judge.\n\nBe terse.\nCite sources.");
    }

    #[test]
    fn render_with_only_instructions_omits_blank_line() {
        let role = Role::default().with_instruction("be brief");
        assert_eq!(role.render().as_deref(), Some("be brief"));
    }

    #[test]
    fn render_empty_identity_treated_as_absent() {
        let role = Role {
            identity: Some(String::new()),
            instructions: vec!["hello".into()],
            scope: RoleScope::Agent,
        };
        assert_eq!(role.render().as_deref(), Some("hello"));
    }

    #[test]
    fn merge_higher_precedence_identity_dominates_when_lower_is_some() {
        let agent = Role::agent("agent");
        let session = Role::agent("session").with_scope(RoleScope::Session);
        let call = Role::agent("call").with_scope(RoleScope::Call);
        let merged = Role::merge(&agent, Some(&session), Some(&call));
        assert_eq!(merged.identity.as_deref(), Some("call"));
    }

    #[test]
    fn merge_falls_back_when_higher_scope_has_none_identity() {
        let agent = Role::agent("agent persona");
        let call = Role::default().with_scope(RoleScope::Call); // identity=None
        let merged = Role::merge(&agent, None, Some(&call));
        // call has no identity → falls back to agent's
        assert_eq!(merged.identity.as_deref(), Some("agent persona"));
        // but scope still tracks the highest-supplied scope
        assert_eq!(merged.scope, RoleScope::Call);
    }
}
