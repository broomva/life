//! Stateless sid synthesis for Spec J L10-D2.
//!
//! Anthropic Messages has no `conversation_id` header — every request
//! carries a full `messages: [...]` history. To keep cross-request
//! sessions stable, lifegw derives a deterministic Life session id
//! from `(anima_did, canonical_first_user_message)`:
//!
//! ```text
//! sid = "claude-code:" || hex(sha256(did || "::" || canon))[..16]
//! ```
//!
//! The 16-hex-char prefix gives 2^64 collision space per anima — enough
//! for any single user's lifetime per Spec J §[Locked Decisions L10-D2].
//!
//! ## Canonicalization
//!
//! Claude Code re-injects prior `tool_result` content into subsequent
//! requests by prefixing the first user message with a marker block
//! (`<tool_result_re_injection>...</tool_result_re_injection>`). The
//! re-injected bytes mean the *literal* `messages[0].content` differs
//! between request 1 and request 2 even though the conversation is the
//! same. Canonicalization strips that prefix and normalizes whitespace
//! so sid stays stable across the tool-use HTTP round-trip.

use sha2::{Digest, Sha256};

use crate::errors::CodecError;
use crate::request::{AnthropicMessagesRequest, MessageContent};

/// Fixed prefix on every claude-code-derived Life sid.
///
/// Embedded in the wire form so operators can distinguish synthesized
/// sids from native lifed sids in `lago log` / `lago replay --tree`
/// output.
pub const SID_PREFIX: &str = "claude-code:";

/// Length of the hex digest tail (out of the full 64-char SHA-256 hex).
///
/// 16 hex chars == 64 bits == 2^64 collision space per anima.
const SID_HEX_LEN: usize = 16;

/// Tool-result re-injection wrapper Claude Code uses. The exact bytes
/// are an *observed convention* — the public Anthropic Messages spec
/// does not document them. If Claude Code changes this prefix, sid
/// stability degrades to "best-effort"; the prefix list is the only
/// mutable knob.
///
/// Each prefix is checked in order against the *trimmed* user message.
/// If any matches, the matching prefix is stripped before hashing.
const REINJECTION_PREFIXES: &[&str] = &[
    "<tool_result_re_injection>",
    "<system-tool-result>",
    "<bash-stdout>",
    "<file-content>",
];

/// Closing tag, only stripped when the matching opening prefix is
/// present. Ordering must mirror [`REINJECTION_PREFIXES`].
const REINJECTION_SUFFIXES: &[&str] = &[
    "</tool_result_re_injection>",
    "</system-tool-result>",
    "</bash-stdout>",
    "</file-content>",
];

/// Canonicalize the first user message body for sid hashing.
///
/// Steps:
/// 1. Concatenate text-only content blocks (ignore tool_use /
///    tool_result / thinking — they aren't "what the user typed").
/// 2. Strip a known tool-result re-injection wrapper if present.
/// 3. Collapse all whitespace runs to a single ASCII space, then trim.
///
/// The resulting bytes are deterministic for any *semantically*
/// equivalent user turn — additional whitespace, mid-stream re-prompts
/// with re-injection, and Claude Code's tool-result reformatting all
/// collapse to the same canonical form.
pub fn canonicalize_first_user_message(content: &MessageContent) -> String {
    let raw = content.plain_text();
    let stripped = strip_reinjection_wrapper(&raw);
    collapse_whitespace(stripped)
}

fn strip_reinjection_wrapper(raw: &str) -> &str {
    // Returns the slice of `raw` that remains AFTER the wrapper has
    // been excised. If no wrapper is present, returns `raw` unchanged.
    //
    // The mental model: a re-injection wrapper is a *prefix* on the
    // user's real first message — strip wrapper + wrapper body +
    // optional close tag, keep the rest. Example:
    //
    //   <tool_result_re_injection>...</tool_result_re_injection>real message
    //   ⇒ "real message"
    let trimmed = raw.trim_start();
    for (open, close) in REINJECTION_PREFIXES.iter().zip(REINJECTION_SUFFIXES.iter()) {
        if let Some(after_open) = trimmed.strip_prefix(open) {
            // Skip past the matching close tag (and everything before
            // it). If no close is present (Claude Code occasionally
            // drops it at message-end truncation), keep what's after
            // the opening tag — best-effort recovery.
            if let Some(idx) = after_open.find(close) {
                let after_close = &after_open[idx + close.len()..];
                return after_close;
            }
            return after_open;
        }
    }
    raw
}

fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_was_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !last_was_space && !out.is_empty() {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(ch);
            last_was_space = false;
        }
    }
    // Trim a trailing space if the final char was whitespace.
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Synthesize a stable Life sid for a Claude Code request + anima DID.
///
/// Returns an error if the request has no user message (the canonical
/// content would be undefined).
///
/// # Wire form
///
/// `claude-code:<16-hex-chars>` — exactly 28 ASCII characters.
///
/// # Determinism
///
/// For any fixed `(did, canonical-first-user-message)` pair, the
/// returned string is byte-for-byte equal across calls and across
/// processes. SHA-256 over UTF-8 bytes is canonical.
pub fn synthesize_sid(req: &AnthropicMessagesRequest, did: &str) -> Result<String, CodecError> {
    let first = req.first_user_message().ok_or(CodecError::NoUserMessage)?;
    let canon = canonicalize_first_user_message(&first.content);

    let mut hasher = Sha256::new();
    hasher.update(did.as_bytes());
    hasher.update(b"::");
    hasher.update(canon.as_bytes());
    let digest = hasher.finalize();

    let hex = hex::encode(digest);
    // hex::encode never produces fewer than 64 chars for a SHA-256
    // digest, so the slice is always in-bounds.
    debug_assert!(hex.len() >= SID_HEX_LEN);
    Ok(format!("{SID_PREFIX}{}", &hex[..SID_HEX_LEN]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::{Message, Role};

    fn req_with_user_text(text: &str) -> AnthropicMessagesRequest {
        AnthropicMessagesRequest {
            model: "m".into(),
            messages: vec![Message {
                role: Role::User,
                content: MessageContent::Text(text.into()),
            }],
            system: None,
            max_tokens: 1,
            stop_sequences: vec![],
            stream: false,
            temperature: None,
            top_p: None,
            top_k: None,
            metadata: None,
            tools: vec![],
            tool_choice: None,
            thinking: None,
        }
    }

    #[test]
    fn synthesize_sid_has_canonical_prefix_and_length() {
        let req = req_with_user_text("hello world");
        let sid = synthesize_sid(&req, "did:life:user123").unwrap();
        assert!(sid.starts_with(SID_PREFIX));
        assert_eq!(sid.len(), SID_PREFIX.len() + SID_HEX_LEN);
    }

    #[test]
    fn synthesize_sid_is_deterministic() {
        let req = req_with_user_text("read foo.txt");
        let a = synthesize_sid(&req, "did:life:user1").unwrap();
        let b = synthesize_sid(&req, "did:life:user1").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn synthesize_sid_changes_with_did() {
        let req = req_with_user_text("read foo.txt");
        let a = synthesize_sid(&req, "did:life:user1").unwrap();
        let b = synthesize_sid(&req, "did:life:user2").unwrap();
        assert_ne!(a, b, "sid must bind to DID");
    }

    #[test]
    fn synthesize_sid_changes_with_first_user_message() {
        let req1 = req_with_user_text("read foo.txt");
        let req2 = req_with_user_text("read bar.txt");
        let a = synthesize_sid(&req1, "did:life:user1").unwrap();
        let b = synthesize_sid(&req2, "did:life:user1").unwrap();
        assert_ne!(a, b, "sid must bind to first user message");
    }

    #[test]
    fn synthesize_sid_is_whitespace_normalized() {
        let req1 = req_with_user_text("read foo.txt");
        let req2 = req_with_user_text("read    foo.txt");
        let req3 = req_with_user_text("read\tfoo.txt");
        let req4 = req_with_user_text("  read foo.txt  ");
        let did = "did:life:user1";
        let a = synthesize_sid(&req1, did).unwrap();
        let b = synthesize_sid(&req2, did).unwrap();
        let c = synthesize_sid(&req3, did).unwrap();
        let d = synthesize_sid(&req4, did).unwrap();
        assert_eq!(a, b);
        assert_eq!(a, c);
        assert_eq!(a, d);
    }

    #[test]
    fn synthesize_sid_strips_tool_result_reinjection_prefix() {
        let plain = req_with_user_text("read foo.txt");
        let wrapped = req_with_user_text(
            "<tool_result_re_injection>{\"path\":\"foo.txt\"}</tool_result_re_injection>read foo.txt",
        );
        let did = "did:life:user1";
        let a = synthesize_sid(&plain, did).unwrap();
        let b = synthesize_sid(&wrapped, did).unwrap();
        assert_eq!(
            a, b,
            "sid must be stable across the tool-use HTTP round-trip"
        );
    }

    #[test]
    fn synthesize_sid_rejects_request_without_user_message() {
        let req = AnthropicMessagesRequest {
            model: "m".into(),
            messages: vec![Message {
                role: Role::Assistant,
                content: MessageContent::Text("a".into()),
            }],
            system: None,
            max_tokens: 1,
            stop_sequences: vec![],
            stream: false,
            temperature: None,
            top_p: None,
            top_k: None,
            metadata: None,
            tools: vec![],
            tool_choice: None,
            thinking: None,
        };
        let e = synthesize_sid(&req, "did").unwrap_err();
        assert!(matches!(e, CodecError::NoUserMessage));
    }

    #[test]
    fn canonicalize_ignores_tool_use_blocks() {
        use crate::request::ContentBlock;
        let blocks = MessageContent::Blocks(vec![
            ContentBlock::Text {
                text: "user typed".into(),
            },
            ContentBlock::ToolUse {
                id: "id1".into(),
                name: "t".into(),
                input: serde_json::json!({"x":1}),
            },
        ]);
        let c = canonicalize_first_user_message(&blocks);
        assert_eq!(c, "user typed");
    }

    #[test]
    fn canonicalize_handles_open_only_reinjection_tag() {
        // Truncated close tag — still strip the opener.
        let blocks = MessageContent::Text("<tool_result_re_injection>real user message".into());
        let c = canonicalize_first_user_message(&blocks);
        assert_eq!(c, "real user message");
    }
}
