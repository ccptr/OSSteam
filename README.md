# ossteam

An open-source Steam client written in Rust. A per-user daemon, `ossteamd`,
owns the Steam session, the library and game processes. Two thin clients
drive it: `ossteam` (CLI, clap) and `ossteam-gui` (GUI, on our own toolkit
`ossteam-ui`). Games reach the daemon through `ossteam-steamclient`, our
own Steamworks client library.

The design, its rules and the milestones are in [SPEC.md](SPEC.md).
Subsystem specs are under [docs/spec/](docs/spec/). Where each milestone
stands is in [docs/status.md](docs/status.md).

> **Status: M1 (a usable client) is built; its acceptance runs are under
> way.** It is tested mostly on Linux. The Windows code has run only under
> Wine so far, and macOS builds but has not been run. Not ready for
> everyday use.

## What works

- Signing in with a password, Steam Guard (email, device code,
  confirmation) or a QR code; remembered accounts and switching.
- The library: the apps your licenses grant, with DLC under its game.
- Install, update (delta), verify and repair, and uninstall, with
  resumable jobs, a bandwidth limit and several library folders.
  Install scripts run as the official client runs them (on Windows,
  so far only under Wine).
- Launching games, natively and through Proton and the Steam Linux
  Runtime. Games that use Steamworks get achievements, stats, local
  storage and auth tickets.
- The GUI: sign-in, the library, app pages, downloads and settings.

Not there yet: Steam DRM and VAC (M2); cloud sync, friends, beta codes,
the Workshop and offline mode (M3); Steam Input, Big Picture and Game Mode
(goals, no milestone yet). These fail honestly rather than pretend.

## Not affiliated with Valve

ossteam is not affiliated with or endorsed by Valve Corporation. "Steam" is
used only to describe compatibility. Using a third-party client may carry a
risk to your account; develop and test with an account you can afford to
lose.

## Building

The toolchain is pinned in `rust-toolchain.toml`; rustup picks it up.

```sh
cargo build --release
```

Games reach the daemon through the Steamworks client library. This
builds it for the host's 64-bit target and its 32-bit counterpart, and
puts both where the daemon looks for them (next to it, and in `lib32/`):

```sh
cargo xtask build-steamclient --release
cargo xtask build-steamclient --release --target x86_64-pc-windows-gnu
```

The 32-bit build needs the target's standard library (`rustup target
add i686-unknown-linux-gnu`) and, on Linux, 32-bit glibc and libgcc. On
macOS there is no 32-bit build; the library is universal instead, which
needs both `aarch64-apple-darwin` and `x86_64-apple-darwin` and Xcode's
`lipo`.

On Linux the build needs development packages for D-Bus (the daemon's
keyring) and zstd. The GUI loads Wayland, X11, xkbcommon and Vulkan at
run time, so it needs no development packages; on Ubuntu:

```sh
sudo apt-get install libdbus-1-dev libzstd-dev pkg-config
```

The GUI's shaders are committed as SPIR-V, so building needs no shader
compiler. Changing one needs `slangc` (the version in
`crates/ossteam-ui/shaders/slangc-version.txt`) and `cargo xtask shaders`;
`cargo xtask shaders --check` checks that the SPIR-V matches its source.

## Using it

The CLI starts the daemon when it needs it.

```sh
ossteam login             # prompts for the password and any Steam Guard code
ossteam library
ossteam install <app>
ossteam launch <app>
ossteam-gui
```

`ossteam --help` lists every command. Add `--json` for output other
programs can read.

## Checks

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo xtask check-layering   # crate dependency rules (SPEC §5.2)
cargo xtask check-spdx       # license headers (SPEC LIC-1, LIC-9)
cargo deny check             # dependency licenses and sources (SPEC LIC-3)
```

## License

ossteam's source code is licensed under the GNU General Public License,
version 2 or (at your option) any later version. See [COPYING](COPYING).

The libraries other programs may embed are licensed under the GNU Lesser
General Public License, version 2.1 or (at your option) any later version:
`ossteam-vdf`, `ossteam-ui`, `ossteam-dbus` and, once they are written,
the Steam Input library `ossteam-input` and the Steam Controller driver
`ossteam-steamcontroller`. See [COPYING.LESSER](COPYING.LESSER) (SPEC
LIC-9). Linked with Apache-2.0 crates, they are conveyed under the terms
of LGPL-3.0-or-later.

Built binaries link crates licensed under Apache-2.0, which
is compatible with GPLv3 but not with GPLv2. **Binaries are therefore
conveyed under the terms of GPL-3.0-or-later**, while the source stays
GPL-2.0-or-later (SPEC LIC-4).

Contributions are accepted under the license of the crate they change, with a DCO
`Signed-off-by` line (SPEC LIC-2).
