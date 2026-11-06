# Compatibility tools (Linux)

| | |
|---|---|
| Status | Draft for M1d. §1 to §4 observed in the official Linux client's app-info cache and logs; §5 and §6 follow Valve's published compat-tool interface; the rest is ossteam's design, partly unverified |
| Requirements | SPEC §11.2 (COMPAT-1 to COMPAT-3), PLAT-2, LAUNCH-4, LAUNCH-5, FAITH-1, DIRS-1, RE-2 |
| Code | tool list, mappings, manifests and chains in `crates/ossteam-launch/src/compat/`; the daemon's side in `crates/ossteamd/src/core/compat.rs` |

A compatibility tool is a program that ossteam puts in front of a game's
command line. Valve ships two kinds as Steam apps: the Steam Linux Runtime
(SLR), a container that gives a Linux game a known set of libraries, and
Proton, which runs Windows games with Wine. Tools can need other tools:
Proton runs inside an SLR container. This page covers which tool an app
gets, how tools are installed, the command line and environment of a
launch through tools, the Proton prefix, and what the shim needs inside
a container. Native launches without a tool are in `launch.md`.

Compatibility tools exist only on Linux (PLAT-2). Everything here is
compiled on every platform and used only on Linux.

**Sources.** Each point was taken from one of these, and is marked
*unverified* where none of them settles it:

- the official Linux client (build 1791918634): its app-info cache
  (`appcache/appinfo.vdf`), `logs/compat_log.txt`,
  `logs/gameprocess_log.txt`, the `toolmanifest.vdf` files of installed
  tools, and strings in `ubuntu12_32/steamclient.so`;
- the compatibility tool interface that Valve's runtime project publishes
  (`docs/steam-compat-tool-interface.md` in steam-runtime-tools, MIT) and
  the `pressure-vessel-wrap` manual from the same project;
- Proton's source (`proton`, `lsteamclient`, `steam_helper`), read for
  the variables and files it uses.

Nothing from those projects is copied into the repository (RE-2).

## 1. The tool list

Valve publishes the tools in the app info of app 891390, "SteamPlay 2.0
Manifests" (type `Config`, `common/oslist` `linux`). Its `extended`
section has four parts:

| Path | Content |
|---|---|
| `extended/compat_tools/<name>` | one tool (below) |
| `extended/selectable_aliases/<alias>` | an alias the user can choose; `display_name` |
| `extended/app_mappings/<n>` | Valve's mapping for one app (§2) |
| `extended/app_compat_configs/<n>` | per-app settings for the tool (§5.3) |

A tool entry has:

| Key | Meaning |
|---|---|
| `appid` | the tool's app id |
| `depotid` | its depot |
| `require_tool_appid` | the tool this one runs inside; absent if none |
| `display_name` | the name shown to the user |
| `from_oslist` | the platform whose programs it runs: `windows` for Proton, `linux` for the SLR |
| `to_oslist` | the platform it runs on: `linux` |
| `aliases` | comma-separated other names |
| `unlisted` | `1`: not offered to the user. `2` is also seen (*unverified* meaning; ossteam treats any value but `1` as listed) |
| `legacy` | `1` for older Proton versions (*unverified*: how the official client presents them) |
| `depotsizemb` | the depot's size, for display |

On 2026-10-18 the list held, among others:

| Name | App | Requires | Aliases |
|---|---|---|---|
| `proton_experimental` | 1493710 | 4183110 | `proton-experimental` |
| `proton_11` | 4628710 | 4183110 | `proton-stable`, `proton-11.0`, `proton-10`, ... (26 in all) |
| `proton_10` | 3658110 | 1628350 | `proton-10.0-4pin` |
| `proton_9` | 2805730 | 1628350 | `proton-9.0-4pin` |
| `proton_hotfix` | 2180100 | 1628350 | `proton-hotfix` |
| `steamlinuxruntime_4` | 4183110 | | `steamrt4`, `steamrt4-any` |
| `steamlinuxruntime_sniper` | 1628350 | | `sniper` |
| `steamlinuxruntime_soldier` | 1391110 | | `soldier` |
| `steamlinuxruntime` | 1070560 | | `native`, `scout`, `SteamLinuxRuntime_scout` |
| `legacysteamruntime` | 4690330 | | `scout_legacy`, `legacy_steam_runtime` |

**Names.** A tool is named by its key or by one of its aliases. Names are
compared without regard to case: Valve's mappings write
`SteamLinuxRuntime_sniper` for the key `steamlinuxruntime_sniper`. If
two tools claim the same name, the key wins over an alias, and among
aliases the first tool in the list wins (*unverified*). A name no tool
claims names nothing.

**Getting it.** App 891390 and the tools are granted by package 0,
which the official client's cache shows for this account. The daemon
fetches 891390's info with the rest of the library (`pics.md` §5) and
follows its changes. *Unverified:* whether `ClientLicenseList` carries
package 0 for every account. If 891390 is not in the library, no tool
is known and no mapping applies: Linux apps run natively, and an app
without a Linux version fails to launch with `UNSUPPORTED`, with a
message that the compat-tool list is not available.

**Third-party tools.** The official client also reads
`compatibilitytool.vdf` files from `compatibilitytools.d` directories.
ossteam does not in M1 (SPEC §21, "Later").

## 2. Which tool an app gets (COMPAT-2)

An app's tool comes from mappings. Each mapping names a tool and has a
priority; the mapping with the highest priority wins. The official
client's log shows these sources and priorities:

| Source | Priority | Applies to |
|---|---|---|
| The user's choice for the app | 250 (*unverified*; from public reports of `config.vdf`) | the app |
| `extended/app_mappings/<n>` of app 891390 | 100 | `appid`, when `platform` is `linux` |
| `config/app_mappings/<n>` in the app's own info | the entry's `priority`, else 90 | the app, when `platform` is `linux` |
| An app that has a Linux version | 75, tool `native` | every app whose `common/oslist` contains `linux` |
| The default for other titles | 75 | every app without a Linux version |

A mapping does not replace one of the same or a higher priority. The
log records the skipped one ("Skip mapping AppID 730 to tool native with
priority 75: mapping to tool SteamLinuxRuntime_sniper with priority 100
already exists"). *Unverified:* which of two mappings of the same
priority wins; ossteam keeps the first in the order of the table.

A tool of the list (§1) gets no mapping: it runs as a program of its
`to_oslist`, and its depots are the host's (§4). (Tools' own info need
not name `linux` in `common/oslist`; without this rule the default would
map them to Proton.)

A mapping whose `tool` is empty, or names no tool (§1), is ignored. The
entries also have a `config` (always `none` in the cache) and a
`comment`. ossteam ignores both (*unverified* meaning of `config`).

Examples from the cache: Counter-Strike 2 (730) is mapped by 891390 to
`SteamLinuxRuntime_sniper` at 100 and by its own info at 90; Team
Fortress 2 (440) maps itself at 101; Spacewar (480) has no mapping and a
Linux version, so it gets `native`, which is the alias of
`steamlinuxruntime`, Steam Linux Runtime 1.0 (scout).

**The default for other titles.** The official client maps app 0 to
`proton_experimental` at priority 75 from its first start, before it has
the tool list, and uses that for every app without a Linux version.
ossteam's setting `compat.default_tool` holds this name and defaults to
`proton_experimental`. The empty string turns the default off: an app
that only Valve's mappings would run then has no tool, and a Windows-only
app cannot be launched. *Unverified:* that this is the official client's
default on every account rather than this machine's.

**The user's choice.** `compat.tool.<app id>` (one setting per app, §8)
names a tool or a selectable alias. It replaces every other mapping of
the app. The empty string, or no setting, means Valve's mapping. `none`
means no tool at all: the app runs natively (below), even where Valve
maps one, as for Counter-Strike 2 (730), which Valve maps to the Steam
Linux Runtime 3.0. `none` is ossteam's own choice; the official client
offers none like it (*unverified*). It is listed last among the choices
(§8).

**No tool.** An app with no mapping at all runs natively, as in
`launch.md`. An app mapped to a tool that runs programs of another
platform than any of its launch options cannot be launched (`UNSUPPORTED`,
naming the tool and the platforms).

## 3. Tool manifests

Every tool's install directory holds `toolmanifest.vdf`, text KeyValues
with one root key `manifest`:

| Key | Meaning |
|---|---|
| `version` | `1` (the default) or `2` |
| `commandline` | the tool's part of the command line (§4) |
| `require_tool_appid` | the app id of the tool this one runs inside |
| `use_tool_subprocess_reaper` | `1`: stopping the game sends `SIGINT` to the tool first (§6) |
| `compatmanager_layer_name` | the tool's layer in a stack: `container-runtime`, `scout-in-container`, `proton` |
| `filter_exclusive_priority` | groups tools of which only the highest is offered to the user |
| `unlisted` | as in §1 |

`require_tool_appid` is read from the manifest, not from the tool list.
They can differ: the tool list gives `steamlinuxruntime` (scout) no
requirement, while its manifest requires 1391110 (soldier). The manifest
describes the files that are installed, so it wins.

Observed on this machine:

```
SteamLinuxRuntime_sniper/toolmanifest.vdf:
  version 2, commandline "/_v2-entry-point --verb=%verb% --",
  use_tool_subprocess_reaper 1, compatmanager_layer_name container-runtime,
  filter_exclusive_priority 3

SteamLinuxRuntime/toolmanifest.vdf:
  version 2, commandline "/scout-on-soldier-entry-point-v2 --",
  require_tool_appid 1391110, compatmanager_layer_name scout-in-container,
  filter_exclusive_priority 10

LegacySteamRuntime/toolmanifest.vdf (2026-11-03):
  version 2, commandline "/legacy-steam-runtime",
  filter_exclusive_priority 10, unlisted 2
```

`legacysteamruntime` (4690330) is the scout runtime of older clients
without a container: `legacy-steam-runtime` is a shell script that pins
the libraries of its `steam-runtime/` directory with `setup.sh`, puts
them on `LD_LIBRARY_PATH` and `PATH`, and runs the game on the host. It
writes its pins and a `.last_runtime_version` file into its own install
directory, so that directory must stay writable. It needs nothing else
from the client, and runs as any version 2 tool (§5).

Proton's manifest is version 2 with `commandline` `/proton %verb%`
(*unverified* for the current versions; to be read from an installed
Proton during acceptance).

## 4. Installing tools (COMPAT-1)

Tools are apps of type `Tool` and are installed through the normal
install engine (`content.md`) into a library folder, like games, under
`apps/<installdir>`. ossteam never bundles them.

**The chain.** An app's chain is its tool, the tool its manifest
requires, and so on outwards. A tool that requires itself or a tool
already in the chain is an error. Before a launch, every tool in the
chain must be installed and up to date. A tool that is not installed is
installed, and one with an update is updated, as jobs that the launch
waits for: the `Launch` reply comes once they have ended and the game
has started, and the jobs show the progress in the meantime. A tool job
that fails fails the launch with `CONFLICT`, naming the job. The
manifest of a tool is read only after it is installed, so the chain is
completed one tool at a time. *Unverified:* whether the official client
installs tools in the game's library folder; ossteam uses the folder of
the app that first needs the tool, and finds an installed tool in any
folder.

Tools are installed when needed and are not uninstalled automatically.
They appear in the library as apps (they are granted by package 0) and
can be uninstalled like any app; an app whose chain needs one gets it
back at the next launch.

**Which depots.** An app's install has a platform (`content.md` §6,
rule 1): the platform its tool runs programs of (`from_oslist`), or the
host's when it has no tool. A Proton app on Linux therefore installs its
Windows depots. The platform is recorded with the install. If the app's
tool changes so that the platform differs from the installed one, the
app has an update known (`content.md` §10), and the update replaces the
depots. Launching before that is refused with `CONFLICT` and a message
that the app must be updated for the new tool. *Unverified:* whether
the official client switches depots in the same way; its per-user
`compat.vdf` holds a `platform_overrides` block, which suggests so.

## 5. Launching through tools

### 5.1 The command line

For each tool from the outermost to the innermost, its `commandline` is
written down with the first word, which starts with `/`, made absolute
by prefixing the tool's install directory, quoted in single quotes; the
rest is copied as it is. In version 2 manifests, `%verb%` becomes
`waitforexitandrun` for the game and `run` for setup commands (§7). The
game's own command line (`launch.md` §3) follows the innermost tool.
Everything is one `/bin/sh -c` command line under the reaper:

```
ossteamd --reaper AppId=<id> -- /bin/sh -c \
  "'<sniper>'/_v2-entry-point --verb=waitforexitandrun -- \
   '<proton>'/proton waitforexitandrun \
   '<install dir>/game.exe' <arguments> '<extra 1>' ..."
```

This matches the official client's log for Counter-Strike 2 (path
shortened): `reaper SteamLaunch AppId=730 -- '<sniper>'/_v2-entry-point
--verb=waitforexitandrun --  '<game>/cs2.sh' -steam`. The official client
also puts its `steam-launch-wrapper` in front, which closes inherited
file descriptors; ossteam's reaper does not leak descriptors in the
first place.

**Windows programs.** The executable of a Windows launch option is
given to Proton as a Linux path, like a native one. The option's
`arguments` follow it as written. *Unverified:* whether the official
client converts `\` in `arguments`.

**Working directory.** The launch option's `workingdir` under the
install directory when it has one, otherwise the install directory
itself; the tools keep it. (Valve's interface document gives Dota 2,
whose executable is under `game/` but which runs from its top-level
folder.) The same rule applies to native launches (`launch.md` §3).

**Launch options of the user.** The official client applies the user's
launch options, including `%command%`, around the whole chain. ossteam
has none in M1 (`launch.md` §3).

### 5.2 Environment (COMPAT-3)

On top of `launch-env.md` §1, a launch through a tool sets:

| Variable | Value |
|---|---|
| `STEAM_COMPAT_APP_ID` | the app id |
| `STEAM_COMPAT_INSTALL_PATH` | the app's install directory |
| `STEAM_COMPAT_DATA_PATH` | the app's prefix directory (§6) |
| `STEAM_COMPAT_CLIENT_INSTALL_PATH` | the generated client-install directory (§6.2) |
| `STEAM_COMPAT_LIBRARY_PATHS` | the library folders that hold the app and every tool of the chain, `:`-separated, without duplicates |
| `STEAM_COMPAT_TOOL_PATHS` | the install directories of the chain's tools, innermost first, `:`-separated |
| `STEAM_COMPAT_MOUNTS` | `<data>/steam-sdk`, the directory of ossteam's client library (§9), when it is not under the home directory |
| `STEAM_COMPAT_CONFIG` | the app's `config` from `app_compat_configs` (§5.3), when it has one |

`SteamAppId` and the other variables of `launch-env.md` §1 are set for
the game only, not for setup commands (§7), as Valve's interface
document requires: the Steamworks library treats every process with
`SteamAppId` as part of the game.

Proton's script fails without `STEAM_COMPAT_DATA_PATH` and reads
`STEAM_COMPAT_CLIENT_INSTALL_PATH` when it creates a prefix. The
container shares the paths in `STEAM_COMPAT_INSTALL_PATH`,
`STEAM_COMPAT_DATA_PATH`, `STEAM_COMPAT_CLIENT_INSTALL_PATH`,
`STEAM_COMPAT_LIBRARY_PATHS`, `STEAM_COMPAT_TOOL_PATHS` and
`STEAM_COMPAT_MOUNTS` read/write, and the home directory by default.

**Library paths.** The official client's paths are the `steamapps`
directory of each library, the one that holds `common/` and
`compatdata/`. ossteam's library folder holds its own `apps/` and
`compatdata/` directly (`content.md` §8), so the folder itself is
given. Proton's `steam.exe`
writes the parent of each path into the prefix's
`libraryfolders.vdf`, which is therefore not a usable library listing
for a Windows game that looks for other games; *unverified* that any
game does. Proton gives the game an `S:` drive, the library's parent,
only when the app's Proton config asks for `gamedrive`, and only when
the library path's last component is `steamapps`. ossteam's folder has
another name, so Proton logs "unable to use parent for game drive" at
every start, and an app that asks for `gamedrive` gets no `S:` drive
(seen 2026-11-03 with Proton Experimental; *unverified* which apps ask).

**Not set:**

| Variables | Why not |
|---|---|
| `STEAM_COMPAT_SHADER_PATH`, `STEAM_COMPAT_MEDIA_PATH`, `STEAM_COMPAT_TRANSCODED_MEDIA_PATH` | ossteam has no shader pre-caching and no media transcoding (FAITH-1) |
| `STEAM_COMPAT_FLAGS` | comes from app metadata whose location is *unverified* |
| `STEAM_COMPAT_SESSION_ID` | historical session mode only |
| `STEAM_COMPAT_LAUNCHER_SERVICE`, `STEAM_COMPAT_TRACING` | debugging and system tracing, which the user can set in the daemon's environment |
| `STEAM_COMPAT_EMULATOR`, `STEAM_COMPAT_GRAPHICS_PROVIDER`, `STEAM_COMPAT_MACHINE_ARCHITECTURE` | CPU emulation, which ossteam does not do |
| `STEAM_COMPAT_PROTON`, `STEAM_COMPAT_TOOL_MAPPINGS`, `STEAM_COMPAT_PROTON_SUPPRESS` | named in the official client's library, *unverified* meaning |

### 5.3 Per-app tool settings

`extended/app_compat_configs/<n>` of app 891390 gives, for `appid`, a
`config` and an `environment`, either or both:

- `config` is a comma-separated list of Proton options (`noesync`,
  `gamedrive`, `cmdlineappend:-changedir`, ...). It is passed unchanged
  as `STEAM_COMPAT_CONFIG`, which Proton's script parses.
- `environment` is `NAME=VALUE`, for example `DXVK_HDR=0`. It is set for
  the game when the chain contains Proton. A value that refers to
  another variable (`$LD_LIBRARY_PATH`) is expanded against the
  environment the game would otherwise get. *Unverified:* the format of
  more than one assignment, and whether it applies to SLR-only chains.

## 6. Prefixes and the client-install directory

### 6.1 The prefix

`STEAM_COMPAT_DATA_PATH` is `<library>/compatdata/<app id>` in the app's
library folder (SPEC COMPAT-3). The daemon creates the directory, empty,
before the first launch through a tool; Proton fills it (`pfx/`,
`version`, `tracked_files`, ...). Every chain gets one, SLR-only chains
included, as the official client gives CS2 a `compatdata/730`.

The prefix holds a Windows game's saves and settings. Uninstall leaves it
in place unless asked to remove it (`Uninstall{app_id,
remove_compat_data}`, CLI `ossteam uninstall <APP> --remove-compat-data`).
*Unverified:* what the official client does with it on uninstall.

### 6.2 The client-install directory

Proton reads `STEAM_COMPAT_CLIENT_INSTALL_PATH` when it builds a prefix:

- it copies `legacycompat/steamclient.dll`, `steamclient64.dll`,
  `Steam.dll`, `SteamService.exe` (as `steam.exe`) and
  `GameOverlayRenderer64.dll` into the prefix's
  `C:\Program Files (x86)\Steam\` when they exist;
- it puts `ubuntu12_64/video/` and `ubuntu12_32/video/` on the library
  path;
- its `steam.exe` lists the directory as a library in the prefix's
  `libraryfolders.vdf`.

None of these files is required: Proton's own `lsteamclient` registers
itself as the Steam client in the prefix's registry and loads the Linux
client library through `~/.steam/sdk64/steamclient.so` (or `sdk32`).
They are Valve's binaries, which ossteam does not have and must not
ship (RE-2). The generated directory is therefore
`<data>/compat-client/`, with an empty `legacycompat/` and nothing else.
*Unverified:* whether some Windows game needs Valve's
`steamclient.dll` in the prefix itself.

The container runtime compares `STEAM_COMPAT_CLIENT_INSTALL_PATH` with
the target of `~/.steam/root` and reports a difference as a
diagnostic. ossteam does not create `~/.steam/root` (DISC-2 allows only
the entries in `steamworks-abi.md` §1).

## 7. Setup commands and install scripts

A setup command runs through the same chain with `%verb%` set to `run`,
`STEAM_COMPAT_APP_ID` set and `SteamAppId` not set, and the working
directory set to the generated client-install directory (Valve's
interface document: not the game's directory). ossteam waits for it to
end before the next step.

Install scripts (`install-scripts.md`) of an app launched through Proton
apply to the prefix, not to the host. They are planned as on Windows
(`install-scripts.md` §3.1), against the prefix's registry:

- **Reading.** The daemon reads the prefix's registry from Wine's files:
  `pfx/system.reg` for `HKEY_LOCAL_MACHINE` (keys relative to
  `REGISTRY\Machine`) and `pfx/user.reg` for `HKEY_CURRENT_USER`. Key
  and value names are matched without regard to case.
- **Views.** When `system.reg` says `#arch=win64`, as Proton's prefixes
  do, a key under `HKEY_LOCAL_MACHINE\Software` is read and written at
  `Software\Wow6432Node` instead, where a 32-bit program's writes land
  (`install-scripts.md` §2.1). Proton's own records of redistributables
  are there (§2.2 of that page). `HKEY_CURRENT_USER` is not redirected.
- **Paths.** Variables (`install-scripts.md` §2.5) give the paths as
  programs in the prefix see them. A host path is written on the drive
  whose `pfx/dosdevices/<letter>:` links to `/` (`Z:` in Proton's
  prefixes), with `\` between components: `%INSTALLDIR%` and
  `%ROOTDRIVE%` so. `%WinDir%` is `C:\windows`; `%APPDATA%`,
  `%LOCALAPPDATA%` and `%USER_MYDOCS%` are under
  `C:\users\<user>`, the one directory under `pfx/drive_c/users` that is
  not `Public` (`steamuser`); `%COMMON_MYDOCS%` is
  `C:\users\Public\Documents`; `%STEAMPATH%` is `InstallPath` under
  `Software\Wow6432Node\Valve\Steam` in the prefix, which Proton sets
  (`C:\Program Files (x86)\Steam`). *Unverified:* which drive the
  official client's evaluator names the install directory with; Proton
  also maps the library folder as `S:`.
- **A new prefix.** Proton makes the prefix at the first verb it runs.
  When `pfx/system.reg` does not exist yet, the first setup command
  imports an empty registry file, so the prefix, with Proton's records,
  is there before anything is planned.
- **Registry.** The values a start writes go into one file,
  `<compatdata>/ossteam-script.reg`, in the form Windows' `regedit`
  exports (`Windows Registry Editor Version 5.00`, UTF-16LE with a byte
  order mark, so any character survives), and are imported with Wine's
  own `regedit /S <file>`, as one setup command. Wine's server writes the
  registry files only once its last process has gone, not when `regedit`
  exits, so the daemon cannot read the values back at once. It records
  the values given to a `regedit` that exits with 0 (`install-scripts.md`
  §5), and the next start, verify or uninstall reads what is really there:
  a recorded value that is missing is not deleted again.
- **Run Process.** Each entry that is due runs as a setup command:
  the program's path and the words of its arguments, each quoted for
  the shell. The arguments are a Windows command line, split as
  `CommandLineToArgvW` splits one, so that the shell neither splits nor
  expands them (unlike a game's, `launch.md` §3); Wine joins the words
  again. When it exits
  with 0, its `HasRunKey` value is set to 1 by a further `regedit`
  import. *Unverified:* that Proton's `steam.exe` starts `.cmd` and
  `.bat` files, and that `run` returns the program's exit code.
- **Firewall.** Wine has no Windows firewall. The step is not planned,
  and the log says so.

A setup command's output goes to the game's log (`launch.md` §3),
before the game's own.

There is no elevation inside a prefix: `HKEY_LOCAL_MACHINE` is the
prefix's own and belongs to this one app, so no step asks. A script
without a `kvsignatures` block gets no `HKEY_LOCAL_MACHINE` writes, as
on native Windows (`install-scripts.md` §2.6). The official client runs
its own Windows interpreter (`iscriptevaluator.exe`) through the tool
instead; that program is Valve's.

Verify and uninstall take the same removal in the prefix (one `regedit`
import of deletions), once the app's tool is installed; uninstall with
`--remove-compat-data` deletes the prefix instead.

SLR-only chains run Linux games, whose install scripts are not applied
(`install-scripts.md` §3).

## 8. Settings and IPC

| Key | Kind | Default | Effect |
|---|---|---|---|
| `compat.default_tool` | `STRING` | `proton_experimental` | the tool for apps without a Linux version that no other mapping covers (§2); empty for none |
| `compat.tool.<app id>` | `STRING` | empty | the user's tool for one app: a tool's name, a selectable alias, or `none` for no tool (§2); empty for Valve's mapping |

`compat.tool.<app id>` is a family of keys: `GetSettings` lists the ones
that have an override, and `SetSetting` accepts any decimal app id. A
value that names no tool is refused with `INVALID_ARGUMENT`. A change
that alters an installed app's platform (§4) marks it as having an
update, also when its depots are the same for both platforms; the
update then records the new platform.

`GetApp` gains `compat`, present on Linux:

| Field | Meaning |
|---|---|
| `tool` | the tool's name, app id and display name; absent when the app runs natively |
| `source` | `USER`, `VALVE` (891390), `APP` (the app's own info), `NATIVE` (a Linux version) or `DEFAULT` (§2) |
| `chain` | the chain's tools, outermost first, each with whether it is installed. A tool that is installed requires what its manifest says, one that is not what the tool list says (§1) |
| `choices` | the listed tools and selectable aliases whose `from_oslist` names a platform the app has launch options for; a launch option without an `oslist` stands for the platforms of `common/oslist` |
| `platform` | the platform its depots are installed for, when installed |

`compat` is unset off Linux, while the tool list is not available, and
for an app without info. `ossteam info <APP>` prints these; `--json`
gives `source` in lower case (`user`, `valve`, ...). `ossteam config set compat.tool.<APP>
<TOOL>` chooses a tool.

## 9. The shim inside a container

The SLR runs the game in a container with its own `/usr` and its own
`$XDG_RUNTIME_DIR` (`pressure-vessel-wrap` manual). The home directory,
`/dev/shm`, the network namespace and, by default, the process ID
namespace are shared. That affects the shim in two ways.

**Finding the library.** `steam_api` and Proton's `lsteamclient` load
`~/.steam/sdk64/steamclient.so` (`sdk32` for 32-bit), after resolving
links. The link chain of `steamworks-abi.md` §1.1 ends in ossteam's
install directory, which inside the container is visible only when it
is under the home directory. So `<data>/steam-sdk/sdk64/steamclient.so`
and `sdk32/steamclient.so` are copies of the libraries, not links. The
daemon replaces a copy that differs from its library at start, by
writing a temporary file and renaming it over the old one, so that a
running game keeps the file it has mapped. The libraries depend only on
the C library, which the container takes from the host when the host's
is newer.

**Reaching the daemon.** `$XDG_RUNTIME_DIR/ossteam/game.sock` does not
exist in the container. On Linux the daemon therefore also listens on
the abstract Unix socket `ossteam-game-<uid>` (no file; the name is
scoped to the network namespace, which the container shares). The shim
tries the socket file first and the abstract socket second. An abstract
socket has no file permissions, so both sides check the other's user
with `SO_PEERCRED`: the daemon refuses a peer of another user (IPC-2),
and the shim refuses a listener of another user, so a name taken first
by another user only makes `steam_api` find no client.
`Hello.pid` is still checked against the peer's pid; that holds while
the container shares the process ID namespace, its default. A container
started with `--unshare-pid` gets `Refused`.

**The private `~/.steam`.** The tools of the chain run inside the
launch's own namespace (`steamworks-abi.md` §1.1.2), so the container is
built from a home directory whose `~/.steam` is `<data>/steam-sdk`,
read-only. *Unverified:* that `pressure-vessel` carries that mount into
the container, and that its `bwrap` nests in the user namespace
(§1.1.7).

## 10. Stopping a game

`StopGame` (`launch.md` §5) sends `SIGTERM` to every process of the tree
and `SIGKILL` after 10 s. When the outermost tool's manifest has
`use_tool_subprocess_reaper` set, it first sends `SIGINT` to the
outermost tool's process only, the reaper's child, which lets the
container stop its own tree (Valve's interface document); the
`SIGKILL` timeout is the same. *Unverified:* the official client's
timeout.

## 11. Verification against the official client

| Item | Status |
|---|---|
| The tool list's keys and the names (§1) | from the official client's app-info cache |
| Package 0 in `ClientLicenseList` | unverified |
| Mapping priorities 100, 90, 75 and the skip rule (§2) | from `compat_log.txt` |
| The user's priority 250 and ties | unverified |
| `compat.default_tool` default | from `compat_log.txt` on one machine |
| `toolmanifest.vdf` keys (§3) | from installed tools and Valve's interface document |
| Proton's manifest | unverified; read it from an installed Proton |
| Tool library folder and depot switch (§4) | unverified |
| Command line shape (§5.1) | from `gameprocess_log.txt` for an SLR chain; a Proton chain is unverified |
| Working directory (§5.1) | from Valve's interface document |
| Environment values (§5.2) | names from the official client's library and Valve's document; values unverified until a capture |
| `app_compat_configs` handling (§5.3) | `config` from Proton's script; `environment` unverified |
| Install scripts in the prefix (§7) | ossteam's design |
| The shim inside a container (§9) | from the `pressure-vessel-wrap` manual; to be run in acceptance |
