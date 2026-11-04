# Steam Cloud

| | |
|---|---|
| Status | Draft for M3; awaiting review. Nothing has been synced yet, and §8 lists what must be observed first |
| Requirements | SPEC M3, §12.3, FAITH-1 to FAITH-4, DIRS-1, LAUNCH-3, LAUNCH-4 |
| Code | none yet. M1c's local files are in `crates/ossteam-steamclient/src/remote_storage.rs` (`steamworks/isteamremotestorage.md`) |

Steam Cloud keeps a game's files on Steam's servers for each account, so
that they follow the account from one computer to another. The client
does the syncing: it downloads what changed before a game starts and
uploads what the game changed after it exits. A game takes part in one of
two ways. With the **API**, it writes its files through
`ISteamRemoteStorage`, and the client keeps them in a directory of its own.
With **Auto-Cloud**, the game writes wherever it likes, and the app's
configuration tells the client which files to sync. This page describes
both as far as they are known, and how ossteam would do the same.

**Verification.** This draft draws on the following sources:

- the message definitions that SteamDatabase extracts from the official
  client (`steammessages_cloud.steamclient.proto`,
  `steammessages_client_objects.proto`, `enums.proto`);
- Valve's Steamworks documentation of Steam Cloud ("Steam Cloud",
  partner site);
- the official Linux client's `logs/cloud_log.txt` and its
  `userdata/<account id>/<app id>/remotecache.vdf` files on the
  development computer. They cover one account, the API files of app 730
  and the client's own app 7, and about 400 launch and exit syncs. Only
  their structure is used here, never their values;
- a community table of the client's numeric file roots
  (SteamCloudFileManager, `ROOT_PATH_MAPPING.md`), used for facts only;
- app info in ossteam's own PICS cache (`ufs`, 201 apps).

The order of the steps in §4 comes from the log. What each step sends
comes from the message definitions, matched to the log by name. Anything
the log does not show is *unverified* unless a line names its source.

## 1. What Steam keeps

For each account and app, Steam keeps a set of files. Each file has:

- a name: a relative path with `/` separators;
- the SHA-1 of its contents;
- a timestamp in Unix seconds;
- its size (`raw_file_size`);
- a persist state (`ECloudStoragePersistState`): persisted (0),
  forgotten (1) or deleted (2);
- the platforms it syncs to (`platforms_to_sync`, an
  `ERemoteStoragePlatform` bit set: Windows 1, macOS 2, Linux 8, all bits
  set for all);
- the machine that wrote it, and a path prefix (§3.3), each as an index
  into lists sent with the file list.

The set has a **change number**, which every upload batch increases by
one. The log shows it going up by one per batch: "Upload batch initiated,
ID: …, ChangeNumber: 361" after "already synced to global change number
'360'". A client remembers the change number it last synced to, and asks
for what changed since then (§4.1).

## 2. The app's configuration (`ufs`)

The app's info carries a `ufs` section (`pics.md`). ossteam already reads
`quota` and `maxnumfiles` (`steamworks/isteamremotestorage.md` §1). The
cache shows these keys:

| Key | Seen in | Meaning |
|---|---|---|
| `quota` | 32 apps | Bytes per user. 0, or no key, means the app has no Steam Cloud |
| `maxnumfiles` | 32 apps | Files per user; 0 is no limit |
| `limiteduserquota` | 4 apps (241100, 760, 761, 764) | A smaller byte quota, always below `quota`. *Unverified:* that it applies to limited accounts (accounts that have spent less than the threshold). How ossteam learns that an account is limited is open |
| `appidredirect` | 410 → 400 | The app whose cloud files this app uses. Valve's documentation calls it "Shared cloud APP ID", "most commonly used to share saved games between a demo and a full game" |
| `hidecloudui` | 3 apps | `1`: the official client shows no cloud status for the app. *Unverified:* whether it changes syncing |
| `ignoreexternalfiles` | 223350 | *Unverified* meaning. 223350 also has quota 0 |
| `sync_while_suspended` | 480 | Dynamic sync on suspend (§6). Out of scope |
| `savefiles` | 2 apps | Auto-Cloud rules (§3.2) |
| `rootoverrides` | none in the cache | Per-OS changes to the rules' roots (§3.2) |

Valve's documentation adds a "developers only" mode, in which only
accounts with a developer comp license see the app's cloud. *Unverified:*
how that reaches the client.

## 3. Files and where they live

### 3.1 API files

A file written through `ISteamRemoteStorage` belongs to root 0. The
official client keeps these under
`<Steam>/userdata/<account id>/<app id>/remote/`. ossteam keeps them under
`<user_dir>/remote/` (`steamworks-abi.md` §3.4, `user_dir`) and already
writes and reads them there (M1c). Its name on Steam is the name the game
used.

### 3.2 Auto-Cloud files

Each entry of `savefiles` is a rule:

| Key | Meaning (Valve's documentation, cache) |
|---|---|
| `root` | Where the path starts (table below) |
| `path` | A subdirectory under the root; `.` for none. `{64BitSteamID}` and `{Steam3AccountID}` are replaced by the account's 64-bit SteamID and its account id (`241100`: `{Steam3AccountID}/config`) |
| `pattern` | A file name with `*` as a wildcard (`*.sav`, `*`) |
| `recursive` | `1`: subdirectories of `path` match too |
| `platforms` | A list of platform names (`all`, `Windows`, `MacOS`, `Linux`). No list is all platforms. Files that a rule restricted to one OS uploads sync only to that OS |
| `siblings` | Seen once (410: `pattern` `*.sav`, `siblings` `tga`). *Unverified:* files with the same stem and that extension, which Portal writes as save thumbnails, sync with the match |

Root names and where ossteam would put them:

| `root` | Number (community table) | Linux | Windows | macOS |
|---|---|---|---|---|
| `gameinstall` | 1 | the app's install directory | the same | the same |
| `WinMyDocuments` | 2 | — | Documents (Known Folder) | — |
| `WinAppDataLocal` | 3 | — | `%LOCALAPPDATA%` | — |
| `WinAppDataRoaming` | 4 | — | `%APPDATA%` | — |
| `MacHome` | 6 | — | — | `~` |
| `MacAppSupport` | 7 | — | — | `~/Library/Application Support` |
| `MacDocuments` | 8 | — | — | `~/Documents` |
| `WinSavedGames` | 9 | — | Saved Games (Known Folder) | — |
| `WinProgramData` | 10 | — | `%PROGRAMDATA%` | — |
| `SteamCloudDocuments` | 11 | `~/.SteamCloud/<account name>/<game folder>` | `Documents\Steam Cloud\<account name>\<game folder>` | `~/Documents/Steam Cloud/<account name>/<game folder>` |
| `WinAppDataLocalLow` | 12 | — | LocalLow (Known Folder) | — |
| `MacCaches` | 13 | — | — | `~/Library/Caches` |
| `LinuxHome` | 14 | `~` | — | — |
| `LinuxXdgDataHome` | 15 | `$XDG_DATA_HOME` | — | — |
| `LinuxXdgConfigHome` | 16 | `$XDG_CONFIG_HOME` | — | — |

Valve's documentation lists the names, except `WinProgramData`,
`MacCaches` and `LinuxXdgConfigHome`, and the Linux path of
`SteamCloudDocuments`. The numbers, the other names and the other
`SteamCloudDocuments` paths are from the community table only. The
Windows folders come from the Known Folder API, since the user can move
them, as Valve's documentation says. A rule whose root does not exist on
this OS, and that no override maps, matches nothing. On Linux,
`SteamCloudDocuments` is treated so until a game is seen that uses it
(§9 Q1).

A **root override** (`rootoverrides`) maps a root to another one on one
OS. From Valve's documentation and the community table: `root` is the
original root, `os` the OS it applies to (`Windows`, `MacOS`, `Linux`),
`oscompare` always `=` so far, `useinstead` the new root, and then either
`addpath`, a directory put between the new root and the rule's `path`, or
`pathtransforms`, a list of `find`/`replace` pairs that rewrite `path`
("Replace Path" in the partner site). *Unverified:* how `find` matches.
Valve's documentation says that files of a rule with overrides "will sync
to all platforms in both the original Root and all Overrides".

**Proton.** A game that runs through a compatibility tool is a Windows
game to Steam Cloud. *Unverified* (no log of a Proton title with
Auto-Cloud yet): its `Win*` roots are the prefix's folders, under
`<compatdata>/pfx/drive_c/users/steamuser/` (`compat-tools.md` §6.1), and
`gameinstall` is the install directory. The log shows that the client
keeps a platform per app and handles a change: "Detected app platform
change since last sync - from 'linux' to 'windows'", followed by a
"Backup of AutoCloud files in preparation for path changes" and a
"Restore … after path changes". What that does to the files is unknown
(§8).

**`steam_autocloud.vdf`.** Valve's documentation says the client creates
this file "in each location specified by your Steamworks cloud paths".
Its contents are unknown (§8).

### 3.3 Names on Steam

The file list (§4.1) gives each file a `path_prefix_index` into a list of
`path_prefixes`. The 730 log shows only root-0 names (`cfg/…`,
`cs2_user_keys.vcfg`). *Unverified:* that an Auto-Cloud file's name on
Steam is a prefix naming its root and path (such as
`%GameInstall%portal/save/`) followed by the name below it, and that this
is how a file written under one root on Windows finds its way to the
override's root on Linux.

### 3.4 What ossteam remembers

The official client keeps, per account and app, a `remotecache.vdf` with
the change number it synced to, an `OSType` and, per file: `root` (the
number above), `size`, `localtime`, `time`, `remotetime`, `sha`,
`syncstate`, `persiststate` and `platformstosync2`. ossteam keeps the same
facts in its state database (D-6), not in that file: Steam's directory is
not ossteam's (DIRS-1). The fields that matter are the change number, and
per file the SHA-1 and timestamp as last synced, the persist state and
the platforms. With them, ossteam can tell for each file whether it
changed here, on Steam, or on both since the last sync.

## 4. Syncing

Every Cloud call is a unified service method (`cm.md` §3.5), `Cloud.<name>#1`.
Uploads and downloads themselves are plain HTTP requests to the hosts
that the replies name: Google Cloud Storage, Amazon S3 and Azure Blob
Storage hosts in the log. ossteam would use the HTTP client it already
has for content downloads (`content.md`).

### 4.1 Before a game starts

This is the `SynchronizingCloud` phase of the official client's launch
(`launch.md` §2). The log shows, per launch:

```
Starting sync (AC Launch,down,)
AutoCloud checking local state for user <account id>
Currently already synced to global change number 'N', should be nothing to download
Download complete in build list
Running AutoCloud on launch. Looking for existing files to watch:
AutoCloud done. Watching N files
```

ossteam would:

1. Send `SignalAppLaunchIntent` with the app id, a client id (§4.6), the
   machine name, `ignore_pending_operations` false, and the OS type.
   The reply lists `pending_remote_operations`: another machine with a
   session of the app running (1) or suspended (4), or an upload in
   progress (2) or pending (3), each with the machine's name and when it
   was last updated. When the list is not empty, the launch stops with
   `CONFLICT`, naming the machine and the operation. The user can launch
   anyway, and the intent is then sent again with
   `ignore_pending_operations` true. *Unverified:* that this is what the
   official client's "Play anyway" sends.
2. Send `GetAppFileChangelist` with the change number synced to. When
   the reply's `current_change_number` is that number, nothing changed on
   Steam. Otherwise its `files` list every file, or with `is_only_delta`
   those that changed.
3. Compare each file (§4.4), download what changed only on Steam (§4.3),
   and delete local files that Steam lists as deleted. A conflict stops
   the launch (§4.4).
4. Record the new change number and the files as synced.
5. For Auto-Cloud, note the matching local files, so that the exit sync
   can tell which are new.

The launch reply waits while this runs, as it waits for tool installs
(`compat-tools.md` §4). When the account is not logged on, or a step
fails, the launch stops with `CONFLICT` and a message that cloud files
could not be synced; the user can launch anyway. *Unverified:* the
official client's dialog offers the same choice ("Play anyway").

### 4.2 After a game exits

When the play session ends (`launch.md` §4), the log shows:

```
Starting sync (up,AC Exit,)
Running AutoCloud on exit. Looking for new and updated files
AutoCloud complete
Currently already synced to global change number 'N', should be nothing to download
Need to upload file <name>
Upload batch initiated, ID: <batch>, ChangeNumber: N+1, App Build ID: <build>
HTTP upload for file '<name>' beginning . . .
HTTP upload for file '<name>' (offset=0, length=<size>) to (<ip>, host: <host>), path <path> - success.
Upload OK for file <name>
Upload complete, result OK
```

ossteam would:

1. Find the files that changed here since the last sync: API files under
   `remote/`, and files that the Auto-Cloud rules match, by SHA-1 against
   what was last synced. Files the game deleted are deletes; files it
   forgot (§5) are left out.
2. Check that nothing changed on Steam meanwhile (`GetAppFileChangelist`,
   as in the log's "already synced" line).
3. Send `BeginAppUploadBatch` with the app id, machine name, the names to
   upload and to delete, the client id and the installed build id. The
   reply gives the batch id and the change number the batch will make.
4. For each file, send `ClientBeginFileUpload` with its size and raw size,
   SHA-1, timestamp, name, platforms, cell id, `can_encrypt` false and the
   batch id. The reply's `block_requests` are HTTP requests to make, each
   with a method, host, path, headers, and a range of the file (or an
   explicit body). Then send `ClientCommitFileUpload` with
   `transfer_succeeded`, the app id, SHA-1 and name; `file_committed` says
   whether Steam took it.
5. For each delete, send `ClientDeleteFile` with the name,
   `is_explicit_delete` true and the batch id.
6. Send `CompleteAppUploadBatchBlocking` with the batch id and the
   batch's result, then `SignalAppExitSyncDone` with whether the uploads
   completed and whether any were needed.

When a step fails, the files stay marked as changed and are uploaded at
the next exit or logon. The log shows such a deferral as "Need to sync
from local change number … but not attempting now". *Unverified:* when
the official client retries.

The game's files are only read during this, never changed.

### 4.3 Downloads

`ClientFileDownload` with the app id and name answers with the host,
path, HTTPS flag and headers of an HTTP GET, and with the file's size,
raw size, SHA-1, timestamp, and whether it is encrypted or an explicit
delete. ossteam would download to a temporary file next to the target,
check the SHA-1, set the timestamp, then replace the file, as the M1c
writes do (`steamworks/isteamremotestorage.md` §1).

Two cases are open (§8):

- `file_size` below `raw_file_size` would mean the stored body is
  compressed. The format is unknown. Until it is known, such a file fails
  to download, and the launch stops as in §4.1.
- `encrypted` true. ossteam sends `can_encrypt` false on upload, but a
  file another client uploaded may be encrypted (`GetClientEncryptionKey`
  exists). Such a file fails the same way.

### 4.4 Changes on both sides

For each file, ossteam compares three SHA-1s: the last synced, the
local one and Steam's.

| Here | On Steam | Action |
|---|---|---|
| unchanged | unchanged | none |
| changed | unchanged | upload at exit |
| unchanged | changed | download before launch |
| changed | changed, to the same contents | none; record as synced |
| changed | changed differently | conflict |

A conflict stops the launch with `CONFLICT`. The user chooses to keep the
files here or the files on Steam, for the app as a whole: the official
client's `ClientConflictResolution` notification carries only the app id
and `chose_local_files`. ossteam sends it, then downloads (keep Steam's)
or uploads (keep these) and launches. *Unverified:* what the official
client counts as a conflict, and whether it compares timestamps as well.

### 4.5 Turned off

Valve's documentation says the user can turn Steam Cloud off for all apps
and for one app. ossteam would have the settings `cloud.enabled` (default
`true`) and `cloud.enabled.<app id>` (a family like `compat.tool.<app
id>`, `ipc.md` §5.4). While either is off, nothing for that app is
synced, and the API says so (§5).

### 4.6 Identity

`client_id` is a 64-bit number that the official client sends with the
intent, batch and exit calls, so Steam can tell its sessions apart.
`machine_name` is shown to the user on other machines. ossteam would use
the host name, and a random client id made once per install and kept in
the state database. *Unverified:* how the official client makes its
client id, and the OS type it sends. The `OSType` in its `remotecache.vdf`
files is `-203` or `0`, and ossteam sends no OS type at logon
(`auth.md`).

## 5. `ISteamRemoteStorage` with Steam Cloud

The M1c behaviour (`steamworks/isteamremotestorage.md` §2) changes as
follows. The shim still only writes the local files; the daemon syncs
them before and after the game. The shim tells the daemon what the game
asked for that is not visible on disk: forget, sync platforms, explicit
deletes.

| Method | With Steam Cloud |
|---|---|
| `IsCloudEnabledForAccount` | `cloud.enabled` |
| `IsCloudEnabledForApp` | both settings (§4.5), and the app has a quota |
| `SetCloudEnabledForApp(on)` | sets `cloud.enabled.<app id>`. *Unverified:* whether the official client lets a game change the user's choice |
| `FilePersisted(name)` | true when the file is as last synced and its persist state is persisted |
| `FileForget(name)` | marks the file forgotten: the exit sync leaves it out, and the local file stays. *Unverified:* what the official client sends Steam for it |
| `FileDelete(name)` | deletes the file and marks an explicit delete for the exit sync |
| `SetSyncPlatforms(name, platforms)`, `GetSyncPlatforms(name)` | the file's `platforms_to_sync`; all for a new file, as Valve's documentation says |
| `GetQuota` | from `ClientGetAppQuotaUsage` when logged on (the log has "Starting sync (Get App Quota Usage,)"), else from the app's info as in M1c |
| `GetLocalFileChangeCount`, `GetLocalFileChange` | still 0: files change only between sessions (§6) |

The quota checks of M1c stay as they are.

## 6. Out of scope

- **Dynamic sync** (`SuspendAppSession`, `ResumeAppSession`,
  `sync_while_suspended`): it syncs when a device suspends with the game
  running, which is a Steam Deck feature. ossteam suspends no games.
- **The client's own settings** (app 7, `sharedconfig.vdf`, "Roaming
  Config" in the log): the official client syncs its own settings
  through Steam Cloud. ossteam has none of those settings.
- **Workshop and shared files** (`FileShare`, UGC): M3, but not this page.
- **Encryption** (§4.3).

## 7. Planned structure

- `crates/ossteamd/src/cloud/`: rules and roots (§3), the sync state
  (§3.4), and the launch and exit syncs (§4). The syncs run on the launch
  path's thread and the game's supervisor thread (CONC-1). They are
  synchronous, like content downloads.
- IPC: `Launch` gains `allow_unsynced` and `ignore_cloud_sessions`;
  `ResolveCloudConflict{app_id, keep}` with `LOCAL` or `STEAM`;
  `CloudSync` events with the app id, phase and files done; `GetApp`
  gains the app's cloud state. CLI: `ossteam cloud status [APP]` and
  `ossteam cloud resolve <APP> --keep local|steam`, plus `launch
  --allow-unsynced`. When standard input and output are a terminal,
  `ossteam launch` asks instead of failing: for a conflict, keep the
  local files, keep Steam's, or cancel; for a pending session or a
  failed sync, launch anyway or cancel. It then sends the launch again
  with the choice. Otherwise, as in scripts, the launch fails with
  `CONFLICT` and the commands above resolve it. GUI: the App page shows the cloud state and offers
  the choices of §4.1 and §4.4.
- Tests: a fake `Cloud` service in the fake CM with an in-memory store
  behind a local HTTP server, driving both syncs, conflicts, pending
  sessions and failed uploads.

## 8. Investigation before code

All of these need the test account logged in.

1. **An API title.** Spacewar (480) has `quota` 4096 and `maxnumfiles`
   10, and its SDK sample writes a file through `FileWrite`. With the
   official client: launch, write, exit, and record `cloud_log.txt` and
   `remotecache.vdf`. Then with ossteam, compare.
2. **An Auto-Cloud title**, native, to see the names on Steam (§3.3),
   `steam_autocloud.vdf`, and root numbers in `remotecache.vdf`.
3. **The same title through Proton** after a native run, to see the
   platform change (§3.2) and where the `Win*` roots land.
4. **A conflict**, made by changing a file both here and through the
   official client on another machine (or another install), to see what
   the official client calls a conflict.
5. **A compressed or encrypted download** (§4.3), if any of the test
   account's files is stored that way: `ClientFileDownload` shows it.

## 9. Questions for review

1. **`SteamCloudDocuments` on Linux** is `~/.SteamCloud/<account
   name>/<game folder>`, outside the XDG directories. Other machines and
   the game itself expect the files there. Recommended: treat it as the
   game's directory, not ossteam's, so DIRS-1 does not apply; the
   alternative is to sync nothing for those rules and say so. The same
   question does not arise for the other roots, which are where the game
   writes anyway.

   *Answer (2026-11-03):* left until a game is seen that uses this
   root. Until then, on Linux, a rule with it matches nothing, so its
   files are not synced.

2. **Writing to the test account's Steam Cloud** (§8): acceptable? A
   mistake can only lose that account's cloud files for the test titles.

   *Answer (2026-11-03):* acceptable.

3. **Conflicts and pending sessions in the CLI**: the launch fails and
   the user resolves with a command (recommended, as `launch` asks
   nothing), or `ossteam launch` asks on the terminal?

   *Answer (2026-11-03):* `ossteam launch` asks on the terminal when it runs
   interactively; otherwise the launch fails and the user resolves with a
   command (§7).

4. **Order of M3**: Steam Cloud first, then the friends list and chat,
   Workshop, betas with passwords, and offline mode?

   *Answer (2026-11-03):* Steam Cloud, then the friends list and chat, then
   offline mode, then Workshop, then betas with passwords (SPEC §21).

## 10. Verification against the official client

| Item | Status |
|---|---|
| Cloud service methods and fields | from the client's message dumps |
| Launch and exit sync order | from the official client's log |
| Upload batches raise the change number by one | from the log |
| Uploads and downloads are HTTP to storage hosts the replies name | from the log |
| `remotecache.vdf` fields | from the official client's files |
| Auto-Cloud rule keys `root`, `path`, `pattern`, `recursive`, `platforms`, `siblings` | from app info |
| `rootoverrides` keys | community table and Valve's documentation; not in our cache |
| Root names | Valve's documentation, plus the community table for three names |
| Root numbers | community table only |
| `limiteduserquota`, `ignoreexternalfiles`, `hidecloudui` meaning | unverified |
| Names on Steam for Auto-Cloud files | unverified |
| Proton roots and the platform change | unverified |
| Conflict rule | unverified |
| Compression and encryption of stored files | unknown |
| `client_id` and OS type | unverified |
