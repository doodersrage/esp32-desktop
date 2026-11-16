---
name: update-plan
description: Keep docs/PLAN.md and CLAUDE.md in step with the project. Use when a phase finishes, an open decision is settled, scope changes, or an estimate turns out wrong.
---

# Updating the plan

`docs/PLAN.md` is the source of truth for scope, decisions and phases. `CLAUDE.md` repeats the settled and open decisions so they are in context for every session. The two must agree.

## When a decision is settled

1. In `docs/PLAN.md`, update the row in "Key decisions": state the choice and the reason in one or two sentences.
2. In `CLAUDE.md`, move the item from "Decisions that are open" to "Decisions that are settled".
3. If the decision changes a phase's work or estimate, update the "Phases" table and the total in the paragraph above it.
4. If the decision is the QEMU fork, also set the GPL version, add the `LICENSE` file, and update the Licence section of `README.md`.

## When a phase finishes

1. Mark the phase as done in the "Phases" table and note the actual effort next to the estimate.
2. Write what was learned as a short note in `docs/` (for phase 0, the fork comparison results).
3. Update "Current state" in `CLAUDE.md` and "Status" in `README.md` to name the next phase.
4. If actual effort was well off the estimate, rescale the remaining phases and say so in the commit message.

## When scope changes

- Only the owner changes scope. If work suggests a scope change, propose it; do not edit the plan first.
- Once agreed, update "Scope for v1" in the plan and working rule 9 in `CLAUDE.md` if the non-goals changed.

## Style

- First sentence of each section states its point.
- Use numbers, names and dates, not adjectives.
- Keep the "Last updated" date at the top of the plan current.
- Estimates are in weeks at about 14 hours a week.
