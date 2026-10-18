# ossteam

An open-source Steam client written in Rust. A per-user daemon, `ossteamd`,
owns the Steam session, the library and game processes. Two thin clients
drive it: `ossteam` (CLI, clap) and `ossteam-gui` (GUI, gpui).

The design, its rules and the milestones are in [SPEC.md](SPEC.md).
Subsystem specs are under [docs/spec/](docs/spec/).

> **Status: M0 (foundations).** The daemon, the IPC protocol, and
> `ossteam daemon start | stop | status` work. There is no Steam login yet.

## Not affiliated with Valve

ossteam is not affiliated with or endorsed by Valve Corporation. "Steam" is
used only to describe compatibility. Using a third-party client may carry a
risk to your account; develop and test with an account you can afford to
lose.

## Building

The toolchain is pinned in `rust-toolchain.toml`; rustup picks it up.

```sh
cargo build --release
target/release/ossteam daemon status
```

Games reach the daemon through the Steamworks client library. This
builds it for the host's 64-bit target and its 32-bit counterpart, and
puts both where the daemon looks for them (next to it, and in `lib32/`):

```sh
cargo xtask build-steamclient --release
cargo xtask build-steamclient --release --target x86_64-pc-windows-gnu
```

The 32-bit build needs the target's standard library (`rustup target
add i686-unknown-linux-gnu`) and, on Linux, 32-bit glibc and libgcc.

On Linux, gpui needs development packages for Wayland, X11 (xcb and
xkbcommon), fontconfig, ALSA and Vulkan. CI installs these on Ubuntu:

```sh
sudo apt-get install libasound2-dev libfontconfig-dev libvulkan1 libwayland-dev \
  libx11-xcb-dev libxkbcommon-x11-dev libzstd-dev
```

## Checks

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo xtask check-layering   # crate dependency rules (SPEC §5.2)
cargo xtask check-spdx       # license headers (SPEC LIC-1)
cargo deny check             # dependency licenses and sources (SPEC LIC-3)
```

## License

ossteam's source code is licensed under the GNU General Public License,
version 2 or (at your option) any later version. See [COPYING](COPYING).

Built binaries link gpui and other crates licensed under Apache-2.0, which
is compatible with GPLv3 but not with GPLv2. **Binaries are therefore
conveyed under the terms of GPL-3.0-or-later**, while the source stays
GPL-2.0-or-later (SPEC LIC-4).

Contributions are accepted under the same license, with a DCO
`Signed-off-by` line (SPEC LIC-2).
