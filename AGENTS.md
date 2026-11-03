# AGENTS.md

Instructions for coding agents working on ossteam (format:
<https://agents.md>). People may find them useful too.

## Where agent notes live

- This file holds the rules and how to work here. Where each milestone
  stands, and what waits on the maintainer, is in
  [docs/status.md](docs/status.md). Read both before starting work.
- If `AGENTS.local.md` exists next to this file, read it too. It holds
  notes about the maintainer's machine and test account. It is
  gitignored because this repository is public. Put account names, IDs,
  personal paths and anything else about the maintainer there, never in
  committed files.
- Use only these files. Do not create agent-specific instruction or
  memory files (`CLAUDE.md`, `CLAUDE.local.md`, `.claude/`,
  `.cursorrules`, `.cursor/`, `.github/copilot-instructions.md`,
  `GEMINI.md` and the like), and do not keep project knowledge in an
  agent's private memory. When you learn something the next session
  needs, update `docs/status.md`, this file or `AGENTS.local.md`. The
  one exception is the committed `CLAUDE.md`, which only points Claude
  Code here. Keep it a pointer; don't add instructions or notes to it.
- A subdirectory may get its own `AGENTS.md` when it needs rules of its
  own; the nearest one wins.

## The project

ossteam is a clean Rust rewrite of the Steam client, GPL-2.0-or-later. A
per-user daemon, `ossteamd`, owns the Steam session, the library and game
processes. Thin clients drive it: the `ossteam` CLI (clap) and
`ossteam-gui` (gpui).

- [SPEC.md](SPEC.md): the design, its rules (IDs like CONC-2, PLAT-1)
  and the milestones (§21).
- `docs/spec/`: subsystem specs. `docs/acceptance/`: per-milestone
  runbooks and results. `docs/adr/`: decisions.
- `docs/release-checklist.md`: things switched off during development.

## Rules

These come from the maintainer. Don't trade them away for convenience.

- **Cross-platform.** Linux, Windows and macOS are all first-class; Proton
  support is the only Linux-only part. Keep every platform compiling.
- **Faithful, never faked** (SPEC §2.1). Every answer comes from the real
  session or server. An unimplemented feature fails honestly. Reject any
  shortcut that stubs or fakes behaviour.
- **VAC and Steam DRM** (SteamStub, CEG) are goals. They are implemented
  faithfully in M2, never bypassed.
- **Platform-standard directories only** (§2.5). The one exception is the
  ABI discovery entries (`~/.steam/sdk*`, the ActiveProcess registry key),
  which the daemon creates and removes (§12.5).
- **Spec first** (§2.6). Reverse engineering is allowed for interop and is
  written up under `docs/spec/` before code. No Valve code is copied. Some
  sources may only be used for facts, not code: Proton (vtables, struct
  layouts), Steamless (CC BY-NC-ND), community tables.
- **Review gates.** No DRM, VAC, cloud, friends, betas, workshop or
  offline code before the maintainer has reviewed that spec page and
  answered its questions. Ask about install-scripts.md §2.6 before writing
  HKLM writes or prefix install scripts. The current gates are listed in
  docs/status.md.
- **Synchronous first** (§2.3). Async only inside gpui or via an ADR.
- **GUI widgets on plain gpui**, in `crates/ossteam-gui/src/widgets/`. Do
  not add `gpui-component`. The gpui examples at the pinned rev are the
  reference: `$CARGO_HOME/git/checkouts/zed-*/<rev>/crates/gpui/examples`.
- **SPEC changes need the maintainer.** If a change rewords a SPEC rule or
  amends an ADR, say so plainly when you report it.

## CI during development

CI runs on macOS only, and only `cargo check`, `cargo clippy` and
`cargo test`. Linux and Windows builds, release builds and artifacts, and
nightly fuzzing are switched off on purpose. Don't switch them back on
during development. Anything else switched off for now gets a
`RELEASE-BLOCKER` marker where it is switched off and an entry in
`docs/release-checklist.md`.

So test locally: Linux natively, Windows by cross-check plus Wine (below).
macOS-only code can be type-checked with `--target aarch64-apple-darwin`
but not built (ring's C does not cross-compile without a macOS SDK); its
tests first run on CI.

## Building and testing

- If fetching new git dependencies fails (DNS through a VPN, for
  example), fetch once with working network access (`cargo fetch`), then
  build with `--offline`.
- Windows: `cargo check --target x86_64-pc-windows-gnu --workspace` (and
  clippy). Tests under Wine:

  ```sh
  CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER=wine WINEDEBUG=-all \
    cargo test --offline --target x86_64-pc-windows-gnu --workspace
  ```

  The steamclient ABI test (`ossteam-steamclient/tests/abi.rs`) needs
  `OSSTEAM_ABI_GAME` set to a game built for the target's arch (commands
  in abi.rs's docs); without it, it tries `clang-cl` and fails under
  Wine. Registry tests write under `HKCU\Software\ossteam-test` and clean
  up after themselves (`grep ossteam-test ~/.wine/user.reg` should come up
  empty). Not covered under Wine: `reaper.rs` (Linux only) and
  `ossteam-steamclient/tests/pump.rs` (Unix only). Wine is not Windows:
  UAC, runas `--script-helper`, real HKLM and firewall behaviour stay
  unverified until a real Windows run.
- Run the workspace's ignored bench tests with `--bin ossteamd` (ossteamd
  has no lib target):
  - `gui_bench_library_of_10k_apps` (`ossteamd/src/testing/gui_bench.rs`)
    serves 10k fake-CM apps. Start the GUI with the `XDG_RUNTIME_DIR` it
    prints.
  - `gui_bench_states` (`testing/gui_states.rs`) serves a small library
    in every state, plus accounts bob (email code BOB12), erin
    (authenticator ERIN5), dave (confirmation, auto-approved after 10 s)
    and a QR login that signs in as carol. Passwords are
    `<name>-password`. `touch <root>/update-sleeper` publishes an update
    to the Sleeper app. The bench game takes `-report` to run the shim
    checks. Bench apps have no install directory, so Install fails there.
    Bench jobs finish fast, so pause one with `ossteam jobs pause N`.
- Measure idle wakeups per thread from `voluntary_ctxt_switches` in
  `/proc/<pid>/task/*/status`. The bench daemon's RSS means nothing,
  because the fake CM runs in the same process.
- Driving the GUI by keyboard (e.g. `wtype`): a fresh window has no focus.
  Tab order from there: Library, Downloads, Settings, account menu,
  search, …, grid. `PgUp`/`PgDn`/`Home`/`End` scroll pages. Don't export
  the bench's `XDG_RUNTIME_DIR` in the shell that runs `wtype`.
- To test scale changes on Wayland without touching the desktop, run a
  headless sway (`WLR_BACKENDS=headless sway -c <config>`) and switch
  scale with `swaymsg output HEADLESS-1 scale N`.
- Never `pkill -f` a pattern that also appears in your own command line;
  it kills your shell. Use `pkill -x`.

## Live testing

Anything that needs a Steam account uses the maintainer's test account,
signed in on the real install. Never use a sandboxed root or an account of
your own. Account details and paths are in `AGENTS.local.md`.

- The CLI starts the daemon if it is not running. To only look, run
  `ossteam --no-autostart status`.
- Don't restart the daemon casually. A failed resume once lost the
  account's token (fixed in e4019cc). If the account is logged out, only
  the maintainer can sign it in again.
- Ask before anything other people can see on the account: cloud writes,
  workshop subscriptions, friend requests.
- The official client's logs and `userdata/` on the maintainer's machine
  belong to their personal account. Read them for structure only; never
  copy values into the repository.
- Ask before changing the desktop's settings (e.g. its colour scheme).
