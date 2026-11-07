---
name: install-scripts-in-prefix
description: Install scripts in a Proton prefix follow the official client since 2026-11-07 (applied at install, launch and verify, nothing removed, uninstall deletes the prefix, Wine's S: paths); built and run live; the SPEC.md lines for Uninstall need sign-off; open - the client installs tools with the app, script order, Windows unverified
metadata:
  type: project
---

On 2026-11-07 the official client was watched running Spacewar's
install script in a Proton prefix (`compat-tools.md` §7.1), and the
maintainer chose that ossteam follow it ("follow the client"). Built the
same day and run live with Spacewar ([m1d](../acceptance/m1d.md)):

- the scripts apply at the end of an install or update (when every tool
  is installed), at every launch, and again at verify; nothing is
  removed;
- uninstall runs the Run Process On Uninstall entries, then deletes the
  files and `compatdata/<app>`, saves in it included;
- paths are named as Wine names them (`S:` for the library folder when
  Proton maps it), from Wine's `find_drive_nt_root`.

**SPEC changes, for the maintainer's sign-off**
([spec-changes](../../AGENTS/rules/spec-changes.md)): SPEC §8's IPC
table now has `Uninstall{app_id}` and §15's CLI synopsis
`ossteam uninstall <APP> [--yes] [--wait]`: `remove_compat_data` and
`--remove-compat-data` are gone (proto field 2 reserved). The behaviour
was asked for; the wording was not. Also the agents' choice: the CLI's
and the GUI's uninstall confirmations say that the prefix and saves in
it go too, which the official client's dialog does not.

Open, not acted on:

- The official client installed the chain's tools together with the app
  (the app's update waited for them); ossteam installs tools at the first
  launch, so after a first install with no tools the scripts wait for
  that launch.
- Order: the client put the app's own script before the Steamworks
  Common Redistributables'; ossteam orders by depot id, which gave the
  same order for Spacewar (481 before 229006) but need not for others.
- After an update, and on native Windows, the client's behaviour is
  unverified; Windows keeps the documentation's (`install-scripts.md`
  §3), removal at verify included.
