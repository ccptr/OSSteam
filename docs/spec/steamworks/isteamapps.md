# ISteamApps

| | |
|---|---|
| Status | Implemented for M1c: ownership, DLC, branch, build, install directory, languages |
| Requirements | SPEC §12.3, PICS-3, FAITH-1 to FAITH-3, GIPC-2 |
| Versions | `STEAMAPPS_INTERFACE_VERSION001` to `008` (`vtables.txt`) |
| Code | `crates/ossteam-steamclient/src/apps.rs` |

"The app" is the app id the process claims (`steamworks-abi.md` §3.3).
Answers about it come from the welcome's `App` and its updates. Answers
about another app come from an `AppState` request to the daemon, which
the shim does not cache.

| Method | ossteam |
|---|---|
| `BIsSubscribed` | Whether the account owns the app. |
| `BIsSubscribedApp(app)` | Whether the account owns `app`: the app itself, one of its DLC, or `AppState`. |
| `BIsAppInstalled(app)` | Whether `app` is installed (`AppState` for another app). |
| `BIsDlcInstalled(dlc)` | Whether `dlc` is owned and installed. DLC without depots of its own is installed when it is owned. |
| `GetDLCCount` | The number of DLC in the app's info (`extended/listofdlc`). |
| `BGetDLCDataByIndex(i, app, available, name, size)` | DLC `i` in that order: its app id; `available` when the DLC's own info says it is released (`common/releasestate` is `released`); its name, truncated to `size` with a NUL. False when `i` is out of range. *Unverified:* that Steam's `available` means released. A DLC whose info the daemon has not fetched is listed as not available, with an empty name. |
| `GetCurrentBetaName(name, size)` | The installed branch and true, or false for `public`. |
| `GetAppBuildId` | The installed build id, or 0. |
| `GetAppInstallDir(app, buffer, size)` | The install directory, truncated to fit with its NUL. Returns the bytes written, NUL included, or 0 when `app` is not installed. *Unverified:* whether the official client counts the NUL. |
| `GetInstalledDepots(app, depots, max)` | The installed depots of `app`, at most `max`. Returns how many were written. |
| `GetCurrentGameLanguage` | The language the app is installed in: the `content.language` setting, which chose its depots. |
| `GetAvailableGameLanguages` | The languages of the app's info, joined with commas: the keys of `common/languages` whose value is not 0, or else the keys of `common/supported_languages` marked `supported`. *Unverified:* which of the two the official client uses. |
| `GetAppOwner` | The license owner: the active account. |
| `BIsSubscribedFromFamilySharing` | False: M1 has no Family Sharing, so the owner is always the account. |
| `BIsLowViolence` | Whether the license that grants the app carries the low-violence flag, which also chooses its depots (`content.md` §6). |
| `BIsCybercafe` | False. *Unverified:* that the official client bases this on cybercafe license types, which ossteam's accounts do not hold. |
| `GetLaunchQueryParam(key)` | An empty string: ossteam starts no game from a `steam://run` URL, so there are no parameters. |
| `GetLaunchCommandLine(buffer, size)` | 0, with an empty buffer, for the same reason. |

Everything else fails under ABI-3: `BIsVACBanned` (VAC is M2),
`GetEarliestPurchaseUnixTime`, `BIsSubscribedFromFreeWeekend`,
`BIsTimedTrial` (the license details are not passed to the shim yet),
`InstallDLC`, `UninstallDLC`, `MarkContentCorrupt`,
`GetDlcDownloadProgress`, `GetFileDetails`, the proof-of-purchase keys,
`SetDlcContext` and the beta methods (`GetNumBetas`, `GetBetaInfo`,
`SetActiveBeta`).
