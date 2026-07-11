---
name: summarize
description: Condenses text into a faithful, structured summary (TL;DR, key points, and any action items). Works on text pasted into the conversation or on files read from the workspace.
license: MIT
tags:
  - summarization
  - writing
  - runtime
allowed_tools:
  - read_file
  - glob
  - list_dir
user_invocable: true
---

# Summarize

When this skill is active, the user wants content distilled — an article, a
thread, a document, a transcript, or notes. The content may be pasted into the
conversation, or it may be a file in their workspace. Produce a summary that
someone could act on without reading the original.

## Getting the content

- If the user pasted the text, work from that.
- If they named a file or path, `read_file` it (use `glob` / `list_dir` to
  locate it first when the path is fuzzy). Read only what you're summarizing —
  don't pull in unrelated files.

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
- **Flag what's missing.** If the source is truncated or clearly missing
  context, say so in one line instead of guessing.
- **Match register.** Keep the summary's voice consistent with the source's
  purpose.
