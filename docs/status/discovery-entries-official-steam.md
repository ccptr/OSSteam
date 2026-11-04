---
name: discovery-entries-official-steam
description: An official Steam install owns ~/.steam/sdk64 on the dev machine, so launches fail with exit 8; a private ~/.steam per launch (DISC-4 to DISC-6) was specified 2026-11-03 and waits on the maintainer's review before code
metadata:
  type: project
---

An official Steam install on the dev machine owns `~/.steam/sdk64`
(→ `~/.local/share/Steam/linux64`), so the daemon makes no ABI discovery
entries (SPEC §12.5) and every launch fails with exit 8.

On 2026-11-03 the maintainer chose a way out, and asked for warnings to
the user wherever it matters. Drafted in `steamworks-abi.md` §1.1 (with
SPEC DIRS-2, DISC-1 and DISC-3 reworded and DISC-4 to DISC-6 added, and
`launch.md` §2, `ipc.md` §5.1 and §5.4, `gui.md` §2.4 and §4.5,
`compat-tools.md` §9 and `vac.md` §7 question 6 following):

- every launch runs in a user and mount namespace with
  `<data>/steam-sdk` mounted read-only over `~/.steam` (DISC-4);
- the real entries only while free, as before; without namespaces,
  launches use them and fail with `CONFLICT` when an official client
  owns them;
- `discovery.take_over_official`, off by default: take over the
  official entries while that client is not running, hand them back,
  yield when it starts; its two risks (children of running games, and
  the race with `steam.sh`) are stated to the user (DISC-5);
- warnings when namespaces are not available, and while the setting is
  on: daemon log and a line on its standard error, `GetStatus`,
  `ossteam status` and `launch` lines, GUI notice and Settings (DISC-6).

Facts behind it, from the official client's `steam.sh` (facts only):
it counts the client as running only when `steam.pid`'s process has
`~/.steam/steam.pipe` open, and otherwise recreates its links with
`ln -fsn`, ossteam's included. One shipped `libsteam_api.so` builds
`~/.steam` from `$HOME`.

**How to apply:** no code until the maintainer has reviewed these SPEC
changes. Open questions for them are in §1.1.7 (hiding
`~/.local/share/Steam`, anti-cheat, the Steam Linux Runtime inside the
namespace). Don't work around the conflict meanwhile.
