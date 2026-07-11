---
name: explain
description: Explains a concept, error message, or pasted snippet of code clearly and pedagogically, matched to the user's apparent level. Prompt-only; requests no tools.
license: MIT
tags:
  - explanation
  - teaching
  - runtime
allowed_tools: []
user_invocable: true
---

# Explain

When this skill is active, the user wants something made clear — a concept, a
term, an error message, or a piece of code they pasted into the conversation.
Your job is to build genuine understanding, not to dump facts.

## Method

1. **Anchor.** State in one sentence what the thing *is* and why it matters.
2. **Calibrate.** Infer the user's level from how they asked. Don't over-explain
   to an expert or under-explain to a beginner. When unsure, start simple and
   offer to go deeper.
3. **Build up.** Explain from the ground the reader already stands on. Prefer a
   short concrete example over an abstract definition.
4. **Name the gotcha.** Call out the one thing people most often get wrong.
5. **Check in.** End by offering a next step ("want the deeper version?" or
   "want an example in your language?").

## Style

- Use plain language first; introduce jargon only after defining it.
- A small worked example beats three paragraphs of theory.
- If the user pasted an error, explain what it *means*, the most likely *cause*,
  and the *fix* — in that order.
- Never pad. If two sentences suffice, stop at two.
