# AGENTS.md

Instructions for coding agents working on ossteam (format:
<https://agents.md>). People may find them useful too. ossteam is a clean
Rust rewrite of the Steam client: see [project](AGENTS/project.md).

This file is only an index. Everything else is in notes, one fact per
file, in `AGENTS/[category/]*.md`; the format is in
[agent-notes](AGENTS/rules/agent-notes.md). Before starting work, read
this index, every rule below, and [docs/status.md](docs/status.md) (where
each milestone stands and what waits on the maintainer, with its notes in
`docs/status/`). Then open the notes that bear on the task.

Read these too if they exist:

- `AGENTS.local.md`: the index of `AGENTS.local/`, notes about the
  maintainer's test account and how ossteam sits on their machine. Both
  are gitignored because this repository is public. Put account names,
  IDs, personal paths and anything else about the maintainer there, never
  in committed files.
- `~/.config/agents/AGENTS.md` (or `~/.agents/AGENTS.md`): notes about
  the machine that hold for any project. They go there, not in
  `AGENTS.local`.

## Rules

These come from the maintainer. Don't trade them away for convenience.

- [Agent notes](AGENTS/rules/agent-notes.md) — knowledge goes only in AGENTS files and their notes, never agent-specific files or private memory
- [Cross-platform](AGENTS/rules/cross-platform.md) — Linux, Windows and macOS are first-class; keep every platform compiling
- [Faithful, never faked](AGENTS/rules/faithful-never-faked.md) — answers come from the real session or server; unimplemented features fail honestly
- [VAC and Steam DRM](AGENTS/rules/vac-drm-goals.md) — goals, implemented faithfully in M2, never bypassed
- [Platform-standard directories](AGENTS/rules/platform-directories.md) — only those, except the ABI discovery entries
- [Spec first](AGENTS/rules/spec-first.md) — reverse engineering is written up in `docs/spec/` before code; no Valve code
- [Review gates](AGENTS/rules/review-gates.md) — no DRM/VAC/cloud/friends/betas/workshop/offline code before review; ask about install-scripts §2.6
- [Synchronous first](AGENTS/rules/synchronous-first.md) — async only behind a sync facade or via an ADR; the gpui GUI until ADR 0004's port
- [Our own GUI toolkit](AGENTS/rules/gui-own-toolkit.md) — `ossteam-ui` on winit, ash (Vulkan) and our own software renderer, ADR 0004; no third-party GUI framework
- [SPEC changes](AGENTS/rules/spec-changes.md) — need the maintainer; say so plainly when reporting
- [CI stays off](AGENTS/rules/ci-stays-off.md) — don't switch CI jobs back on; mark anything else off with `RELEASE-BLOCKER`
- [Test account only](AGENTS/rules/test-account-only.md) — live runs use the maintainer's test account on the real install
- [Daemon restarts](AGENTS/rules/daemon-restarts.md) — don't restart the daemon casually; it once lost the account's token
- [Ask before visible actions](AGENTS/rules/ask-before-visible.md) — cloud writes, workshop subscriptions, friend requests
- [Official client files](AGENTS/rules/official-client-files.md) — the maintainer's personal account; structure only
- [Desktop settings](AGENTS/rules/desktop-settings.md) — ask before changing them

## Testing

- [CI](AGENTS/testing/ci.md) — macOS check/clippy/test only; test Linux and Windows locally
- [Windows under Wine](AGENTS/testing/windows-wine.md) — cross-check, run tests under Wine, GUI runs in a scratch prefix with fonts, what Wine can't show
- [Bench tests](AGENTS/testing/bench-tests.md) — the 10k-app library and gui_states with its test logins
- [Idle wakeups](AGENTS/testing/idle-wakeups.md) — count `voluntary_ctxt_switches` per thread
- [Driving the GUI by keyboard](AGENTS/testing/gui-keyboard-driving.md) — `wtype`, tab order, `XDG_RUNTIME_DIR`
- [Wayland scale tests](AGENTS/testing/wayland-scale-tests.md) — headless sway and `swaymsg` with `SWAYSOCK` set
- [Input on headless sway](AGENTS/testing/headless-sway-input.md) — wtype's lost first key, IME with `wayland_ime`, Xwayland keymaps, clipboard tools, portals on a private bus with `fake_portal.py`
- [Daemon autostart](AGENTS/testing/daemon-autostart.md) — `--no-autostart status` only looks

## Reference

- [Project](AGENTS/project.md) — what ossteam is and where the spec, runbooks and ADRs live
- [Research sources](AGENTS/research-sources.md) — outside sources, and which are facts-only
- [GUI focus](AGENTS/gui/focus.md) — focus handles, shared ids, reveal and page scrolling
- [ossteam-ui runtime](AGENTS/gui/ossteam-ui-runtime.md) — pre_present_notify only before a present, clipboard dropped before winit's connection, scrolled layers are damage, the gallery example
- [GUI lost keyboard focus](AGENTS/gui/lost-keyboard-focus.md) — seen once; check compositor focus first
- [Portal wakeups](AGENTS/gui/portal-wakeups.md) — connect the session bus yourself, or zbus's thread pool wakes every 500 ms
- [Steamworks ABI test](AGENTS/steamworks/abi-test.md) — what tests/abi.rs covers and the layout gotchas it found
- [macOS discovery](AGENTS/steamworks/macos-discovery.md) — the ipctool Mach service and what is unverified
