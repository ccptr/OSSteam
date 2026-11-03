---
name: agent-notes
description: The note format, how to keep notes, and that agent knowledge goes only in AGENTS files, never agent-specific ones
metadata:
  type: feedback
---

A note is one Markdown file holding one fact, named after its `name`,
with frontmatter: `name`, a one-line `description`, and `metadata.type`:

- `user`: the maintainer and their accounts (local and global notes only).
- `feedback`: how the maintainer wants work done, with the reason. The
  rules are feedback notes.
- `project`: where work stands, what waits on whom, decisions not yet
  confirmed.
- `reference`: facts, how-tos, gotchas, pointers to sources and tools.

Keeping them:

- Each note has one line in its index: `- [Title](path.md) — hook`.
  Indexes hold only these lines under headings, plus their own short
  intro.
- Update an existing note rather than adding a near-duplicate. Delete
  notes that stop being true, with their index lines.
- Write absolute dates. Point to commits for details.
- Link notes with relative Markdown links so they work on GitHub.
  Committed notes never link to local or global ones, and never name the
  maintainer's accounts or paths.

Use only the AGENTS files (and the notes they index). Do not create
agent-specific instruction or memory files (`CLAUDE.md`,
`CLAUDE.local.md`, `.claude/`, `.cursorrules`, `.cursor/`,
`.github/copilot-instructions.md`, `GEMINI.md` and the like), and do not
keep project knowledge in an agent's private memory. The one exception
is the committed `CLAUDE.md`, which only points Claude Code at AGENTS.md.
Keep it a pointer; don't add instructions or notes to it.

A subdirectory may get its own `AGENTS.md` when it needs rules of its
own; the nearest one wins.
