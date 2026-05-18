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
//! ## Canonicalization — Phase 1 scope
//!
//! Phase 1 canonicalization is intentionally minimal:
//!
//! 1. Concatenate text-only content blocks (ignore tool_use /
//!    tool_result / thinking — they aren't "what the user typed").
//! 2. Collapse runs of whitespace to a single ASCII space, then trim.
//!
//! That is **the entire canonical form**. Spec J L10-D2 mentions
//! stripping a "known tool-result re-injection wrapper" but does not
//! pin the wire shape, because the wire shape is an observed property
//! of Claude Code's runtime, not part of the Anthropic Messages public
//! contract. In practice Claude Code's tool_result re-injection rides
//! on subsequent `{type:"tool_result", tool_use_id, content}` content
//! blocks in later messages — the *first* user message stays
//! byte-identical across the tool-use HTTP round-trip — so the
//! whitespace-normalization-only canonical form is sufficient for sid
//! stability in current Claude Code (v0.x, May 2026).
//!
//! Empirical canonicalization of any future Claude Code re-injection
//! shape is deferred to **J-Sub-D (BRO-1143)**, where we'll observe
//! the actual tool round-trip behavior against a live Claude Code
//! client and add evidence-backed prefix/wrapper stripping (with
//! source-linked observed-bytes documentation) only if a real wire
//! shape demands it. Until that evidence exists, this codec does not
//! invent stripping rules.

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

/// Canonicalize the first user message body for sid hashing.
///
/// Steps (Phase 1, whitespace-normalization-only — see module docs):
/// 1. Concatenate text-only content blocks (ignore tool_use /
///    tool_result / thinking — they aren't "what the user typed").
/// 2. Collapse all whitespace runs to a single ASCII space, then trim.
///
/// The resulting bytes are deterministic for any *semantically*
/// equivalent user turn that differs only in whitespace. The "strip a
/// tool-result re-injection wrapper" requirement from Spec J L10-D2 is
/// deferred to J-Sub-D once we have empirical evidence of Claude
/// Code's actual re-injection shape.
pub fn canonicalize_first_user_message(content: &MessageContent) -> String {
    collapse_whitespace(&content.plain_text())
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
    fn synthesize_sid_stable_across_first_user_message_byte_identity() {
        // Per Spec J §[Tool use] example, Claude Code's tool_result
        // re-injection rides on subsequent content blocks in *later*
        // messages — the first user message stays byte-identical
        // across the tool-use HTTP round-trip. Whitespace-normalization
        // canonicalization is sufficient for sid stability in this
        // shape. (Empirical canonicalization of any future re-injection
        // shape is deferred to J-Sub-D; see module docs.)
        let req1 = req_with_user_text("read foo.txt");
        let req2 = req_with_user_text("read foo.txt");
        let did = "did:life:user1";
        let a = synthesize_sid(&req1, did).unwrap();
        let b = synthesize_sid(&req2, did).unwrap();
        assert_eq!(a, b);
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
                cache_control: None,
            },
            ContentBlock::ToolUse {
                id: "id1".into(),
                name: "t".into(),
                input: serde_json::json!({"x":1}),
                cache_control: None,
            },
        ]);
        let c = canonicalize_first_user_message(&blocks);
        assert_eq!(c, "user typed");
    }

    #[test]
    fn canonicalize_preserves_xml_like_content_verbatim() {
        // Phase 1 canonicalization is whitespace-normalization only.
        // Any XML-looking content the user actually typed must reach
        // the hasher unmodified. Empirical re-injection-prefix
        // stripping is deferred to J-Sub-D.
        let blocks = MessageContent::Text("<tool_result_re_injection>literal user text".into());
        let c = canonicalize_first_user_message(&blocks);
        assert_eq!(c, "<tool_result_re_injection>literal user text");
    }
}
