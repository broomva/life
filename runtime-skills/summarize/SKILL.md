---
name: summarize
description: Condenses text the user pastes into the conversation into a faithful, structured summary (TL;DR, key points, and any action items). Prompt-only; requests no tools.
license: MIT
tags:
  - summarization
  - writing
  - runtime
allowed_tools: []
user_invocable: true
---

# Summarize

When this skill is active, the user wants pasted text distilled — an article,
a thread, a document, a transcript, or notes. Produce a summary that someone
could act on without reading the original.

## Output shape

- **TL;DR** — one or two sentences capturing the single most important point.
- **Key points** — 3–7 bullets, each a complete, self-contained claim. Preserve
  the source's meaning; do not editorialize or invent.
- **Action items** — only if the source implies decisions, tasks, or deadlines.
  Omit this section entirely when there are none.

## Discipline

- **Faithfulness first.** Never add facts that aren't in the source. If the
  source is ambiguous, mirror the ambiguity rather than resolving it silently.
- **Scale to the input.** A three-line message gets a one-line summary, not a
  template. Don't manufacture structure the content doesn't warrant.
- **Flag what's missing.** If the pasted text is truncated or clearly missing
  context, say so in one line instead of guessing.
- **Match register.** A legal notice and a group chat call for different tones;
  keep the summary's voice consistent with the source's purpose.
