# ossteam — Specification

| | |
|---|---|
| Status | Draft 1 |
| Date | 2026-10-17 |
| License | GPL-2.0-or-later |
| Scope | Whole project; subsystem detail lives in `docs/spec/` |

ossteam is an open-source Steam client written in Rust. A single per-user
daemon, `ossteamd`, owns the Steam session, the library and all game
processes. Two thin leaf clients drive it: `ossteam`, a CLI built on clap, and
`ossteam-gui`, a GUI built on `ossteam-ui`, our own GUI toolkit
(ADR 0004).

The key words MUST, MUST NOT, SHOULD, SHOULD NOT and MAY are used as in
RFC 2119. Requirements carry an ID (`[AREA-n]`) so issues, ADRs and tests can
refer to them.

---

## 1. Goals and scope

### 1.1 Goals

1. A complete Steam client: log in, see and manage the library, install,
   update, verify and uninstall games, and launch them so that they work as
   they do under the official client.
2. **Steam DRM and VAC are explicit goals.** Both are implemented faithfully.
   ossteam provides exactly what Valve's protection systems expect from a
   client and never bypasses, weakens, spoofs or modifies their intended
   behaviour (§13, §14). Neither is in M1.
3. Linux, Windows and macOS are all first-class platforms (§2.4).
4. One daemon and many thin clients: anything the GUI can do, the CLI can do,
   and so can any third-party client that speaks the IPC protocol (§8).
5. Free software: GPL-2.0-or-later, with a clean-room-style reverse
   engineering process (§3).

### 1.2 Non-goals

These are out of scope, either permanently or until a later spec revision
adds them:

- Store purchasing, wallet and gifting. Store and community pages open in the
  system browser.
- Remote Play, broadcasting, Big Picture mode and Steam Input remapping.
- Acting as a Steam Guard mobile authenticator.
- Anything that misrepresents what happened. That includes playtime or "idle"
  sessions with no real game process, setting achievements or stats from
  outside the game, ownership or DLC spoofing, and trading or market
  automation.
- Running Valve's own client binaries, such as `steamclient.so`,
  `steamservice` or `steamwebhelper`. ossteam reimplements the client. It
  does not wrap the official one.

---

## 2. Principles

These rules override convenience. A PR that breaks one of them is rejected
even if it works.

### 2.1 Faithful, never faked

- **[FAITH-1]** Every answer given to a user or a game comes from the real
  Steam session, from real server data, or from real local state that was
  derived from those.
- **[FAITH-2]** Unimplemented means honest failure. When something is not
  implemented, ossteam reports the truthful state of *this client*. Examples:
  `IsCloudEnabledForApp` returns `false` until Steam Cloud ships, and an
  unsupported operation returns the API's documented failure value. ossteam
  never returns a success it did not achieve.
- **[FAITH-3]** ossteam never fabricates licenses, tickets, stats,
  achievements, playtime, presence or protection results.
- **[FAITH-4]** Test doubles, such as a fake CM server or recorded sessions,
  exist only in test code. They MUST NOT be compiled into release binaries
  (`#[cfg(test)]` or `dev-dependencies` only).

### 2.2 Thin leaf clients

- **[LEAF-1]** `ossteam` (CLI) and `ossteam-gui` hold no authoritative state.
  They talk to nothing except the daemon. They never open network
  connections, touch library folders, read credentials or start games.
- **[LEAF-2]** Leaf clients depend on `ossteam-ipc` and on their UI framework
  only. CI enforces this by rejecting any path from `ossteam-cli` or
  `ossteam-gui` to a Steam-layer crate in `cargo tree` (§5.2).
- **[LEAF-3]** The CLI and GUI have feature parity at the IPC level. A
  presentation-only feature, such as artwork, is fine. A capability that only
  one client has is not. During M1 the GUI is built last (M1e), so parity
  is required from M1e onward.

### 2.3 Synchronous first

- **[SYNC-1]** Synchronous code is the default: blocking I/O, OS threads,
  channels and explicit cancellation.
- **[SYNC-2]** Async code is used only where it is clearly the better fit, and
  each case is recorded in an ADR. §6 lists the cases this spec already
  accepts.

### 2.4 Cross-platform

- **[PLAT-1]** Every crate builds and passes tests on Linux (x86_64, aarch64),
  Windows (x86_64) and macOS (aarch64, x86_64) in CI. Platform-specific code
  sits behind `cfg(target_os)` modules that share one interface.
- **[PLAT-2]** Compatibility tools, meaning Proton and the Steam Linux
  Runtime, exist only on Linux. That is the only intentionally
  platform-exclusive feature.

### 2.5 Platform-standard directories

- **[DIRS-1]** ossteam keeps all of its files in the platform-standard
  locations: XDG base directories on Linux, Known Folders on Windows, and
  `~/Library/...` on macOS (§17).
- **[DIRS-2]** The only exceptions are the minimal ABI discovery entries that
  Valve's Steamworks API needs to find a running client (§12.5). The daemon
  creates them while it runs and removes them when it exits. On Linux, the
  private `~/.steam` a launched game sees (DISC-4) is a directory under the
  data directory, mounted only in the game's own namespace; it creates
  nothing outside the standard directories but an empty `~/.steam` to mount
  it on.

### 2.6 Spec-first reverse engineering

- **[RE-1]** Reverse engineering is allowed for interoperability. Its results
  are written up in our own words under `docs/spec/` before or together with
  the code that uses them.
- **[RE-2]** No Valve code, binaries or copyrighted assets are copied into the
  repository. The one exception is the community-maintained protobuf
  definitions described in §3.3.

---

## 3. Legal and licensing

### 3.1 License

- **[LIC-1]** All ossteam code is licensed GPL-2.0-or-later. Every crate sets
  `license = "GPL-2.0-or-later"`, and every source file starts with
  `// SPDX-License-Identifier: GPL-2.0-or-later`. The repository root holds
  `COPYING` with the GPLv2 text.
- **[LIC-2]** Contributions are accepted under the same license (inbound =
  outbound) with a DCO `Signed-off-by`. There is no CLA.

### 3.2 Dependency licenses

- **[LIC-3]** `cargo-deny` enforces an allow-list: MIT, Apache-2.0,
  BSD-2-Clause, BSD-3-Clause, ISC, 0BSD, CC0-1.0, Zlib, BSL-1.0, Unicode-3.0, MPL-2.0, and any
  `GPL-2.0-or-later`/`GPL-3.0-or-later`/`LGPL-*-or-later` crate. GPL-2.0-only
  dependencies are denied, because they cannot be combined with Apache-2.0
  code.
- **[LIC-4]** winit and much of the Rust ecosystem are licensed Apache-2.0.
  Apache-2.0 is compatible with GPLv3 but not with GPLv2. Our source is still
  GPL-2.0-or-later. Binaries that link Apache-2.0 code are therefore conveyed
  under the terms of GPL-3.0-or-later. `README` and the release notes say this
  plainly.

### 3.3 Steam protocol definitions

- **[LIC-5]** `ossteam-proto` uses the community-maintained Steam protobuf
  definitions from the SteamDatabase `Protobufs` repository. ossteam does not
  hand-write its own.
- **[LIC-6]** The files ossteam needs are vendored unmodified into
  `crates/ossteam-proto/protos/`, from a pinned upstream commit.
  `cargo xtask update-protos <rev>` refreshes them, and the commit hash,
  upstream URL and file list are recorded in `crates/ossteam-proto/PROVENANCE.md`.
  Vendored files are exempt from the SPDX header check (LIC-1) and keep
  whatever notices they carry upstream.
- **[LIC-7]** Any local fix goes upstream first. Until it lands, the fix lives
  as a patch in `crates/ossteam-proto/patches/` that `update-protos` applies,
  so the vendored tree always stays identical to upstream plus explicit
  patches.

### 3.4 Trademarks and affiliation

- **[LIC-8]** ossteam is not affiliated with or endorsed by Valve. "Steam" is
  used only to describe compatibility. The project uses no Valve logos or
  artwork. Game artwork is fetched at runtime from Valve's CDN for the
  logged-in user, exactly as the official client does, and is never
  redistributed.

---

## 4. Architecture

```
            ┌──────────────┐   ┌──────────────┐   ┌──────────────────┐
            │ ossteam (CLI)│   │ ossteam-gui  │   │ 3rd-party client │
            │    clap      │   │  ossteam-ui  │   │                  │
            └──────┬───────┘   └──────┬───────┘   └────────┬─────────┘
                   │  client IPC (local socket / named pipe, §8)
                   └──────────────────┼─────────────────────┘
                              ┌───────┴────────┐
                              │    ossteamd     │  per-user daemon
                              │ core · session  │
                              │ pics · content  │
                              │ launch · stats  │
                              └─┬────────────┬──┘
                    game IPC    │            │   CM (WebSocket/TCP)
                     (§12.4)    │            │   WebAPI + CDN (HTTPS)
          ┌─────────────────────┴──┐       ┌─┴────────────────┐
          │ game process           │       │   Steam network  │
          │  steam_api (Valve) ──▶ │       └──────────────────┘
          │  ossteam-steamclient   │
          │  (our steamclient lib) │
          └────────────────────────┘
```

- **[ARCH-1]** Each OS user runs one daemon. It owns the CM connection, the
  credentials, the library database, the install jobs and the game processes.
- **[ARCH-2]** Leaf clients connect over the client IPC endpoint. Any number
  of them may be connected at once, and all of them see the same state.
- **[ARCH-3]** Games reach the daemon through `ossteam-steamclient`, our
  implementation of the library that Valve's `steam_api` loads. It uses a
  separate game IPC endpoint that exposes only the game-facing API (§12.4).
- **[ARCH-4]** The daemon runs with the user's own privileges. Any privileged
  helper that a later milestone needs, for example for VAC on Windows (§14),
  is a separate minimal process with its own spec page and security review.

---

## 5. Workspace

### 5.1 Crates

```
Cargo.toml                 workspace; edition 2024; pinned toolchain
crates/
  ossteam-types/           ids (AppId, DepotId, ManifestId, PackageId, SteamId), shared plain types; no I/O
  ossteam-vdf/             KeyValues text + binary; appinfo/packageinfo cache formats; .acf/.vdf
  ossteam-proto/           vendored community Steam .proto files (LIC-5..7) + prost codegen (build.rs)
  ossteam-net/             CM directory, transports (WebSocket, TCP+channel encryption), framing, EMsg, job ids, service methods
  ossteam-webapi/          HTTPS WebAPI client (ureq)
  ossteam-auth/            IAuthenticationService flows, Steam Guard, token lifecycle
  ossteam-pics/            licenses, PICS app/package info, change tracking, cache
  ossteam-content/         CDN directory, manifests, chunks, depot keys, install engine
  ossteam-launch/          launch-option resolution, compat-tool chains, process supervision, discovery entries
  ossteam-stats/           user stats/achievements schema + storage protocol
  ossteam-gameipc/         game IPC protocol (daemon ↔ steamclient shim)
  ossteam-steamclient/     cdylib: steamclient ABI (CreateInterface, vtables, callbacks)
  ossteam-ipc/             client IPC protocol + blocking client library
  ossteamd/                daemon binary
  ossteam-cli/             binary `ossteam` (clap)
  ossteam-dbus/            a minimal blocking D-Bus client, for the desktop portal (ADR 0004)
  ossteam-ui/              our GUI toolkit: window, layout, text, Vulkan and software renderers, focus (ADR 0004)
  ossteam-gui/             binary `ossteam-gui` (ossteam-ui); src/widgets/
xtask/                     codegen, packaging, license and layering checks
docs/spec/                 subsystem specs (RE notes in our own words)
docs/adr/                  architecture decision records
docs/acceptance/           per-milestone acceptance corpora and checklists
```

### 5.2 Layering rules

- **[WS-1]** Dependencies flow downward only: `types → vdf/proto → net/webapi
  → auth/pics/content/stats/launch → ossteamd`. Leaf clients depend only on
  `ossteam-ipc` and `ossteam-types`, and the GUI also on `ossteam-ui`.
  `ossteam-ui` depends only on `ossteam-dbus`, which depends on no other
  workspace crate.
- **[WS-2]** `ossteam-steamclient` depends only on `ossteam-gameipc` and
  `ossteam-types`, plus `std`, and keeps third-party dependencies to a
  minimum. It is built for i686 and x86_64 on Linux and Windows, and as a
  universal binary on macOS.
- **[WS-3]** `cargo xtask check-layering` enforces WS-1 and WS-2 in CI.

### 5.3 Toolchain and key dependencies

| Concern | Choice | Notes |
|---|---|---|
| Toolchain | Pinned in `rust-toolchain.toml` | Currently 1.98.1; bumped deliberately, as a change of its own |
| GUI | `ossteam-ui`, ours (ADR 0004), on `winit`, `ash` (Vulkan; MoltenVK on macOS), `taffy`, `cosmic-text` and `swash`; our own software renderer, presented with `softbuffer` | No third-party GUI framework or widget kit |
| CLI | `clap` v4 (derive), `clap_complete` | |
| Protobuf | `prost`, `prost-build` | Used for both Steam messages and our IPC |
| WebSocket | `tungstenite` (non-blocking, ADR 0001) + `rustls` | rustls uses the `ring` provider and `rustls-platform-verifier` (SEC-3) |
| HTTP | `ureq` + `rustls` | Same TLS configuration as WebSocket |
| Crypto | RustCrypto (`aes`, `rsa`, `sha1`, `sha2`, `hmac`, `crc32fast`) | |
| Compression | `lzma-rs` or `xz2`, `zstd`, `flate2` | Covers every chunk and multi-message format |
| CM I/O | `polling`, `socket2` | Synchronous readiness polling and non-blocking connect (ADR 0001) |
| Local IPC | `interprocess` (local sockets, sync) | Unix sockets / Windows named pipes |
| Storage | `rusqlite` (bundled) | Daemon state database |
| Secrets | `keyring` | Secret Service, Keychain, Credential Manager |
| Dirs | `directories` | |
| Channels | `crossbeam-channel` | |
| Logging | `tracing`, `tracing-subscriber`, `tracing-appender` | |
| Errors | `thiserror` in libraries, `anyhow` only in binaries | |

Adding an async runtime (tokio, async-std, smol) to the daemon requires an ADR
(§6).

---

## 6. Concurrency model

- **[CONC-1]** The daemon is synchronous. Its structure:
  - **core**: one thread that owns all mutable state. It receives `Command`s
    on a `crossbeam-channel` and publishes `Event`s to subscribers. Subsystems
    never share mutable state directly.
  - **cm**: one I/O thread per CM connection, which waits on a readiness
    poller over a non-blocking socket (ADR 0001). Responses are matched to
    requests by job id. A caller waits on a one-shot channel that has a
    timeout.
  - **ipc**: one acceptor thread per endpoint and one thread per connected
    client. Few clients connect, so a thread each is cheap.
  - **content**: the core schedules jobs. Each running job has one thread
    that drives it and a bounded pool of worker threads (default 8,
    configurable up to 64) for chunk download, decryption, decompression
    and writes. At most `content.concurrent_jobs` jobs (default 1, up to 8)
    run at once.
  - **launch**: one supervisor thread per running game. It waits on the
    process and reports its exit.
- **[CONC-2]** Cancellation is cooperative. Cloneable `CancelToken`s
  (a shared flag) are combined with socket read timeouts of at most 1 s,
  so every blocking loop notices cancellation within that bound. A loop
  that waits while the daemon is idle registers a wake-up on the token
  instead (`CancelToken::on_cancel`) and waits without a timeout, so that
  the slice does not spend the §20 wakeup budget (ADR 0001).
- **[CONC-3]** Async is accepted only in a dependency with no reasonable
  sync alternative, wrapped behind a synchronous facade so the async code
  stays in one place, and nowhere else without an ADR. No part of
  ossteam uses it today. The GUI is synchronous: its event loop waits for
  input, for messages from its IPC threads, and for timers, and it calls
  the desktop portal through its own blocking D-Bus client (ADR 0004).
- **[CONC-4]** Before async is considered anywhere else, the ADR must show a
  measured problem with the threaded design, such as thousands of concurrent
  waits or a missed latency budget from §20.

---

## 7. Daemon

### 7.1 Lifecycle

- **[D-1]** There is one daemon per user. A lock file in the runtime directory
  enforces this. When a second instance starts, it exits with status 0 if the
  first one is healthy.
- **[D-2]** Leaf clients start the daemon if it is not running. Passing
  `--no-autostart` turns that off. The daemon detaches from the client that
  started it.
- **[D-3]** Optional integration files are provided for start-at-login: a
  systemd user unit, a launchd agent and a Windows scheduled task.
- **[D-4]** By default the daemon keeps running until `ossteam daemon stop`.
  With `daemon.exit_when_idle = true` it exits when no client is connected,
  no job is running and no game is running.
- **[D-5]** Shutdown is graceful. The daemon checkpoints jobs, sends logoff,
  removes the ABI discovery entries (§12.5) and closes its sockets. Running
  games are not killed. If games are still running, the daemon refuses to
  stop unless `--force` is given, because a running game depends on it.

### 7.2 State and persistence

- **[D-6]** One SQLite database holds accounts (without secrets), library
  folders, installed apps (app id, build id, branch, depots and manifest ids,
  install directory, size), the job journal and settings overrides.
- **[D-7]** The PICS cache uses the binary KeyValues format, stored in the
  cache directory, and is keyed by change number. It can be rebuilt at any
  time.
- **[D-8]** Refresh tokens are stored in the OS keyring. If no keyring is
  available, the daemon stores tokens in a mode-0600 file only after the user
  explicitly opts in. Passwords are never stored.
- **[D-9]** Configuration is a TOML file (`ossteamd.toml`) in the config
  directory, and IPC can override individual settings. Unknown keys produce a
  warning, not an error.

### 7.3 Logging

- **[D-10]** Logs are written with `tracing` to the state directory, rotated
  daily, and kept for 7 days. Tokens, passwords, Steam Guard codes, depot
  keys and ticket bytes are redacted when the value is constructed, using
  types that have no `Debug`/`Display` output for their content.

---

## 8. Client IPC protocol

### 8.1 Transport

- **[IPC-1]** The endpoints are:
  - Linux: Unix socket at `$XDG_RUNTIME_DIR/ossteam/daemon.sock`, in a
    directory with mode 0700 and a socket with mode 0600.
  - macOS: Unix socket in a per-user directory under `$TMPDIR`.
  - Windows: named pipe `\\.\pipe\ossteam-<user SID>`, with a DACL that
    grants access to that SID only.
- **[IPC-2]** The daemon checks every peer's credentials: `SO_PEERCRED` on
  Linux, `getpeereid` on macOS, and the client token user SID on Windows. A
  peer belonging to another user is rejected.
- **[IPC-3]** Nothing listens on TCP. Remote control is a non-goal.

### 8.2 Framing and schema

- **[IPC-4]** Each frame is a little-endian `u32` length followed by a
  protobuf `Envelope`. Frames are at most 16 MiB.
- **[IPC-5]** The schema is defined in `crates/ossteam-ipc/proto/ossteam/ipc/v1/*.proto`
  and forms the public API for third-party clients:

  ```
  Envelope { uint64 id; oneof body { Hello, Request, Response, Event, Error } }
  ```

- **[IPC-6]** Every connection starts with a `Hello` that carries the
  protocol version, the client kind and the client version. The daemon
  rejects incompatible major versions with an `Error` that names the
  supported range. Minor versions only add fields or methods.
- **[IPC-7]** Requests and responses are matched by `id`. Events have
  `id = 0`. A client receives events only for the topics it subscribed to.
- **[IPC-8]** Errors carry a stable `code`, a human-readable `message` and an
  optional `detail`. The codes map to the CLI exit codes (§15.3).

### 8.3 Methods (M1)

| Area | Methods |
|---|---|
| Daemon | `GetStatus`, `GetVersion`, `Shutdown{force}` |
| Session | `LoginWithCredentials{username, password, remember}`, `LoginWithQr`, `SubmitGuardCode{code}`, `CancelLogin`, `Logout`, `ListAccounts`, `SwitchAccount{steam_id}`, `SetPersonaState{online\|invisible\|offline}` |
| Library | `ListApps{filter, installed_only}`, `GetApp{app_id}`, `GetAsset{app_id, kind}` → path of a cached file |
| Folders | `ListLibraryFolders`, `AddLibraryFolder{path}`, `RemoveLibraryFolder{path}` |
| Jobs | `Install{app_id, folder, branch}`, `Update{app_ids \| all}`, `Verify{app_id}`, `Uninstall{app_id, remove_compat_data}`, `ListJobs`, `PauseJob`, `ResumeJob`, `CancelJob`, `SetJobPriority` |
| Launch | `GetLaunchOptions{app_id}`, `Launch{app_id, option, extra_args}`, `ListRunning`, `StopGame{app_id}` |
| Settings | `GetSettings`, `SetSetting{key, value, reset}` |
| Events | `Subscribe{topics}`, `Unsubscribe{topics}` |

### 8.4 Events

`SessionChanged`, `LoginPrompt{kind: email_code | device_code |
device_confirmation | qr_url}`, `AppChanged`, `JobProgress` (at most 10 Hz
per job), `JobStateChanged`, `GameStarted`, `GameExited{status}`,
`ShimReport{app_id, unimplemented_calls}` (§12.6), `Notice{level, text}`,
`SettingChanged{setting}`.

### 8.5 Interactive login

- **[IPC-9]** Login is driven by the daemon. The client starts a login. The
  daemon emits `LoginPrompt` events whenever Steam needs something, such as a
  Steam Guard code or a QR URL to render. The client answers with
  `SubmitGuardCode`. Device confirmation completes on its own once the user
  approves in the Steam mobile app.
- **[IPC-10]** A password reaches the daemon only through the IPC request
  that carries it. Leaf clients read it from a TTY prompt or a GUI password
  field, never from argv or the environment, and zero the buffer after
  sending.

---

## 9. Steam network layer

Details are in `docs/spec/cm.md`, `auth.md`, `pics.md` and `content.md`.

### 9.1 Connection manager (CM)

- **[NET-1]** The CM server list comes from the Steam Directory WebAPI and is
  cached. Servers are tried in the order the directory returns them, and
  failures back off exponentially.
- **[NET-2]** WebSocket over TLS is the primary transport. TCP with Steam
  channel encryption is the fallback. Both sit behind one `Transport` trait.
- **[NET-3]** The framing layer handles EMsg headers, both protobuf and
  extended headers, `Multi` messages (gzip), heartbeats at the interval the
  server sets, and unified service method calls, both authenticated and
  unauthenticated.
- **[NET-4]** When the connection drops, the daemon reconnects with backoff
  and re-logs on with the stored refresh token. While disconnected, IPC
  status reports `connecting`, and the game API reports logged-off
  truthfully.

### 9.2 Authentication

- **[AUTH-1]** Credential login uses `IAuthenticationService`. The password
  is RSA-encrypted with the key returned for that account, and the flow
  covers session polling, email codes, device codes and device confirmation.
- **[AUTH-2]** QR login uses the same service through the QR begin-session
  flow. The daemon sends the challenge URL to clients, and the GUI renders it
  as a QR code. The CLI prints it as a QR code made of terminal block
  characters.
- **[AUTH-3]** Logon to the CM uses the access token. Refresh tokens are
  persisted (D-8). The daemon renews the access token before it expires and
  handles revocation by asking the user to log in again.
- **[AUTH-4]** One account is active per daemon. Several accounts can keep
  remembered tokens, and switching is a logoff followed by a logon.

### 9.3 Licenses and product info

- **[PICS-1]** Licenses come from the server-sent license list.
- **[PICS-2]** App and package info comes from PICS. Product info requests
  use access tokens wherever they are required. A changes-since poll
  (default every 60 s while connected) keeps the cache current and triggers
  update detection.
- **[PICS-3]** The library shows exactly the apps that the account's licenses
  grant, with DLC grouped under its parent app.

### 9.4 Stats, presence and play sessions

- **[PLAY-1]** While a game process launched through ossteam is running, the
  daemon reports it as being played through the same CM message the official
  client uses. When the process exits, the daemon clears that report. This
  is the only source of playtime and in-game presence.
- **[PLAY-2]** Persona state is chosen by the user: online, invisible or
  offline. The friends list and chat are planned for M3.

---

## 10. Content and install engine

- **[CONT-1]** CDN servers come from the content server directory service.
  Manifest request codes are fetched for each depot and manifest, and depot
  decryption keys come over CM. The account only obtains keys for depots it
  is entitled to, so entitlement is enforced by Valve's servers, as it should
  be.
- **[CONT-2]** Depots are selected exactly as the app's depot configuration
  declares: OS, architecture, language, low-violence, DLC ownership and
  shared installs. Depots for which no key is granted are not offered.
- **[CONT-3]** The engine parses manifests, which may be compressed and
  encrypted, and decrypts file names when needed. Chunks are AES-256
  decrypted, decompressed with whichever format the chunk declares
  (VZip/LZMA, zstd, zip), and checked against their SHA-1 before they are
  written.
- **[CONT-4]** Updates are delta updates. Chunks that are unchanged between
  the old and new manifests are copied locally, and only new chunks are
  downloaded. Files are written to staging and moved into place once a whole
  file is complete. The job journal records completed chunks so that a job
  resumes after a crash or restart.
- **[CONT-5]** Verify re-hashes installed files against the manifest and
  repairs any mismatch by downloading the affected chunks again.
- **[CONT-6]** Installs land in `<library>/common/<installdir>`, using the
  `installdir` from app info, and disk space is checked before the job
  starts.
- **[CONT-7]** The bandwidth limit, the number of concurrent jobs (default
  1) and the auto-update policy are all settings.
- **[CONT-8]** Install scripts (`installscript.vdf`) follow what the official
  client does. They run on native Windows, with elevation through UAC only
  when a script needs it. Under Proton they run inside the prefix.
  `docs/spec/install-scripts.md` documents the format and the order of
  operations.
- **[CONT-9]** Branches: the public branch and branches without a password
  are supported in M1. Password-protected betas are planned for M3.

---

## 11. Launching

### 11.1 Resolving launch options

- **[LAUNCH-1]** Launch options come from the app info `config/launch`
  entries. They are filtered by OS, architecture and beta key, and their type
  is honoured. When more than one entry qualifies, the leaf client asks the
  user to choose, as the official client does.
- **[LAUNCH-2]** The environment the official client provides is reproduced
  and documented in `docs/spec/launch-env.md`. It includes `SteamAppId`,
  `SteamGameId` and the other variables that spec page establishes.
- **[LAUNCH-3]** Before launching, the daemon checks that the account owns
  the app, that the app is installed, that it is up to date (or that the user
  chose to launch the stale build), and that the ABI discovery entries are in
  place. If the executable is protected by Steam DRM (§13.2), the daemon
  refuses honestly in M1.

### 11.2 Compatibility tools (Linux)

- **[COMPAT-1]** Proton and the Steam Linux Runtime are installed as Steam
  apps through the normal install engine. ossteam never bundles them.
- **[COMPAT-2]** The tool used for each app comes from Valve's published
  compat-tool mapping, which the user can override. The launch chain
  (runtime → Proton → game) is built from each tool's `toolmanifest.vdf`,
  including `require_tool_appid` and `commandline`.
- **[COMPAT-3]** The variables Proton expects (`STEAM_COMPAT_DATA_PATH`,
  `STEAM_COMPAT_CLIENT_INSTALL_PATH` and others) are documented in
  `docs/spec/compat-tools.md`. If Proton expects files from a Steam client
  installation, the daemon generates a minimal client-install directory
  inside its data directory that contains only those files. Prefixes live in
  `<library>/compatdata/<appid>`.

### 11.3 Supervision

- **[LAUNCH-4]** Games run as children of the daemon, in their own process
  group or job object. The whole process tree is tracked so that launcher
  stubs which exit early do not end the play session before the game does.
- **[LAUNCH-5]** `StopGame` asks the game to close politely first and
  force-terminates it only after a timeout, which mirrors the official
  client's Stop button.

---

## 12. Steamworks client ABI (`ossteam-steamclient`)

Details are in `docs/spec/steamworks-abi.md`, with one page per interface.

### 12.1 Purpose

Games link Valve's `steam_api` library, which ships with the game. That
library locates and loads a client library (`steamclient.so`,
`steamclient.dll`/`steamclient64.dll` or `steamclient.dylib`) and talks to
it through versioned C++ interfaces. `ossteam-steamclient` is our
implementation of that library.

### 12.2 ABI rules

- **[ABI-1]** The library exports `CreateInterface` and the callback-pump
  entry points that `steam_api` calls (`Steam_BGetCallback`,
  `Steam_FreeLastCallback`, `Steam_GetAPICallResult` and the others the ABI
  spec page establishes).
- **[ABI-2]** Interfaces are `#[repr(C)]` vtables of `extern "C"` functions,
  or `extern "thiscall"` functions on 32-bit Windows. Every slot is filled.
  The supported interface version strings are listed in a table generated
  from `docs/spec/steamworks-abi.md`. An unknown version makes
  `CreateInterface` return null and logs the request (FAITH-2).
- **[ABI-3]** Every unimplemented method returns the API's documented failure
  value, or `k_uAPICallInvalid` for call-result APIs. It never returns
  success.
- **[ABI-4]** No exported function lets a Rust panic cross the FFI boundary.
  A panic is caught, logged, and turned into the failure value.

### 12.3 M1 interface scope

| Interface | M1 scope |
|---|---|
| ISteamClient | Pipes, users and getters for every interface listed here |
| ISteamUser | `BLoggedOn` and `GetSteamID`; auth session tickets via app ownership tickets, game-connect tokens and the auth ticket list; encrypted app tickets |
| ISteamApps | Ownership, DLC (`BIsDlcInstalled`, `BIsSubscribedApp`, DLC enumeration), current beta name, build id, install directory, game language |
| ISteamUserStats | Stats and achievements: request current stats, get and set, `StoreStats`, schema and icons, global achievement percentages |
| ISteamUtils | App id, IPC country, server time, image data, callback and API-call plumbing, overlay notification position (accepted and recorded; no overlay exists) |
| ISteamFriends | Own persona name and state. Everything else returns its failure value until M3. |
| ISteamRemoteStorage | `IsCloudEnabledFor*` returns `false` (truthful). File APIs write and read locally under the cloud root that the app's config defines, and are documented as not synced until M3. |

Other interfaces, such as Networking, Matchmaking, UGC, Input and HTMLSurface,
are reachable through their getters and fail truthfully under ABI-3.

### 12.4 Game IPC

- **[GIPC-1]** The game endpoint is separate from the client endpoint and uses
  the same socket conventions (IPC-1, IPC-2), but its schema is restricted.
  A game can never log out, install, uninstall, read tokens or affect any app
  other than its own.
- **[GIPC-2]** The daemon identifies the game by its PID, which it supervises
  (LAUNCH-4), and by the `SteamAppId` it handed out. A game that was not
  launched by ossteam can connect, as games do under the official client, but
  receives only what Steam itself would grant for the app id it claims:
  ownership is checked against the account's licenses.
- **[GIPC-3]** Callbacks and API-call results are produced by the daemon and
  queued in the shim. The shim hands them to the game only when the game
  pumps callbacks, which preserves Steamworks' synchronous dispatch model.

### 12.5 ABI discovery entries (the one exception to DIRS-1)

- **[DISC-1]** While the daemon runs and an account is logged on, it creates
  only the entries `steam_api` needs in order to find a client:
  - Linux: `~/.steam/sdk32/steamclient.so` and `~/.steam/sdk64/steamclient.so`
    as symlinks to our libraries, plus the running-client marker that the ABI
    spec page establishes, while those locations are free (no other client's
    entries are there).
  - Windows: `HKCU\Software\Valve\Steam\ActiveProcess`, with the client DLL
    paths and the PID.
  - macOS: the Mach service `steam_api` looks up, serving the path of a copy
    of our library and the PID (`steamworks-abi.md` §1.3).
- **[DISC-2]** The daemon removes the entries when it exits and repairs them
  on its next start after a crash.
- **[DISC-3]** If these locations already belong to an official Steam
  installation, the daemon does not touch them, except as DISC-5 allows on
  Linux. Launching games then fails with error `conflict` and a message that
  explains why, unless the launch finds ossteam another way (DISC-4).
  Coexistence is documented, and ossteam never silently overwrites Valve's
  entries.
- **[DISC-4]** On Linux, every process the daemon starts for a launch runs in
  a user and mount namespace of its own in which a `~/.steam` of ossteam's
  own, holding only the entries `steam_api` needs, is mounted over the real
  one. Launched games therefore find ossteam whether or not an official
  client owns the real `~/.steam`, and the official client and everything it
  starts are unaffected. In the same namespace, an official installation's
  `userdata/` shows ossteam's own user data and its `config/` is empty, so
  that games cannot read the official client's accounts. When the system
  does not allow such a namespace, launches use the real entries of DISC-1.
- **[DISC-5]** On Linux, an off-by-default setting lets the daemon take over
  an official client's entries while that client is not running, hand them
  back as they were when it is done, and yield them at once when the
  official client starts. The setting states its risks (`steamworks-abi.md`
  §1.1.5) to the user before it is turned on.
- **[DISC-6]** When the system does not allow DISC-4's namespace, the user is
  told so, with the reason and what it means for launches: a warning line on
  the daemon's standard error at start, the daemon's status, a warning line
  from the CLI's `status` and `launch`, and the GUI, not only in a log. The
  same places warn while DISC-5's setting is on.

### 12.6 Visibility

- **[ABI-5]** Each play session produces a `ShimReport`: the interfaces
  requested, the unknown versions, and the unimplemented methods with their
  call counts. The GUI shows it on the app page and the CLI prints it after
  the game exits, so gaps are visible and never hidden.

---

## 13. Steam DRM (M2)

Details are in `docs/spec/drm.md`, which must exist and be reviewed before any
DRM code lands.

### 13.1 Requirements

- **[DRM-1]** ossteam implements its client-side part of Steam DRM as
  designed. When the protection asks the client a question, the answer is
  true for this user and this license.
- **[DRM-2]** ossteam MUST NOT unpack, strip, patch, emulate around or
  otherwise defeat protection. It never alters a protected binary on disk or
  in memory. Every decryption step is performed by the protection's own code.
- **[DRM-3]** An account without a license MUST see the same failure the
  official client produces. This is checked by differential testing (§19.3).

### 13.2 Scope

- **SteamStub** (the executable wrapper): ossteam detects protected
  executables at launch. In M1, detection leads to an honest refusal with
  error `unsupported` and a message that DRM support arrives in M2. In M2,
  the shim and the daemon serve the stub's checks faithfully.
- **CEG** (Custom Executable Generation): ossteam obtains user-specific
  executable material from Valve's servers through the same interactions as
  the official client, and installs and verifies it as specified.
- Third-party DRM, such as an anti-tamper layer that a game embeds, is the
  game's own concern. ossteam provides a faithful client environment and
  never interferes with it.

---

## 14. VAC (M2)

Details are in `docs/spec/vac.md`, which must exist and be reviewed before any
VAC code lands.

- **[VAC-1]** ossteam hosts VAC modules exactly as Valve delivers them. It
  verifies them the way the official client does, gives them the privileges,
  inputs and process environment they expect, and passes their results back
  unchanged.
- **[VAC-2]** ossteam MUST NOT filter, sandbox away, delay, spoof or inspect
  in order to influence what VAC observes. It MUST NOT offer any setting that
  weakens VAC.
- **[VAC-3]** If VAC cannot be hosted faithfully on a platform, ossteam
  reports VAC as unavailable. VAC-secured play then fails as it would with a
  VAC failure. ossteam never claims a VAC state it did not achieve.
- **[VAC-4]** ossteam itself never injects into, hooks or reads the memory
  of game processes. Any later feature that would, such as an overlay, needs
  an ADR that analyses its VAC impact.
- **[VAC-5]** VAC hosting ships behind a build-time feature until the M2
  acceptance criteria pass, so users are never exposed to an unvalidated
  implementation. While the feature is off, VAC-3 applies.
- **[VAC-6]** If Windows VAC needs an elevated component (ARCH-4), that
  component is minimal, speaks a narrow typed protocol to the daemon, and is
  reviewed separately.

---

## 15. CLI (`ossteam`, clap)

### 15.1 Conventions

- **[CLI-1]** Commands use clap derive and follow `noun verb` where it reads
  naturally, with short top-level forms for the common verbs. `<APP>` accepts
  an app id or an exact name from the account's library. An ambiguous name
  fails and lists the candidates.
- **[CLI-2]** Output is human-readable by default. `--json` emits one JSON
  document, and streaming commands with `--json` emit NDJSON. The JSON shapes
  are versioned and documented, and are stable for scripting.
- **[CLI-3]** Progress output goes to stderr and only when stderr is a TTY.
  Results go to stdout.
- **[CLI-4]** Global flags: `--json`, `--quiet`, `--no-autostart`,
  `--color <auto|always|never>`, `-v/--verbose`.
- **[CLI-5]** Shell completions come from `ossteam completions <shell>`, and
  man pages are generated at build time with `clap_mangen`.

### 15.2 Commands (M1)

```
ossteam login [--qr] [--username <NAME>] [--no-remember]
ossteam logout
ossteam account list | switch <STEAMID|NAME>
ossteam status                                  # daemon, connection, account, jobs, running games
ossteam persona <online|invisible|offline>

ossteam library [--installed] [--filter <TEXT>]
ossteam info <APP>                              # app info, depots, branches, install state, launch options
ossteam install <APP> [--folder <PATH>] [--branch <NAME>] [--wait]
ossteam update [<APP>... | --all] [--wait]
ossteam verify <APP> [--wait]
ossteam uninstall <APP> [--yes] [--remove-compat-data] [--wait]
ossteam jobs [list | pause <ID> | resume <ID> | cancel <ID> | priority <ID> <N> | watch]

ossteam folders [list | add <PATH> | remove <PATH>]
ossteam launch <APP> [--option <N>] [--wait] [-- <ARGS>...]
ossteam running | stop <APP>

ossteam config [list | get <KEY> | set <KEY> <VALUE> | unset <KEY>]
ossteam events [--topic <T>]...                 # follow the event stream
ossteam daemon [start | stop [--force] | status]
ossteam completions <SHELL>
```

`login` prompts for the password and for any Steam Guard code on the TTY. It
refuses to run without a TTY unless `--qr` is given.

### 15.3 Exit codes

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Other error |
| 2 | Usage error (clap) |
| 3 | Daemon unavailable |
| 4 | Not logged in, or authentication failed |
| 5 | Not found |
| 6 | Not owned or not entitled |
| 7 | Unsupported (honest "not implemented", FAITH-2) |
| 8 | Conflict (for example DISC-3, or a job already running) |
| 9 | Network or Steam unavailable |
| 130 | Interrupted |

---

## 16. GUI (`ossteam-gui`, `ossteam-ui`)

### 16.1 Rules

- **[GUI-1]** The GUI is built on `ossteam-ui`, our own toolkit in
  `crates/ossteam-ui/` (ADR 0004). The GUI's own widgets live in
  `crates/ossteam-gui/src/widgets.rs`. No third-party GUI framework or
  widget kit is used.
- **[GUI-2]** The GUI renders state that came from the daemon. It keeps no
  model of its own beyond view state. All mutations happen through IPC, and
  an optimistic update is allowed only when the event that confirms it
  replaces it.
- **[GUI-3]** There is no embedded web view. Store, community and support
  links open in the system browser.
- **[GUI-4]** The GUI provides keyboard navigation for every action, visible
  focus, and system light and dark themes, and passes the DPI and scaling
  checks on every platform.
- **[GUI-5]** An idle window does not wake. The GUI draws only when input,
  a daemon event, a call's result, a timer or a change of scale or
  appearance changed something.

### 16.2 Views (M1)

| View | Contents |
|---|---|
| Login | Credentials form, QR tab, Steam Guard prompt, account picker for remembered accounts |
| Library | Virtualised grid and list (`uniform_list`), search, filters (installed, has updates), capsule art via `GetAsset` |
| App | Header art, install and play state, Install/Play/Update/Stop, launch-option chooser, size and folder, branch, verify and uninstall, last ShimReport |
| Downloads | Queue, progress, rate, pause/resume/reorder, history |
| Settings | Library folders, bandwidth, concurrency, auto-update, compat-tool overrides (Linux), persona, daemon behaviour |
| Status bar | Connection and account state, active job summary, running game |

When the daemon is not running, the GUI starts it (D-2). When the daemon
cannot be reached, the GUI shows a blocking state that explains why instead
of empty views.

---

## 17. Filesystem layout

Directories are resolved with `directories::ProjectDirs`. `<id>` is the
application identifier (§24, Q2).

| Kind | Linux | Windows | macOS |
|---|---|---|---|
| Config | `$XDG_CONFIG_HOME/ossteam` | `%APPDATA%\ossteam\config` | `~/Library/Application Support/<id>` |
| Data (database, generated client-install dir) | `$XDG_DATA_HOME/ossteam` | `%LOCALAPPDATA%\ossteam\data` | `~/Library/Application Support/<id>` |
| Default library | `$XDG_DATA_HOME/ossteam/library` | `%LOCALAPPDATA%\ossteam\library` | `~/Library/Application Support/<id>/library` |
| Cache (PICS, assets, CDN list) | `$XDG_CACHE_HOME/ossteam` | `%LOCALAPPDATA%\ossteam\cache` | `~/Library/Caches/<id>` |
| State (logs) | `$XDG_STATE_HOME/ossteam` | `%LOCALAPPDATA%\ossteam\state` | `~/Library/Logs/<id>` |
| Runtime (sockets, lock) | `$XDG_RUNTIME_DIR/ossteam` | named pipes | per-user dir under `$TMPDIR` |
| ABI exception | `~/.steam/sdk32`, `~/.steam/sdk64`, running marker; the private `~/.steam` in `<data>/steam-sdk` (DISC-4) | `HKCU\Software\Valve\Steam\ActiveProcess` | per ABI spec |

Each library folder has this layout: `common/<installdir>`,
`compatdata/<appid>` (Linux), `.ossteam/staging/`, and
`.ossteam/library.toml`, which identifies the folder.

---

## 18. Security and privacy

- **[SEC-1]** The daemon never listens on the network, and the IPC endpoints
  are restricted to the user (IPC-1 to IPC-3).
- **[SEC-2]** Secrets have dedicated types that are zeroed on drop
  (`zeroize`) and never appear in logs (D-10).
- **[SEC-3]** All HTTPS and WSS connections use rustls with the platform
  verifier. Certificate checks cannot be disabled.
- **[SEC-4]** The game endpoint is restricted (GIPC-1) and treats every game
  as untrusted input. Message sizes, rates and the decoding of every field
  are checked.
- **[SEC-5]** The parsers for manifests, KeyValues, protobuf and chunks are
  fuzzed with `cargo-fuzz` in CI on a nightly schedule.
- **[SEC-6]** ossteam sends no telemetry. The only traffic is to Valve, to
  serve the user's own requests.

---

## 19. Testing and quality

### 19.1 Levels

1. **Unit tests** in every crate. Formats are tested against golden fixtures
   that we create ourselves, or that are scrubbed captures from our own test
   accounts.
2. **Protocol replay**: recorded CM and CDN sessions from test accounts,
   scrubbed of secrets, are replayed against `ossteam-net`, `ossteam-auth`,
   `ossteam-pics` and `ossteam-content` in CI without network access.
3. **Daemon integration tests** run a real daemon against a test-only fake
   CM and CDN (FAITH-4), driven through `ossteam-ipc`.
4. **ABI tests** build small C++ programs against the public Steamworks
   headers (when their license permits CI use; otherwise against our own
   declarations from the ABI spec) and load `ossteam-steamclient` through a
   test harness.
5. **Live tests**, gated on `OSSTEAM_LIVE=1` and test-account credentials in
   CI secrets, run on a schedule rather than on every PR.

### 19.2 CI

The build matrix covers every target in PLAT-1 and the 32-bit shim targets.
CI runs `cargo fmt --check`, `clippy -D warnings`, `cargo deny`,
`xtask check-layering` and an SPDX header check.

### 19.3 Differential testing against the official client

Where possible, behaviour is compared against the official client on the same
account and title. That covers the messages on the wire (ordering and
fields), the values the shim returns to a probe game, the failures for
unowned apps, and in M2 the DRM and VAC outcomes. Deviations are bugs unless
an ADR justifies them.

---

## 20. Performance budgets

These are measured on reference hardware defined in
`docs/acceptance/reference-hw.md`, with hardware GPU rendering.

| Metric | Budget |
|---|---|
| GUI cold start to first frame (daemon already running) | ≤ 500 ms |
| Library view with 10,000 apps | Scrolls at display refresh rate, with no frame over 16 ms |
| CLI command round trip (daemon running) | ≤ 50 ms p95 |
| Daemon idle RSS | ≤ 60 MB |
| Daemon idle CPU | ≈ 0, with no busy polling and wakeups ≤ 1/s |
| Download throughput | Saturates 1 Gbit/s on reference hardware with default settings |
| Shim callback-pump overhead | ≤ 5 µs per `RunCallbacks` call when no callbacks are pending |

---

## 21. Milestones

### M0: Foundations

- The workspace, CI matrix, license tooling, layering check and SPDX check.
- `ossteam-vdf` complete for text and binary KeyValues.
- The IPC skeleton: `Hello`, `GetStatus`, and `daemon start/stop/status`
  wired end to end through the CLI and a GUI window.
- GUI toolkit spike results recorded for every platform: window, DPI,
  clipboard, first-frame time.
- Spec pages written: `cm.md`, `auth.md`, `ipc.md`.

**Exit:** `ossteam daemon status` and an empty `ossteam-gui` window work on
all three OSes from CI-built artifacts.

### M1: A usable client (no VAC, no Steam DRM)

M1 is split into five sub-milestones along the user's path: sign in, install,
play, then Proton. Each one is usable end to end through the CLI. These rules
apply to all five:

- **Platforms.** M1a to M1d exit on Linux and Windows. macOS builds and passes
  unit and replay tests in CI on every slice (PLAT-1). Its acceptance runs
  are deferred to M1e.
- **Clients.** M1a to M1d ship CLI commands only. The GUI crate keeps
  compiling but gets no new views until M1e, which builds all of them against
  the finished IPC API. LEAF-3 parity applies from M1e onward.
- **Acceptance.** Each slice has a checklist in `docs/acceptance/m1x.md`
  (`m1a.md` to `m1e.md`). Each check's result must match the official
  client's result for the same account (§19.3). A slice's exit includes the
  §20 budgets for what it introduces. GUI budgets apply at M1e.

#### M1a: Session and library

- The CM connection, its transports and reconnection (§9.1).
- Login with credentials, Steam Guard (email, device code, confirmation) and
  QR. Remembered accounts and switching (§9.2).
- Licenses, PICS with change polling, and the PICS cache (§9.3).
- Persona state (PLAY-2).
- The daemon state database, keyring storage and the IPC methods for the
  Daemon, Session and Library areas (§8.3).
- CLI: `login`, `logout`, `account`, `status`, `persona`, `library`, `info`,
  `config`, `events`, `daemon`, `completions`.

**Exit:** on Linux and Windows, a test account logs in through every login
path, and `ossteam library` lists exactly the apps its licenses grant, with
DLC grouped under the parent app. After a restart the session resumes from
the stored refresh token.

#### M1b: Content

- The CDN directory, manifests, chunks and depot keys (§10).
- Install, delta update, verify and uninstall. Resumable jobs, bandwidth and
  concurrency settings, update detection and the auto-update policy.
- Multiple library folders.
- Install scripts on native Windows (CONT-8).
- CLI: `install`, `update`, `verify`, `uninstall`, `jobs`, `folders`.

**Exit:** on Linux and Windows, a title with DLC depots installs, updates
through a delta, survives a daemon kill in the middle of a download and
resumes, verifies and repairs a corrupted file, and uninstalls cleanly. The
download-throughput budget is met.

#### M1c: Play (native)

- Launch-option resolution, the launch environment, supervision and
  `StopGame` (§11.1, §11.3).
- Play-session reporting (PLAY-1).
- `ossteam-steamclient` with the interfaces in §12.3, the game IPC (§12.4),
  the ABI discovery entries and conflict handling (§12.5), and ShimReports
  (§12.6).
- SteamStub detection with an honest refusal (§13.2).
- CLI: `launch`, `running`, `stop`.

**Exit:** on Linux and Windows, these run correctly: Spacewar (app 480) for
achievements and stats, a native title without Steamworks, and a 32-bit
Steamworks title. Playtime and in-game presence show up on the account. A
SteamStub-protected title is refused with exit code 7, and launches fail with
exit code 8 while an official Steam install owns the discovery entries.

#### M1d: Compatibility tools (Linux)

- Proton and Steam Linux Runtime chains built from `toolmanifest.vdf`, the
  compat-tool mapping and user overrides (§11.2).
- `compatdata` prefixes, the generated client-install directory, and install
  scripts inside the prefix.
- Compat-tool settings in the CLI (`config`, plus the `info` output).

**Exit:** on Linux, a Windows-only Steamworks title installs its tools
through the normal install engine and runs under Proton inside the Steam
Linux Runtime. Its stats and achievements reach Steam through the shim.

#### M1e: GUI and macOS

- Every GUI view from §16, built against the IPC API delivered by M1a to M1d.
- macOS: the discovery mechanism (§24, Q1), the macOS discovery entries, and
  acceptance runs for M1a to M1c.
- The GUI performance budgets in §20.

**Exit:** the checklists for M1a to M1c pass on macOS, and the M1a to M1d
checklists pass through the GUI on every platform each applies to. All §20
budgets are met. This completes M1.

### M2: Protection (Steam DRM and VAC)

- `docs/spec/drm.md` and `docs/spec/vac.md` written and reviewed.
- SteamStub and CEG served faithfully (§13).
- VAC hosting (§14), including any privileged helper and its review.

**Exit:** owned DRM-protected titles launch, and the same titles fail exactly
as they do with the official client on an account without a license.
VAC-secured titles connect to VAC-secured servers on every platform where VAC
is hosted, and every other platform reports VAC as unavailable. No VAC-related
setting exists that could weaken it.

### M3: Social and cloud

Steam Cloud sync (ISteamRemoteStorage and auto-cloud), the friends list and
chat (ISteamFriends in full), Workshop (UGC), password-protected betas, and
offline mode with truthful offline reporting to games.

### Later

The overlay (needs a VAC-impact ADR, VAC-4), `steam://` URL handling (opt-in,
so it never hijacks an official install), Family Sharing, and additional
Steamworks interfaces driven by ShimReport data.

---

## 22. Definition of done (any change)

- Every relevant requirement ID is referenced in the PR.
- A protocol or ABI behaviour comes with a `docs/spec/` page in our own words.
- No faking (FAITH-1 to FAITH-4), and unimplemented paths fail honestly.
- Code builds and tests pass on all platforms. Async code appears only where
  §6 allows it.
- Each new dependency passes `cargo deny`.

---

## 23. Risks

| Risk | Mitigation |
|---|---|
| Valve may disapprove of third-party clients, and accounts could be put at risk | Behave exactly like the official client on the wire (§19.3). Use test accounts for development. Document the risk to users in README. |
| A VAC implementation that is wrong could get users banned | Ship behind a feature flag (VAC-5), test differentially, and never ship an unvalidated build |
| The Steam protocol changes | Keep spec pages current, use recorded-replay CI, run live tests on a schedule, track Valve's protocol updates |
| Our own GUI toolkit is a large surface: text editing, IME, a Vulkan renderer, three platforms | Build only what the GUI uses; winit, cosmic-text and taffy do windows, shaping and layout; headless tests; a spike gates the port (ADR 0004) |
| Vulkan is not everywhere: macOS needs MoltenVK, and some Windows machines have no driver | Ship MoltenVK in the macOS bundle; draw with our own software renderer when no Vulkan device can be created (ADR 0004) |
| The Steamworks interface versions in use are a large surface | Expand interfaces based on ShimReport data. Fail honestly in the meantime. |
| The name "ossteam" contains "steam" | State non-affiliation clearly (LIC-8). Revisit the name before a 1.0 release (Q3). |
| Coexisting with an official Steam install | DISC-3 conflict handling; on Linux a private `~/.steam` for launched games (DISC-4), and Valve's entries taken over only by the user's choice, with its risks stated (DISC-5). Never silently overwrite Valve's entries. |

---

## 24. Open questions

1. **macOS discovery.** What exactly does macOS `steam_api` need to locate a
   client? Answered in `steamworks-abi.md` §1.3: the Mach service
   `com.valvesoftware.steam.ipctool`, not a file.
2. **Application identifier.** The reverse-DNS id used for macOS paths and
   bundles, and for Windows AppUserModelID.
3. **Project name.** Keep "ossteam" or rename before 1.0 (see §23).
4. **Windows VAC privileges.** Does faithful VAC hosting on Windows require
   an elevated service? To be decided in `vac.md` (M2).
