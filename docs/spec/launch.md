# Launching games

| | |
|---|---|
| Status | Draft for M1c; parts observed in the official Linux client's logs (§9), the rest unverified |
| Requirements | SPEC §11.1 and §11.3 (LAUNCH-1 to LAUNCH-5), PLAY-1, §13.2, D-4, D-5 |
| Code | `crates/ossteam-launch` (options, command lines, SteamStub detection, processes); the daemon's side in `crates/ossteamd/src/launch.rs` and `crates/ossteamd/src/core/games.rs` |

This page covers how ossteam turns an installed app into a running
process: which launch option it uses, what it checks first, the command
line, how it watches the game and how it stops it, and how it reports
the game to Steam. The environment the game gets is in `launch-env.md`.
The entries that let the game's `steam_api` find a client are in
`steamworks-abi.md` §1. Compatibility tools (Proton, the Steam Linux
Runtime) are in `compat-tools.md`, which builds on this page: on Linux,
an app that Valve's mappings give a tool is launched through it.

**Verification.** The official Linux client writes each launch's
command line and its tracked processes to its logs
(`logs/console-linux.txt`, `logs/gameprocess_log.txt`), and those show
the shape described in §3 and §5. What the logs do not show is marked
*unverified*. It must be checked against the official client (§9)
before anything relies on it.

## 1. Launch options (LAUNCH-1)

An app's launch options are the entries under `config/launch` in its
app info (`pics.md` §4). Each key is a decimal index. ossteam reads:

| Key | Meaning |
|---|---|
| `executable` | the program, relative to the install directory |
| `arguments` | the arguments, as one string (§3) |
| `workingdir` | the working directory, relative to the install directory; optional |
| `description` | the text shown when the user chooses; `description_loc/<language>` holds translations |
| `type` | `default`, `option1`, `option2`, `option3`, `none`, `safemode`, `multiplayer`, `config`, `vr`, `server`, `editor`, `manual`, `benchmark`, `openvroverlay`, `osvr`, `openxr`, `othervr`, `dialog` (*unverified*: whether the list is complete) |
| `config/oslist` | comma-separated platforms: `windows`, `macos`, `linux` |
| `config/osarch` | `32` or `64` |
| `config/betakey` | the branch the option is for |
| `config/ownsdlc` | comma-separated DLC app ids the account must own (*unverified*: the key's name and whether the list means all or any; ossteam requires all) |
| `config/realm` | `steamglobal` or `steamchina` (*unverified*) |

Keys are matched without regard to case, as KeyValues keys are. Path
separators in `executable` and `workingdir` may be either `/` or `\`,
whichever platform the option is for. ossteam treats both as separators.

**Which options qualify.** An option qualifies for a launch when all of
these hold:

1. its `oslist` is empty or names the platform the game will run on
   (`linux` or `windows` for a native launch; `macos` in M1e);
2. its `osarch` is empty, or `32`, or `64` on a 64-bit OS;
3. its `betakey` is empty or names the installed branch;
4. the account owns every DLC in `ownsdlc`;
5. its `realm` is empty or `steamglobal`. ossteam connects to the
   global realm only.

*Unverified:* when options for both `32` and `64` qualify on a 64-bit
OS and are otherwise the same, whether the official client hides the
32-bit ones. ossteam lists both and lets the user choose.

*Unverified:* whether the official client hides any `type` from its
choice dialog (`none` is the likely one). ossteam hides nothing, and
`GetLaunchOptions` reports each option's type, so a client can decide.

**Choosing.** If exactly one option qualifies, a launch uses it. If more
than one qualifies, the leaf client asks the user to choose, as the
official client does, and passes the chosen index. A `Launch` request
without an index is then refused with `INVALID_ARGUMENT`, and the
message lists the indexes. If none qualifies, the launch is refused with
`UNSUPPORTED`, and the message names the platforms the app has options
for.

## 2. Checks before a launch (LAUNCH-3)

`Launch{app_id, option, extra_args, allow_stale}` checks these in order
and stops at the first that fails:

| Check | Error |
|---|---|
| An account is active | `NOT_LOGGED_IN` |
| The app is in the account's library | `NOT_OWNED` |
| The app is installed | `NOT_FOUND` |
| The app is not already running | `CONFLICT` |
| No unfinished job (install, update, verify, uninstall) is working on the app | `CONFLICT` |
| The app has no update known (`content.md` §10), unless `allow_stale` | `CONFLICT`, with a message that the build is out of date and `allow_stale` launches it anyway |
| The launch option qualifies (§1) | `INVALID_ARGUMENT` for an index that does not exist or does not qualify; `UNSUPPORTED` when none qualifies |
| The executable exists under the install directory and does not leave it | `NOT_FOUND`, with a message that the app needs a verify |
| The executable is not protected by SteamStub (§7) | `UNSUPPORTED` |
| Linux: the app's compatibility tool, if it has one, runs programs of the option's platform, and the install is for that platform (`compat-tools.md` §2, §4) | `UNSUPPORTED`; `CONFLICT` when the app must be updated for its tool |
| The discovery entries are in place (`steamworks-abi.md` §1) | `CONFLICT` when an official Steam client owns them (DISC-3); `UNSUPPORTED` when ossteam's client library is not installed next to the daemon |

On Linux, the launch options that qualify are those of the platform the
app's tool runs programs of (`windows` for Proton), or of `linux` when
it has none. Once the checks pass, missing or outdated tools of the
chain are installed before the game starts (`compat-tools.md` §4).

The account need not be logged on at that moment. A game started while
the connection is down runs, and its play session is reported once the
session is back (§6). The game's `steam_api` sees the client as logged
off until then (NET-4).

*Unverified:* the official client also synchronises cloud files and
stats, shows notices and runs install scripts in this phase. Its log
names the steps `UpdatingAppInfo`, `RunningInstallScript`,
`SynchronizingCloud`, `SynchronizingStats`, `ShowInterstitials`,
`SiteLicenseSeatCheckout` and `CreatingProcess`. Cloud is M3. Install
scripts at launch are part of M1c, with `install-scripts.md` §3.

## 3. The command line

**Linux and macOS.** The official Linux client runs one `/bin/sh -c`
command line for each launch. In it, the executable's absolute path is
quoted in single quotes, the option's `arguments` follow as written,
without quoting, and arguments the user adds are each quoted in single
quotes. The shell therefore splits and expands `arguments` itself,
including quotes and variables. ossteam does the same. The command line
for the shell is:

```
'<install dir>/<executable>' <arguments> '<extra 1>' '<extra 2>' ...
```

A single quote inside a quoted word is written `'\''`.

**Windows.** The command line given to `CreateProcessW` is the
executable's absolute path in double quotes, then `arguments` as
written, then the user's arguments, each quoted by the rules that
`CommandLineToArgvW` reverses. *Unverified:* that this matches the
official Windows client.

**Working directory.** It is `workingdir` under the install directory
when the option gives one. Otherwise it is the install directory itself,
even when the executable is in a subdirectory. Valve's compat-tool
interface document states this for the official client, with Dota 2 as
its example (`compat-tools.md` §5.1).

**Standard streams.** Standard input is the null device. Standard output
and standard error go to `<state>/logs/games/<app id>.log`, which is
replaced at each launch. The official client sends them to its own
output.

**User launch options.** The official client lets the user set launch
options per app, including a `%command%` template. ossteam has no such
setting in M1. Its extra arguments are always appended.

## 4. Supervision (LAUNCH-4)

A game often starts as a launcher script or stub that starts the real
game and exits. The play session lasts until the last process of the
tree is gone, not until the first one exits.

**Linux.** The official client runs every game under `reaper`, a small
program that makes itself a child subreaper (`PR_SET_CHILD_SUBREAPER`),
starts the game and waits until it has no children left. Orphans in the
game's tree are re-parented to `reaper` rather than to `init`, so it
sees them exit. Its command line names the app
(`reaper SteamLaunch AppId=<id> -- <command>`), so the tree can be found
by name. ossteam does the same with its own program:
`ossteamd --reaper AppId=<id> -- /bin/sh -c <command line>`. The reaper:

1. makes itself a child subreaper;
2. starts the command in a new process group, with its standard streams;
3. waits for children until `ECHILD`, keeping the status of the
   command's own process;
4. exits with that status. When that process ended by a signal, the
   reaper resets the signal's disposition, turns off core dumps and
   raises it on itself, so the daemon sees the same signal.

The daemon has one supervisor thread per game (CONC-1), which waits for
the reaper.

**Windows.** The daemon creates the game suspended, puts it in a job
object of its own and then resumes it. The job is not given
`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, so games outlive the daemon
(D-5). Children are in the job unless they break away, which the job
does not allow. The supervisor thread waits on the job's completion
port for `JOB_OBJECT_MSG_ACTIVE_PROCESS_ZERO`. The exit status is that
of the process the daemon created. *Unverified:* how the official
Windows client tracks the tree.

**macOS.** Darwin has no subreaper. The game runs in its own process
group. The supervisor waits for the process the daemon started, then
checks once a second whether any process of the group remains. Processes
that leave the group are not tracked. This is enough to compile and
test until M1e establishes the official client's behaviour.

**Processes the shim reports.** The official client also tracks
processes that connect through its client library with the app's id.
Its log adds them as tracked processes without a command line. ossteam
does the same once the shim exists: a game process that connects over
the game IPC (`steamworks-abi.md`) with the app id is tracked, and the
session lasts while it runs as well.

**After a daemon restart.** A game outlives a daemon that stopped with
`force` or crashed. A daemon that starts again does not adopt it. It is
not listed by `ListRunning` and is not reported as played. *Unverified:*
what the official client does.

## 5. Stopping a game (LAUNCH-5)

`StopGame{app_id}` asks the game to close and then waits:

- **Linux:** `SIGTERM` to every descendant of the reaper, found through
  `/proc`, then `SIGKILL` to every one left after 10 s.
- **macOS:** `SIGTERM` to the game's process group, then `SIGKILL`
  after 10 s.
- **Windows:** `WM_CLOSE` posted to each top-level window owned by a
  process in the job, then `TerminateJobObject` after 10 s. A game
  without windows is terminated at once.

*Unverified:* the signals, the order and the 10 s timeout the official
client uses.

`StopGame` replies once the request has been made. The game's end
arrives as `GameExited` (§8). Stopping a game that is not running is
`NOT_FOUND`.

The daemon refuses `Shutdown` without `force` while a game it launched
runs (`CONFLICT`, D-5). It also refuses `Logout` and `SwitchAccount`
then (`CONFLICT`), since the game's session belongs to the account. With `force` it stops, and the games keep
running. With `daemon.exit_when_idle` (D-4), a running game keeps it
alive.

## 6. Play sessions (PLAY-1)

While at least one game runs, the daemon tells Steam which games are
being played. It sends `CMsgClientGamesPlayed` with EMsg
`ClientGamesPlayedWithDataBlob` (5410). *Unverified:* which of 5410,
`ClientGamesPlayed` (742) and `ClientGamesPlayedNoDataBlob` (715) the
official client sends. The message has one `games_played` entry per
running game:

| Field | Value |
|---|---|
| `game_id` | the app's game id: the app id, with the type and mod bits zero |
| `process_id` | the pid of the process the daemon started (the reaper on Linux) |
| `owner_id` | the account id of the account that owns the license, which is the active account in M1 |
| `game_build_id` | the installed build |
| `beta_name` | the installed branch, when it is not `public` |
| `game_os_platform` | not sent; *unverified* |
| `launch_option_type`, `launch_source`, `game_flags` | not sent; *unverified* |

The message is sent again with every change: when a game starts or
exits, and after every logon, because the server forgets it with the
connection. When the last game exits, the daemon sends an empty
`games_played` list, which ends the session.

*Unverified:* which other fields the official client fills, and whether
it reports anything before the process starts. Playtime and in-game
presence come from this message alone. Nothing else reports them.

## 7. SteamStub (SPEC §13.2)

SteamStub wraps a Windows executable. The wrapper code runs first, talks
to the client and then decrypts the game's code. Every SteamStub variant
that has been described adds a section named `.bind` to the PE image.
Before a launch the daemon reads the executable's PE headers. An
executable with a `.bind` section is refused with `UNSUPPORTED`, and the
message says that Steam DRM support arrives in M2 (SPEC §13). Nothing in
the file is changed (DRM-2).

The check reads only headers: the DOS header's `e_lfanew`, the PE
signature, the COFF header and the section table. A file that is not a
PE image (an ELF or Mach-O executable, a shell script) passes the check.
*Unverified:* whether any Linux or macOS executable carries SteamStub.
None is known.

A launcher that starts a protected executable is not caught by this
check. The protected process then asks the client questions that the
shim does not answer, and it fails under ABI-3.

## 8. IPC (`ipc.md` §5.7)

| Method | Behaviour |
|---|---|
| `GetLaunchOptions{app_id}` | The app's options as §1 reads them, each with `qualifies` set for this OS, branch and account, and `required` set when exactly one qualifies. `NOT_FOUND` when the app is not in the library. |
| `Launch{app_id, option, extra_args, allow_stale}` | Runs the checks in §2, starts the game and replies with the running game. `option` unset means the only qualifying option. When compatibility tools must be installed first (`compat-tools.md` §4) the reply waits for their jobs, however long they take; the client library waits for this reply without a timeout. |
| `ListRunning` | The games the daemon launched that are still running. |
| `StopGame{app_id}` | §5 |

A running game carries its app id, the option index, the pid the daemon
started, the time it started (Unix seconds) and the path of its log
file.

Events on `GAMES`: `GameStarted{game}` after the process is created,
and `GameExited{app_id, exit_code | signal, seconds}` after the last
process of the game is gone and its connections to the game endpoint
have closed, at most 2 s later (`steamworks-abi.md` §3.5). Just before
`GameExited` comes `ShimReport` when any of the game's processes loaded
ossteam's Steamworks library (`steamworks-abi.md` §4). Exactly one of `exit_code` and `signal` is
set: the exit code of the process the daemon started, or the signal
that ended it. On Windows, `exit_code` is the process's exit code as an
unsigned 32-bit number.

## 9. Verification against the official client

The logs of an official Linux client (build 1791918634) show:

- the command is run through `/bin/sh -c`;
- the executable's path is single-quoted, the app info's arguments are
  not quoted, and command-line arguments added for the launch are each
  single-quoted;
- the command is wrapped as `reaper SteamLaunch AppId=<id> -- ...`;
- the launched process and a second process are tracked: the second is
  added without a command line, and its exit is logged with exit code
  -1;
- the app is removed from the running list when the last tracked process
  is gone.

| Item | Status |
|---|---|
| The keys under `config/launch` beyond `executable`, `arguments`, `description`, `type`, `oslist`, `osarch`, `betakey` | unverified |
| Hiding of types and of 32-bit options (§1) | unverified |
| The working directory (§3) | from Valve's compat-tool interface document |
| The Windows command line (§3) | unverified |
| Tree tracking on Windows (§4) | unverified |
| Stop signals and timeout (§5) | unverified |
| The EMsg and fields of the play-session message (§6) | unverified; needs a capture |
| `.bind` as the SteamStub marker (§7) | from public analyses of SteamStub; to be checked on a protected title |
