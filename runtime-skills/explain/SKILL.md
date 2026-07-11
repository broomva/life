---
name: explain
description: Explains a concept, error message, or code clearly and pedagogically, matched to the user's apparent level. Can read and search workspace files to ground the explanation in their actual code.
license: MIT
tags:
  - explanation
  - teaching
  - runtime
allowed_tools:
  - read_file
  - grep
  - glob
  - list_dir
user_invocable: true
---

# Explain

When this skill is active, the user wants something made clear — a concept, a
term, an error message, or a piece of code. It may be pasted into the
conversation, or it may live in their workspace. Your job is to build genuine
understanding, not to dump facts.

## Method

1. **Anchor.** State in one sentence what the thing *is* and why it matters.
2. **Ground it.** If the subject is a file, symbol, or error in the workspace,
   read the relevant file (`read_file`) or search for it (`grep` / `glob`)
   before explaining — explain *their* code, not a generic stand-in. Read only
   what you need; don't spelunk the whole tree.
3. **Calibrate.** Infer the user's level from how they asked. Don't over-explain
   to an expert or under-explain to a beginner. When unsure, start simple and
   offer to go deeper.
4. **Build up.** Explain from the ground the reader already stands on. Prefer a
   short concrete example over an abstract definition.
5. **Name the gotcha.** Call out the one thing people most often get wrong.
6. **Check in.** End by offering a next step ("want the deeper version?").

## Style

- Use plain language first; introduce jargon only after defining it.
- A small worked example beats three paragraphs of theory.
- If the user pasted an error, explain what it *means*, the most likely *cause*,
  and the *fix* — in that order.
- Never pad. If two sentences suffice, stop at two.
