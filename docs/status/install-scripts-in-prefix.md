---
name: install-scripts-in-prefix
description: In a Proton prefix the official client applies install scripts at install, launch and verify, removes nothing at verify, deletes the prefix at uninstall, and names the install directory on S: (seen 2026-11-07); ossteam's compat-tools §7 differs on each; whether to follow it is for the maintainer; the removal at verify/uninstall waits on that
metadata:
  type: project
---

On 2026-11-07 the official client ran Spacewar (480) under Proton
Experimental on the test account, through install, a launch, two
verifies and an uninstall (`compat-tools.md` §7.1). Before then §7 was
ossteam's own design. It differs from what the client did:

| | Official client | ossteam (§7) |
|---|---|---|
| When the scripts run | end of install, every launch, every verify, uninstall | every launch |
| Verify | applies them again; removes nothing | removes the recorded values (built for Windows, not yet for a prefix) |
| Uninstall | evaluator in `mode` 3, then deletes `compatdata/<app>` | would remove the values; keeps the prefix unless `--remove-compat-data` |
| `%INSTALLDIR%` | `S:\steamapps\common\<dir>` | `Z:\<host path>` |

For the maintainer: whether ossteam should follow the client on any of
these. Deleting the prefix at uninstall loses saves kept in it, which is
why ossteam keeps it by default (`--remove-compat-data`); that one is a
choice, not only a fact to copy. Until it is settled, the unbuilt part
of M1d (removing values in the prefix at verify and uninstall,
[m1d-compat-tools](m1d-compat-tools.md)) waits: the client's verify
removes nothing, so building it as §7 says would knowingly differ.

On native Windows the client's behaviour is still unverified
([hardware-mac-windows](hardware-mac-windows.md)); `install-scripts.md`
§3 follows Valve's documentation there.
