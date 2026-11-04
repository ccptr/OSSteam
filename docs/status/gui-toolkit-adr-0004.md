---
name: gui-toolkit-adr-0004
description: ADR 0004 (own GUI toolkit ossteam-ui on winit + ash) and its SPEC changes accepted 2026-11-03; spike done but for the maintainer's checks; toolkit (step 3) and the port (step 4) done, the previous framework gone from the workspace; live checks on the test account passed 2026-11-03; the portal moved to our own D-Bus client (an ADR amendment) the same day; local main since 2026-11-03 (the old main is the branch gpui), not pushed
metadata:
  type: project
---

On 2026-11-02 the maintainer asked to replace the GUI's third-party
framework with an in-house GUI
toolkit, as a new crate on our own stack, renderer on `ash` (Vulkan),
not wgpu, plus our own software renderer (used when Vulkan is missing,
or forced with `--renderer`; the spike measures both, memory included). [ADR 0004](../adr/0004-own-gui-toolkit.md) was proposed with
these SPEC changes: GUI-1 rewritten, GUI-5 added (an idle window does
not wake), CONC-3 loses its GUI exception, WS-1 admits `ossteam-ui`,
LIC-4 names winit, and the §1 intro, §4, §5.1, §5.3, §16 title and §23
rows follow. `docs/spec/gui.md` §2.1, §3, §4.2, §6 and §7 now describe
the toolkit design.

Also on 2026-11-02 the maintainer chose Slang for the shaders; the ADR's
Shaders item says so.

The maintainer asked on 2026-11-02 to start the spike (ADR step 2)
before reviewing the ADR;
it is on the branch `ossteam-ui-spike`, with its results and what is
left in [adr-0004-spike](../acceptance/adr-0004-spike.md). The maintainer
chose to keep winit's client-side decorations (sctk-adwaita, which draws
with tiny-skia) for now. After the spike and the review: the toolkit and
the port on a branch.

By the end of 2026-11-02 the spike has both renderers, the text field
with IME and the clipboard, the folder picker, and its measures on
Hyprland and on a headless sway (Wayland, Xwayland, lavapipe), all
within §20 with no idle wakeups, except fonts read from a cold disk
cache (spike item 1). The maintainer then decided: `auto` prefers
Vulkan; `arboard` on every platform but Wayland, with BSL-1.0 added to
SPEC LIC-3; the folder picker through ashpd on Linux, `rfd` elsewhere.
The ADR says so.

On 2026-11-02 the maintainer also asked to replace the previous
framework with `ossteam-ui` once nothing blocks it. On 2026-11-03 they accepted the
ADR and its SPEC changes, and agreed to this order: the spike's scale
and Wine runs, then the toolkit (step 3) starting with loading only the
fonts the GUI uses (spike item 1, the cold first frame that breaks
§20's 500 ms), then the port (step 4) on a branch. Still the
maintainer's: a light desktop and a pick through the real portal
dialog.

The scale and Wine runs were done on 2026-11-03: sharp at 1.25, 1.5
and 2 with both renderers, the first frame at scale 1 as before;
under Wine both renderers draw and the clipboard works, but any winit
window spins there (Wine's bug), and a prefix without fonts made the
spike panic (spike items 10 and 11). Idle wakeups on Windows wait for
real Windows.

Step 3 began the same day with the fonts (`fonts.rs`): only the UI
family's files load before the first frame, the rest on the font
thread. The cold first frame went from about 2 s to 64 ms and the warm
one from 101 ms to 17 to 30 ms; no fonts at all is an error, not a
panic (spike items 1 and 10).

Also on 2026-11-03 the maintainer asked for a clean implementation that
does not port the previous framework's code: the toolkit's API is its own (`panel()`,
`.row()`, `.pad()`, `.on_click(|state, cx| …)`, actions as plain types,
`"primary+f"` keystrokes), and the GUI's views will be written anew
against it. The ADR's Layout row and its Identity, Focus and Images
items were reworded to say so (an amendment to the accepted ADR). The
rest of step 3 landed then:

- `element.rs`, `style.rs`: one `Element<S>` type (panel, text, image,
  canvas, list, text input, overlay) with handlers that get `&mut S`;
  flexbox styles in logical pixels with inherited text properties,
  hover and keyboard-focus styles.
- `window.rs`: build, `taffy` layout in device pixels (whole pixels at
  any scale), paint into the scene, pointer, wheel, keys, IME, focus
  and Tab order, key contexts with actions bubbling up, scrolling the
  focused element into view, rows of a list built only in view,
  overlays kept inside the window, tooltips, timers and messages. No
  winit window in it, so `tests/window.rs` drives it headless (also
  under Wine).
- `app.rs`: winit's loop, `WaitUntil` for timers, a `Sender` for other
  threads, the appearance portal's change signal (`appearance::watch`).
- Images: a scene primitive drawn 1:1 by both renderers; the Vulkan
  renderer has the image pipeline (`shaders/image.slang`), a texture
  per image, freed after the application drops it.
- `examples/gallery.rs` shows all of it; on the headless sway it draws
  the same with both renderers, its first frame in 105 to 114 ms, no
  idle wakeups (one from Mesa's shader cache with Vulkan).

Bugs found and fixed on the way: the software renderer did not count
pixels it moved for a scroll as changed, so the compositor (and older
buffers) kept stale content; the runtime told winit a frame was coming
before a frame the software renderer then skipped, which on Wayland
stopped all later redraws; and the Wayland clipboard's worker crashed
when it outlived winit's connection at exit.

Resident memory in the gallery was 75 MB with Vulkan and 49 MB with
software (the spike: 55 and 28 MB). Of the 20 MB more, 16 MB was 72
Noto Sans files: the gallery's "▾" is not in Noto Sans, and shaping it
before the rest of the fonts had loaded sent cosmic-text through every
face it had, each mapped and kept. `text.rs` now merges the rest of the
fonts first when the UI face lacks a character; the gallery is at 58
and 31 MB, the rest being its larger heap and code and DejaVu Sans for
the "▾". (26 MB of the Vulkan figure is libLLVM, which Mesa's radeon
driver links; it was in the spike too.)

Open: a character whose fonts are not on cosmic-text's fallback lists
still makes it open faces until one has it, and one that no font has
opens them all: on this machine 2,637 files mapped, 489 MB resident
and 0.3 to 1.6 s on the UI thread for one call (Tangut, installed but
not on the lists: 220 files, 20 MB). Common scripts and emoji map one
font each. Steam names can carry such characters, so this wants our
own fallback (an index of which face covers which characters, built on
the font thread) before the friends list is ported; the maintainer to
decide.

The text field gained a secret mode for the login page's password
(bullets; Copy and Cut do nothing).

The port (step 4) was done on 2026-11-03 on a branch that became
`main` the same day, at the maintainer's request (the old `main`, with
the previous framework, is the branch `gpui`); not pushed. Every page is written
anew against `ossteam-ui` (`gui.rs`, `widgets.rs`, `pages/`); the
model, store, daemon connection and formatting are the same code, now
plain structs whose results come back as messages. The previous
framework, its patches
and its git sources are gone from `Cargo.toml`, `deny.toml` and the
lock file, and the GUI needs no Linux development packages to build.
The toolkit gained on the way: Tab from a focused element that is not a
tab stop goes on from it (it went back to the first stop),
`Cx::focus_visible`, `Cx::refresh_interval`, and `CursorIcon` from
`style`.

Checked on the headless sway against the gui_states bench, by keyboard
only: library grid and list, the App page (hero and logo, details,
compatibility, install, launch and uninstall choosers, Play and Stop,
last run), Downloads, Settings, the account menu and bob's email-code
sign-in. No idle wakeups in 10 s. The 10k-app scroll meets §20 with
both renderers (worst frame 2.1 ms Vulkan, 3.3 ms software; to the end
of the present 2.7 and 10.2 ms); the first frame comes after 27 ms
(Vulkan) and 13 ms (software). The bench now also prints the time to
the end of the present; its budget is still on the work to the end of
the page's paint, as `gui.md` §7 says. The daemon's GUI integration
test passes; the toolkit's and GUI's tests pass under Wine, given a
prefix with fonts, and the Windows build draws its no-daemon page
there.

Live on 2026-11-03, on the maintainer's desktop (Hyprland, a 165 Hz
monitor at scale 1) against the real daemon and the test account, by
keyboard but for what needed the maintainer:

- Sign-in: a wrong password refused by Steam (`INVALID_PASSWORD (5)`);
  a password sign-in with *Remember me* off (not listed, nothing in the
  keyring; Steam asked no Steam Guard code); QR, which the maintainer
  saw rotate and sign in; log out from the account menu deleting the
  token.
- The Dota 2 App page (hero, logo, its four DLC as `info` lists them);
  persona Invisible, Offline and Online from Settings, each sent to the
  CM and shown on the account button.
- Spacewar installed, a damaged file repaired by Verify, uninstalled,
  each matching `ossteam jobs`; Play failed with the error `launch`
  gives (see [m1c-play](m1c-play.md)).
- A library folder added through the real portal dialog
  (`xdg-desktop-portal-gtk`) and removed; the GUI following the desktop
  from dark to light and back.
- First frame 31 to 58 ms (Vulkan) and 14 to 15 ms (software); the 10k
  scroll at 165 Hz: worst frame 4.6 ms, no frame missed with Vulkan in
  two runs, 0 to 2 of 11,411 with the software renderer.
- Idle, the main thread wakes about every 1.5 s, when Hyprland pings
  the window (`xdg_wm_base.ping`, which it uses to find hung apps), and
  the clipboard thread, which shares winit's connection, with it. Sway
  sends no pings, so there are no wakeups there.

Fixed from it: after a refused sign-in the cursor goes back to the
password field, and a running sign-in's Cancel keeps the focus of the
button that started it; Enter in the library search selects the first
result once it arrives, with the focus ring on the grid, so a second
Enter opens it.

At the maintainer's request the same day, SPEC.md (CONC-3, GUI-1,
§21 M0, §23), gui.md, ADR 0004 and the other notes stopped naming the
previous framework; git history keeps it.

IME on X11 works: on the headless sway's Xwayland, ibus's XIM front
end composed "ue9" in the library search (drawn underlined at the
cursor) and committed "é" (ibus's Ctrl-Shift-U entry, since no
composing engine is installed here).

Later on 2026-11-03 the maintainer asked why the dependency tree is so
large (505 packages in the lock file, 328 crates on Linux, the GUI's
204) and chose to replace `ashpd` with our own minimal D-Bus client. ADR
0004 is amended (its Amendments section) and SPEC CONC-3 no longer names
the portal as the GUI's async exception: nothing in ossteam is async
now. The client is its own crate, `ossteam-dbus` (at the maintainer's
request; WS-1 and the layering check name it), with the portal's calls
in `ossteam-ui`'s `src/portal.rs`; `ashpd`, `async-io` and `futures-lite` are gone,
and with them zbus. The GUI's dependencies on Linux went from 204 crates
to 153, the workspace's from 328 to 284, and the lock file from 505 to
469. Checked: unit tests against GLib's encoding of the same messages
(both byte orders) and against a bus on a socket pair, also on 32-bit
Linux; on the headless sway, with `fake_portal.py` (now with Settings)
on a private dbus-daemon, the gallery came up dark and followed two
change signals, with `ReadOne` and with version 1's `Read`, and its
threads did not wake in 6 s idle; the spike picked a folder named with
a space and "ä", and a cancel, with both renderers; xdg-desktop-portal
with its GTK back end opened the dialog with our title, and killing it
came back as response 2 (an error, as with `ashpd`); on the
maintainer's session bus (dbus-broker) the gallery read `prefer-dark`
from the real portal. FreeBSD, which the client compiles for, is not
checked (no target here).

Other findings from the same look at the tree, not acted on: `skrifa`,
`read-fonts` and `font-types` are in twice because `swash` resolved to
`skrifa` 0.44 while `cosmic-text` asks for 0.40 (a lock-file update
merges them); `smithay-clipboard` 0.7.3 needs `smithay-client-toolkit`
0.20 while winit 0.30 has 0.19 (0.7.2 would share it); `rsa` 0.9 keeps
the older RustCrypto crates beside `sha1` 0.11's until `rsa` 0.10.

Still the maintainer's: pushing, real Windows
and macOS. The rows of m1e.md not run live (Steam Guard codes, a second
account, a PICS change, a download big enough to pause, Proton) wait as
before. Open as before: the font fallback.
