# The Steamworks client ABI

| | |
|---|---|
| Status | M1c; macOS discovery (§1.3) M1e. §1 (discovery entries), §2 (the library), §3 (game IPC) and §4 (ShimReports) implemented, except §1.1.2 to §1.1.6 (the private `~/.steam`, taking over the official entries, telling the user), drafted 2026-11-03 and not reviewed; the interface pages in `steamworks/` grow with the shim |
| Requirements | SPEC §12 (ABI-1 to ABI-5, GIPC-1 to GIPC-3, DISC-1 to DISC-6), PLAY-1, LAUNCH-4 |
| Code | discovery entries in `crates/ossteam-launch/src/discovery/`; the library in `crates/ossteam-steamclient`; the game IPC in `crates/ossteam-gameipc`; the daemon's side in `crates/ossteamd/src/launch.rs`, `crates/ossteamd/src/gameipc.rs` and `crates/ossteamd/src/core/{games,shim}.rs` |
| Interfaces | one page each under `docs/spec/steamworks/`; the vtables in `docs/spec/steamworks/vtables.txt` (§2.4) |

A game links Valve's `steam_api` library and ships it. `steam_api` finds
a running client, loads that client's library (`steamclient.so`,
`steamclient.dll` or `steamclient64.dll`, `steamclient.dylib`) and talks
to it through versioned interfaces. `ossteam-steamclient` is ossteam's
client library.

## 1. Discovery entries (DISC-1 to DISC-6)

`<data>` and `<config>` are ossteam's data and configuration
directories (SPEC §17): on Linux `$XDG_DATA_HOME/ossteam` (by default
`~/.local/share/ossteam`) and `$XDG_CONFIG_HOME/ossteam` (by default
`~/.config/ossteam`).

**Verification.** §1.1 comes from the official Linux client's files in
the home directory, from its `steam.sh` (read for facts, §1.1.3), and
from a shipped `libsteam_api.so`: the strings `%s/.steam/%s` and
`HOME`, its import of `getenv` (and none of `getpwuid`), the message
that `SteamAPI_IsSteamRunning()` found no running client, and the
message that a local `steamclient.so` was tried first. Only one
`libsteam_api.so` was read (a 32-bit build of an older SDK); that newer
ones also build the path from `$HOME` is *unverified*. How `steam_api`
checks that the client is running is *unverified*. The Windows entries
in §1.2 are *unverified*.

### 1.1 Linux

The official client keeps these in `~/.steam`:

| Entry | What it is |
|---|---|
| `sdk64` | symlink to the directory that holds the 64-bit `steamclient.so` (`<Steam>/linux64`) |
| `sdk32` | symlink to the directory that holds the 32-bit `steamclient.so` (`<Steam>/linux32`) |
| `steam.pid` | the client's pid in decimal, without a newline |
| `steam`, `root`, `bin`, `bin32`, `bin64` | symlinks into the client's installation |
| `registry.vdf`, `steam.pipe`, `steam.token` | the client's own state; not read by `steam_api` (*unverified*) |

`steam_api` first tries a `steamclient.so` next to itself, then
`$HOME/.steam/sdk64/steamclient.so` (or `sdk32` for a 32-bit game).
Before that it checks for a running client through `~/.steam/steam.pid`
(*unverified*: that it checks the pid names a live process).

A game finds ossteam in one of two ways:

- **A private `~/.steam`** (§1.1.2): every process the daemon starts
  for a launch sees a `~/.steam` of ossteam's own, whatever the real
  one holds. This is how launches find ossteam whenever the system
  allows it, and it never touches the real `~/.steam`.
- **The real entries** (§1.1.1): `sdk64`, `sdk32` and `steam.pid` in the
  real `~/.steam`, made while they are free. They let a process ossteam
  did not start, such as a game run from a terminal, find ossteam, and
  they are how launches find it when no private `~/.steam` can be made.

#### 1.1.1 The real entries (DISC-1)

ossteam creates only `sdk64`, `sdk32` and `steam.pid`:

- `~/.steam/sdk64` and `~/.steam/sdk32` are symlinks to
  `<data>/steam-sdk/sdk64` and `<data>/steam-sdk/sdk32`. Those
  directories belong to ossteam, and each holds a `steamclient.so` that
  is a copy of the matching library (§1.4), not a link: a game in a
  Steam Linux Runtime container sees the home directory but not
  ossteam's install directory (`compat-tools.md` §9). The daemon
  replaces a copy that differs from the library by writing a temporary
  file and renaming it, so that a running game keeps the file it has
  mapped.
- `~/.steam/steam.pid` holds the daemon's pid.

`~/.steam` is created with mode 0755 when it does not exist, as the
official client creates it.

An entry is ossteam's when `sdk64` or `sdk32` is a symlink whose target
is the matching directory under `<data>/steam-sdk`. `steam.pid` is
ossteam's when `sdk64` is. ossteam writes `steam.pid` last and removes
it first.

The entries are **free** when none of the three exists, or all that
exist are ossteam's. The daemon makes them only then. When they belong
to an official client, it leaves them alone (§1.1.4), unless the user
has turned on `discovery.take_over_official` (§1.1.5).

#### 1.1.2 A private `~/.steam` for each launch (DISC-4)

`<data>/steam-sdk` holds, beside `sdk64/` and `sdk32/`, a `steam.pid`
with the daemon's pid, written and removed with the real entries
(§1.5). It is a complete `~/.steam` as `steam_api` needs one, and
nothing else.

Every process the daemon starts for a launch, the game and the
compatibility tools and setup commands of its chain (`compat-tools.md`
§4, §7), runs in a mount namespace of its own in which `<data>/steam-sdk`
is mounted over `~/.steam`. The daemon's own process, and every process
it did not start, keep the real `~/.steam`. A process inherits the
namespace, so whatever the game starts sees the private `~/.steam`
too, for as long as it runs.

In the child, after `fork` and before `exec`, in this order:

1. `unshare(CLONE_NEWUSER | CLONE_NEWNS)`.
2. Write `deny` to `/proc/self/setgroups`, `<uid> <uid> 1` to
   `/proc/self/uid_map` and `<gid> <gid> 1` to `/proc/self/gid_map`, so
   that the process keeps its own user and group.
3. Make every mount private (`mount(NULL, "/", NULL, MS_REC |
   MS_PRIVATE, NULL)`), so that nothing done in the namespace reaches
   the host.
4. Bind `<data>/steam-sdk` onto `~/.steam` (`MS_BIND`), then remount
   that bind read-only (`MS_REMOUNT | MS_BIND | MS_RDONLY`), keeping the
   flags of the mount it comes from, which a user namespace may not
   clear.
5. For each official root (below): when `<root>/userdata` is a
   directory, bind `<data>/userdata` onto it (`MS_BIND`, writable).
6. For each official root: when `<root>/config` is a directory, bind
   `<data>/empty` onto it and remount that bind read-only, as in step 4.

These are system calls only, as the code between `fork` and `exec`
must be. The process ID namespace is shared, so the daemon's pid in
`steam.pid`, the `Hello.pid` check (§3.1) and the supervision of the
game's processes (`launch.md` §4) are unchanged. The network namespace
is shared, so the abstract socket (§3.1) is reached as before.

`~/.steam` must exist as the mount point. The daemon creates it with
mode 0755 when it does not exist, as for the real entries. When it is a
symlink, the bind covers the directory it points to, in the game's
namespace only. When it exists and is not a directory, no private
`~/.steam` can be made.

What the game sees in `~/.steam` is `sdk64/`, `sdk32/` and `steam.pid`,
read-only. It does not see the official client's links (`root`,
`steam`, `bin*`), its `registry.vdf` or its `steam.pipe`, so it cannot
reach the official client through them.

**The official installation.** The official client keeps its accounts'
data in `<root>/userdata/<account id>/` and their names in
`<root>/config/loginusers.vdf`. Steps 5 and 6 keep a game under
ossteam's account from reading them, and leave the rest of the
installation (its library folders, the content other games read)
visible:

- In `<root>/userdata` the game sees ossteam's own
  `<data>/userdata/<account id>/<app id>/` (`user_dir`, §3.4), the
  same tree `GetUserDataFolder` and `ISteamRemoteStorage` serve, laid
  out as the official client's. It is writable, so a game that writes
  into `remote/` or `local/` directly writes into ossteam's copy. What
  only the official client keeps there (`config/localconfig.vdf`,
  screenshots and the like) is absent.
- `<root>/config` is an empty directory, read-only. `<data>/empty` is
  a directory of ossteam's that it keeps empty.

The **official roots** are found by the daemon before the fork, as
canonical paths, each once: the targets of the real `~/.steam/root` and
`~/.steam/steam` when they are symlinks, and those of
`~/.local/share/Steam`,
`~/.var/app/com.valvesoftware.Steam/.local/share/Steam` (Flatpak) and
`~/snap/steam/common/.local/share/Steam` (Snap) that exist. A root
under `~/.steam`, such as Debian's `~/.steam/debian-installation`, needs
neither step: step 4 has already hidden it. ossteam never creates
anything in an official root; a root without `userdata/` or `config/`
gets no bind for it. The daemon creates `<data>/userdata` and
`<data>/empty` before the first launch.

The rest of the home directory stays as it is.

In the namespace the user is the same, but setuid and setgid programs
do not gain privileges, `setgroups` is refused, and files owned by
users other than the process's own appear to belong to the overflow
user (`nobody`).

**Whether it can be made.** At start, the daemon forks a child that
does steps 1 to 4 with a directory of ossteam's own as the mount point,
and exits; the result is kept as the private `~/.steam`'s **state**:
available, or not available with the step that failed and its error.
Each launch does the steps again. When a launch's steps fail although
the state was available, the launch fails (below), the state becomes
not available with that failure, and the daemon sends a warning
`Notice` (§1.1.6). It never quietly starts the game without its private
`~/.steam`. The state is checked again at the next start of the daemon.

When the state is not available, the daemon adds what it can read about
why to the failure, each when it is so:

- `/proc/sys/kernel/unprivileged_userns_clone` is 0 (Debian's switch);
- `/proc/sys/user/max_user_namespaces` is 0;
- `/proc/sys/kernel/apparmor_restrict_unprivileged_userns` is 1
  (Ubuntu 23.10 and later), unless the daemon is confined by an
  AppArmor profile that allows `userns`;
- `/.flatpak-info` exists (the daemon runs inside Flatpak, whose
  sandbox refuses new user namespaces).

Packages for distributions that restrict unprivileged user namespaces
with AppArmor ship a profile for `ossteamd` that allows `userns`.

**Launches without it.** When no private `~/.steam` can be made, a
launch finds ossteam only through the real entries, and the game sees
the official installation as any other process of the user does. The
launch check (`launch.md` §2) then needs the real entries to be ossteam's, and fails
with `CONFLICT` when an official client owns them (§1.1.4). When a
launch's own steps fail, the launch fails with `UNSUPPORTED`, with the
step and its error in the message.

#### 1.1.3 The official client

These are facts from the official client's `steam.sh`, read for its
behaviour; none of its code is used:

- It counts the client as running when `~/.steam/steam.pid` names a
  live process that has `~/.steam/steam.pipe` open (a link in
  `/proc/<pid>/fd` to it). Any other pid, ossteam's daemon included,
  is a stale file to it.
- When the client is not running by that test, or one of its links in
  `~/.steam` is missing or dangling, it recreates `sdk32`, `sdk64`,
  `steam`, `root`, `bin32` and `bin64` with `ln -fsn`, replacing
  whatever was there, ossteam's links included.
- It does not write `steam.pid`; the client does, after the script has
  made the links. *Unverified:* that the client replaces a `steam.pid`
  that names a live process other than itself.
- The links and `steam.pid` stay when the client exits.

So an official client that starts while ossteam has the real entries
takes them over without asking, and an official client that has exited
still owns them. ossteam uses the same test to tell whether the
official client runs: **the official client runs** when `steam.pid` is
not ossteam's and names a live process with `~/.steam/steam.pipe` open.

#### 1.1.4 Conflict (DISC-3)

If `sdk64`, `sdk32` or `steam.pid` exists and is not ossteam's, the
entries belong to an official client, whether or not it is running,
because its links stay when it exits (§1.1.3). The daemon then leaves
all three alone, and never removes, renames or replaces them, except
under `discovery.take_over_official` (§1.1.5).

With a private `~/.steam` available, that changes nothing for
launches: they find ossteam in their own `~/.steam`, and the official
client finds its own entries. Only processes ossteam did not start find
the official client.

Without one, a launch fails with `CONFLICT`. The message names the
entry, says that an official Steam client owns it, gives the reason no
private `~/.steam` could be made (§1.1.2), and names the ways out:
allow unprivileged user namespaces (or install the AppArmor profile);
quit the official client and remove its `~/.steam/sdk32`, `sdk64` and
`steam.pid`, which it recreates when it starts; or turn on
`discovery.take_over_official`.

#### 1.1.5 Taking over the official entries (DISC-5)

`discovery.take_over_official` (`ipc.md` §5.4), off by default, lets
ossteam use the real `~/.steam` while the official client is not
running, so that processes ossteam did not start find it then, and so
that launches work without a private `~/.steam`. It is Linux only.

**Taking over.** While the setting is on and an account is active, the
daemon takes the entries over when all of these hold:

- the entries belong to an official client (§1.1.4);
- the official client does not run (§1.1.3);
- no process of the user has a `steamclient.so` mapped from the
  directory that the official `sdk64` or `sdk32` points to (a game the
  official client started can outlive it, and what that game starts
  next would otherwise find ossteam);
- nothing in `~/.steam` has been created, renamed or removed by anyone
  but ossteam in the last 30 s (the race, below).

It then records the official entries in `<config>/official-entries.toml`
(the targets of `sdk64` and `sdk32`, and the text of `steam.pid`, each
when it exists), and replaces them with its own: each link by creating
the new one under a temporary name and renaming it over the old one,
`steam.pid` last. The conditions are checked again just before each
rename, and the daemon stops at the first that no longer holds.

**Handing back.** Where §1.5 removes the real entries (a logout, the
daemon's exit, ossteam's own stale entries at start), and when the
setting is turned off, the daemon puts back what
`official-entries.toml` records, by the same renames, `steam.pid`
first, then deletes `official-entries.toml`. An official client that
never starts again therefore finds its entries as it left them.

**Yielding.** The daemon watches `~/.steam` (inotify) while it holds
the entries. When one of them changes and is no longer ossteam's, the
official client is starting, or the user changed it. The daemon then
writes nothing more to `~/.steam`, deletes `official-entries.toml` (the
official client has made its own entries), sends a warning `Notice`
(§1.1.6), and takes over again only when the conditions above hold
again. Without a private `~/.steam`, launches fail with `CONFLICT`
meanwhile, as in §1.1.4.

**Risks.** Turning the setting on accepts these, and §1.1.6 says so to
the user:

- **Games still running when the official client starts.** A game
  ossteam started keeps working, since it has loaded ossteam's library
  and holds its connection. But without a private `~/.steam`, what it
  starts from then on (a launcher's real game, a game restarting itself)
  finds the official client, and that client's account. With a private
  `~/.steam` this cannot happen.
- **The race with the official client's start.** `steam.sh` makes the
  links some time before the client writes `steam.pid` and opens
  `steam.pipe`. A take-over in that window replaces the links the
  official client has just made, and the games it then starts find
  ossteam and play under ossteam's account. The 30 s wait and the
  checks before each rename narrow that window; nothing closes it,
  because nothing tells ossteam that `steam.sh` has begun.

The setting does not change Windows or macOS (§1.2, §1.3), where an
official client's entries are never taken over.

#### 1.1.6 Telling the user (DISC-6)

The user is told when no private `~/.steam` can be made, and what the
take-over setting risks, wherever they would look:

- **Daemon.** At start, when the private `~/.steam` is not available,
  a warning in the log with the failed step, its error and the reasons
  of §1.1.2, and the same as one line on standard error:

  ```
  ossteamd: warning: games get no private ~/.steam (<reason>); while an official Steam client owns ~/.steam, games cannot be launched
  ```

  The part after the semicolon is there only while launches fail
  because of it. When `discovery.take_over_official` is on, the daemon
  also writes, at start and when the setting is turned on:

  ```
  ossteamd: warning: discovery.take_over_official is on: <the risks of §1.1.5, in one sentence each>
  ```

  Taking over, handing back and yielding are logged as warnings too.
- **`GetStatus`** (`ipc.md` §5.1) carries `game_isolation`: whether a
  private `~/.steam` is available, the reason when it is not, whether
  launches fail because of it (not available, the entries belong to an
  official client, and the daemon does not hold them under §1.1.5), and
  whether the daemon holds the official entries under §1.1.5. It is
  unset off Linux.
- **CLI.** `ossteam status` prints a `games` line when the private
  `~/.steam` is not available: that games are not isolated from the
  real `~/.steam`, the reason, and, when launches fail because of it,
  that they do and the ways out of §1.1.4. `--json` carries the same
  fields. `ossteam launch` reads `game_isolation` too and, when the
  private `~/.steam` is not available, prints one line on standard
  error before it launches, whether or not the launch then succeeds:

  ```
  warning: games get no private ~/.steam (<reason>); see `ossteam status`
  ```

  When the daemon also holds the official entries (§1.1.5), `launch`
  adds a second line: `warning: ossteam holds the official Steam
  client's ~/.steam entries; if that client starts, what this game
  starts next finds it and its account`. (With a private `~/.steam`,
  that cannot happen, so the line is not printed.) These lines go to standard error so that `--json`
  output stays clean, and `--quiet` does not hide them.
- **GUI** (`gui.md` §2.4, §4.5). On connecting, when the private
  `~/.steam` is not available, a warning notice says so, with the
  reason; when launches fail because of it, the notice says that and
  the ways out. The Settings page shows the same in its Daemon section
  for as long as it is so.
- **Notices at run time.** A warning `Notice` when a launch's steps
  fail (§1.1.2), when the daemon yields the entries (§1.1.5: "The
  official Steam client took ~/.steam back. What games ossteam started
  without a private ~/.steam start from now on finds the official
  client and its account."), and when the setting is turned on.
- **The setting.** Its description in `GetSettings` states both risks of
  §1.1.5. Turning it on in the GUI asks for confirmation with that text;
  `ossteam config set discovery.take_over_official true` prints it on
  standard error and then sets it.

#### 1.1.7 Open questions

- **Anti-cheat.** Anti-cheat accepts the Steam Linux Runtime's
  containers, which are user namespaces too; a native game in a user
  namespace of its own is less common. Whether any anti-cheat, VAC
  included (`vac.md`), treats it differently is *unverified*. DISC-4 is
  built without waiting for it, and checked live with an anti-cheat
  title the test account owns (§1.1.8).
- **Games installed under `~/.steam`.** Step 4 hides everything under
  the real `~/.steam`, so a game whose install directory is there (a
  library folder in Debian's `~/.steam/debian-installation`, say) is
  not found in its own namespace. Whether the launch check refuses
  such a game with a message of its own, or adding a library folder
  under `~/.steam` is refused, is not decided.
- **Inside the Steam Linux Runtime.** `pressure-vessel` builds its
  container from the mounts it sees, so it should carry the private
  `~/.steam`, and the binds of steps 5 and 6, into the container, and
  its own `bwrap` should nest inside the user namespace. Both are
  *unverified* (§1.1.8). A setuid `bwrap`
  gains no privileges in a user namespace, so a system that allows user
  namespaces only through a setuid `bwrap` makes no private `~/.steam`
  (the check at start fails), and the real entries are used.

#### 1.1.8 To check

| Item | Status |
|---|---|
| `steam_api` builds the path from `$HOME` | one older 32-bit `libsteam_api.so` read; newer SDKs unverified. Portal: First Slice's `libsteam_api.so` loaded `/home/<user>/.steam/sdk32/steamclient.so` (2026-11-03) |
| A native game finds ossteam through the private `~/.steam` | done 2026-11-03: Portal: First Slice (410, 32-bit `hl2_linux`), with an official client owning the real `~/.steam`. It loaded ossteam's `sdk32/steamclient.so` and connected to the daemon (`Hello.pid` matched). It ran in the Steam Linux Runtime, as Valve maps native games. Without a container: done the same day, Portal: First Slice through `legacysteamruntime` (compat-tools.md §3), which runs the game on the host with scout's libraries on `LD_LIBRARY_PATH`; it ran in ossteam's namespace, loaded `sdk32/steamclient.so` from the private `~/.steam`, connected, and saw the official `userdata/` and `config/` covered. With no tool at all it needs the host's own 32-bit libraries (SDL2 among them). Counter-Strike 2 (730) cannot run outside the Steam Linux Runtime 3.0: its `cs2.sh` exits unless `/etc/os-release` names `sniper` |
| A game in the Steam Linux Runtime, and one under Proton, find ossteam through it | done 2026-11-03. Steam Linux Runtime 1.0 (scout on soldier): its container nests inside ossteam's namespace, and the game reached the daemon through the abstract socket. Proton Experimental in the Steam Linux Runtime 4.0: Spacewar (480) connected (64-bit) |
| Proton and the Steam Linux Runtime work with a read-only `~/.steam` | done 2026-11-03, both |
| A game in the namespace sees ossteam's `userdata/` and an empty `config/` in the official root, also inside the Steam Linux Runtime | done 2026-11-03 inside the Steam Linux Runtime's container; on the host both were unchanged |
| Counter-Strike 2 in the Steam Linux Runtime 3.0 (sniper) reaches the daemon | done 2026-11-03 (64-bit), once a stale lock was removed: its single-instance lock `/tmp/source_engine_<n>.lock`, left by another user, cannot be opened with `fs.protected_regular` set, and the game then hangs after its error dialog fails (the same under the official client). It loaded `sdk64/steamclient.so` from the private `~/.steam` and connected within a second, with `userdata/` and `config/` covered inside the container, and loaded its menus. It cannot go online: `SteamGameCoordinator001` is not implemented, and it asks for `SteamNetworkingSocketsSerialized005` and `SteamAppDisableUpdate001`, which ossteam does not provide |
| Anti-cheat (VAC, and EAC or BattlEye where the test account owns a title) accepts a native game in the namespace | to be run |
| The check at start reports Ubuntu's AppArmor restriction | to be run on Ubuntu 24.04 |
| The official client starting while ossteam holds the entries (§1.1.5) | to be run |
| The client replaces a `steam.pid` naming a live process | unverified |

### 1.2 Windows

The official client keeps the key
`HKEY_CURRENT_USER\Software\Valve\Steam\ActiveProcess`. ossteam writes:

| Value | Type | Data |
|---|---|---|
| `pid` | `REG_DWORD` | the daemon's pid |
| `SteamClientDll` | `REG_SZ` | the 32-bit library's absolute path |
| `SteamClientDll64` | `REG_SZ` | the 64-bit library's absolute path |
| `ActiveUser` | `REG_DWORD` | the active account's account id, or 0 when none is logged on |

The entries are ossteam's when `SteamClientDll64` names ossteam's
64-bit library. If the key has a `SteamClientDll` or `SteamClientDll64`
that names another file, the key belongs to an official client, and the
daemon does not touch it (DISC-3). *Unverified:* which values `steam_api`
reads, and whether the official client removes them when it exits.

### 1.3 macOS

**Verification.** §1.3.1 and §1.3.2 come from the macOS
`libsteam_api.dylib` of Steamworks SDK 1.65a (Steamworks.NET
`ba71581f1e`, sha256 `a37faf19…ba759d2`), read with a disassembler,
and agree with one from 2017 (Steamworks.NET `625e1cc67f`) whose symbol
names (`CSimpleMachClient::Init`, `DoVersionCommand`,
`CIPCToolMachClient::GetSteamPath`, `DoKillCommand`) name the parts.
Neither has a path under `~/Library` or reads a registry file.
*Unverified:* what the official client's server puts in the fields
`steam_api` does not read, which file its path names, and whether its
launchd job stays loaded after the client quits. *Unverified on a Mac:*
the behaviour of launchd that §1.3.3 relies on, beyond what CI's macOS runners show.

macOS has no entries in the file system. `steam_api` asks launchd for
the Mach service `com.valvesoftware.steam.ipctool`, which the official
client's `ipcserver` serves as a launchd job of that label, and asks it
where the client is.

#### 1.3.1 Messages

Each message is a `mach_msg_header_t` (24 bytes) followed by 32-bit
words in the machine's byte order. `msgh_id` is the protocol version,
104, in requests and replies alike. A request carries the client's
reply port as its local port with `MACH_MSG_TYPE_MAKE_SEND`
(`msgh_bits` 0x1413: `COPY_SEND` remote, `MAKE_SEND` local), so the
server receives a send right, not a send-once right, and the reply goes
to that port. The client
receives until a message with `msgh_id` 104 arrives, into a buffer of
40 or 552 bytes. The kernel appends an 8-byte trailer to a message it
delivers and refuses one that does not fit with it
(`MACH_RCV_TOO_LARGE`), so the replies are 32 and 544 bytes.

| Request (32 bytes) | Word at 24 | Word at 28 | Reply | Client's timeout |
|---|---|---|---|---|
| Version | 100001 | the client's version, 104 | 32 bytes; the word at 28 is the server's version | 3 s |
| Kill | 100000 | the client's pid | none | 2 s to send |
| GetSteamPath | 14 | 0 | 544 bytes; the word at 28 is the client's pid, bytes 32 to 543 a NUL-terminated path | 5 s |

#### 1.3.2 What `steam_api` does

1. It looks the service up with `bootstrap_look_up`. If that fails, no
   client is running (`ipcserver init failed`).
2. It sends Version. A server version above 103 is accepted. Otherwise,
   or when no reply comes, it sends Kill, and if that send fails, asks
   launchd to `StopJob` the job `com.valvesoftware.steam.ipctool`. It
   then waits 100 ms, looks the service up again and carries on
   whatever the version, after printing `ipcserver restart: expected:
   104, got: <version>`.
3. It sends GetSteamPath. `SteamAPI_IsSteamRunning()` is true when the
   reply comes, its pid is not 0, `kill(pid, 0)` succeeds, and the pid
   is not one `steam_api` recorded before. A handler that `SteamAPI_Init`
   registers asks GetSteamPath again and records the pid (when it runs
   is *unverified*).
4. `SteamAPI_Init` replaces the path's last component with
   `steamclient.dylib` and loads that with `dlopen(RTLD_NOW)`. A path
   without a `/` fails (`steam path (…) missing directory separator`).
   Before asking, it may try a `steamclient.dylib` that the dynamic
   loader finds by name alone, as on Linux.

#### 1.3.3 ossteam's entries

- `<data>/steam-sdk/osx/steamclient.dylib`, a copy of the library
  (§1.4), replaced as the Linux copies are, so that a running game keeps
  the file it has mapped.
- The Mach service. The daemon takes its receive right with
  `bootstrap_check_in` and serves it on a thread of its own. It answers
  Version with version 104 and GetSteamPath with its pid and the path of
  the copy; a copy whose path does not fit in 512 bytes with its NUL is
  an error, and no service is made. Each reply has the request's
  `msgh_id` and repeats the request's word at 24, and moves back the
  right the request brought: a send right from `steam_api`, or a
  send-once right from another client. Kill
  is logged and ignored: there is no ipcserver to restart, and the
  client carries on. Other requests are dropped without a reply.

launchd lets a process check in a name that no job declares, as
Chromium relies on (seen on CI's macOS runners, aarch64 and x86_64,
on 0254d99: the check-in succeeded and a look-up found the name). The entries are ossteam's
while the daemon holds that receive right. Removing them destroys it,
which takes the name away (*unverified*), and a crash takes it away
with the process, so DISC-2 has nothing to repair on macOS. The copy
stays, as the Linux directories do.

**Conflict (DISC-3).** If `bootstrap_look_up` finds the service while
the daemon does not hold it, or `bootstrap_check_in` reports it in use,
the service belongs to an official client and the daemon leaves it
alone. A launch then fails with `CONFLICT`, naming the service. While
the official client's job is loaded, launchd holds the name even when
the client is not running, in order to start it on demand.

### 1.4 The client libraries

The daemon finds its client libraries in the directory given by
`OSSTEAM_STEAMCLIENT_DIR`, or else in the directory of its own
executable:

| OS | 64-bit | 32-bit |
|---|---|---|
| Linux | `libossteam_steamclient.so` | `lib32/libossteam_steamclient.so` |
| Windows | `ossteam_steamclient.dll` | `lib32\ossteam_steamclient.dll` |
| macOS | `libossteam_steamclient.dylib`, universal (x86_64 and arm64) | none |

The 64-bit library is required. Without it, no entries are created, and
a launch fails with `UNSUPPORTED`, with a message that the client
library is not installed. The 32-bit library is optional. Without it,
`sdk32` (Linux) and `SteamClientDll` (Windows) are not created, and a
32-bit game's `steam_api` finds no client, as under an official client
that lacks one. On macOS the library holds both architectures, so that
an arm64 game and an x86_64 game under Rosetta both load it; macOS runs
no 32-bit code.

### 1.5 Lifetime

The daemon creates the entries once the session has an active account,
and removes them when it has none any more (a logout) and when the
daemon exits (DISC-1). An account stays active while the connection is
down and the daemon reconnects, so the entries stay too: the game's
`steam_api` then finds a client that reports being logged off (NET-4),
not a missing one. On start, before anything else, the daemon removes
entries that are ossteam's but hold a pid that is not its own, which a
crash leaves behind (DISC-2).

On Linux, "the entries" are the real ones while they are free (§1.1.1)
and `<data>/steam-sdk/steam.pid`, which the private `~/.steam` needs
(§1.1.2), whether or not the real ones are free. Real entries taken
over from an official client (§1.1.5) are handed back, not just
removed, wherever this section removes them; after a crash, the next
start hands them back from `official-entries.toml` before anything
else.

## 2. The client library (ABI-1 to ABI-4)

**Verification.** The exports in §2.1 are the ones the official Linux
client's `steamclient.so` (build 1791918634) defines that a shipped
`libsteam_api.so` looks up, with the signatures that Proton's
`lsteamclient` declares for them. The calling conventions in §2.2 are
those of the platforms' C++ ABIs. The vtables (§2.4) come from Proton's
`lsteamclient`, Valve's open-source bridge between Windows games and
the Linux client library, which lists every interface version of every
published Steamworks SDK. What happens inside the official client is
*unverified* unless a section says otherwise.

### 2.1 Exports

`steam_api` loads the library, then calls these by name. All use the C
calling convention (`cdecl` on 32-bit x86). `bool` is one byte.

| Export | Signature | ossteam |
|---|---|---|
| `CreateInterface` | `void *(const char *version, int *return_code)` | An `ISteamClient` object (§2.3) when `version` is a `SteamClient` version in the table (§2.4). Otherwise NULL, logged and recorded as an unknown version (§4). `*return_code`, when not NULL, is set to 0 on success and 1 on failure. |
| `Steam_BGetCallback` | `bool (HSteamPipe, CallbackMsg_t *, HSteamCall *)` | §2.5. The third parameter exists only in older SDKs and is never read or written. |
| `Steam_FreeLastCallback` | `bool (HSteamPipe)` | §2.5 |
| `Steam_GetAPICallResult` | `bool (HSteamPipe, SteamAPICall_t, void *callback, int size, int expected_id, bool *failed)` | §2.5 |
| `Steam_IsKnownInterface` | `bool (const char *version)` | Whether `version` is in the table (§2.4). |
| `Steam_NotifyMissingInterface` | `void (HSteamPipe, const char *version)` | Logged and recorded as an unknown version (§4). |
| `Steam_ReleaseThreadLocalMemory` | `void (int thread_exit)` | Nothing: the shim keeps no per-thread memory. |
| `Breakpad_SteamMiniDumpInit` | `void (uint32, const char *, const char *)` | Unimplemented: ossteam collects and uploads no crash dumps. Recorded (§4). |
| `Breakpad_SteamSetAppID` | `void (uint32)` | Unimplemented, recorded. |
| `Breakpad_SteamSetSteamID` | `int (uint64)` | Unimplemented, recorded; returns 0. |
| `Breakpad_SteamWriteMiniDumpSetComment` | `int (const char *)` | Unimplemented, recorded; returns 0. |
| `Breakpad_SteamWriteMiniDumpUsingExceptionInfoWithBuildId` | `void (int, int)` | Unimplemented, recorded. |

The official library exports more: `Breakpad_SteamSendMiniDump`, and
an older flat interface (`Steam_CreateSteamPipe`, `Steam_LogOn`,
`Steam_GSLogOn` and others) that no `steam_api` from a published SDK
calls (*unverified*). ossteam does not export them, so a program that
needs them fails to find them, which is an honest failure.

### 2.2 Calling conventions and types (ABI-2)

An interface object is a pointer to a structure whose first field points
to the vtable, an array of function pointers in the order of §2.4. Each
method takes the object (`this`) first.

| Target | Methods | A struct returned by value |
|---|---|---|
| Linux x86_64, macOS x86_64 | System V: `this` in the first integer register | In registers when it is at most 16 bytes, by the C classification. Otherwise through a hidden pointer passed before `this`. |
| Linux i686 | `cdecl`: `this` first on the stack, the caller pops the arguments | Always through a hidden pointer, passed before `this`. The callee pops the pointer. |
| Windows x86_64 | Microsoft x64: `this` in `rcx` | Always through a hidden pointer, passed after `this`. The method returns the pointer. |
| Windows i686 | `thiscall`: `this` in `ecx`, the callee pops the arguments | Always through a hidden pointer, passed after `this`. The method returns the pointer. |
| macOS aarch64 | AAPCS64 | In registers when it is at most 16 bytes, otherwise through `x8` (M1e). |

The struct return rule on Windows is MSVC's rule for member functions,
which differs from its rule for plain functions. The Itanium C++ ABI
(Linux and macOS) treats `CSteamID` and the other returned structs as
trivially copyable, so they follow the C rules.

- A `bool` argument is true when its byte is not zero. A returned `bool`
  is 0 or 1.
- `CSteamID` and `CGameID` are 64-bit classes. As arguments they are
  passed exactly like a `uint64`. As return values they follow the
  struct rule above.
- Strings are UTF-8 and end in NUL. A method that writes into a buffer
  the game provides never writes past the size the game gave. Each
  interface page says whether it truncates or fails when the buffer is
  too small.
- Structures that games and the library exchange, callbacks included,
  are packed to 4 bytes on Linux and macOS and to 8 bytes on Windows
  (`VALVE_CALLBACK_PACK_SMALL` and `VALVE_CALLBACK_PACK_LARGE` in the
  SDK). The Steam Input action data structures and `SteamIPAddress_t`
  are packed to 1 byte. So are `CSteamID` and `CGameID`, which the SDK
  declares inside `pack(1)`: as a member of a callback they are aligned
  to 1 byte, so in `UserStatsReceived_t` (`uint64`, `int`, `CSteamID`)
  the Steam id is at offset 12 on every platform; the struct is 20 bytes
  on Linux and macOS, 64-bit included, and 24 on Windows, where the
  `uint64` pads its end.

### 2.3 Objects, pipes and users

**Interface objects.** Each object holds its vtable, its interface
version, and the pipe and user it was obtained for. The library makes
one object for each combination and returns the same pointer every time
it is asked for it again. Objects are never freed, because games keep
the pointers for as long as they run. `CreateInterface` returns one
`ISteamClient` object per version, not bound to a pipe.

**Pipes.** `ISteamClient::CreateSteamPipe` opens a connection to the
daemon's game endpoint (§3) and returns a pipe handle. Handles count up
from 1 and are not reused while the process runs (*unverified*: how the
official client numbers them). It returns 0 when no daemon answers or
the daemon refuses the connection, and logs why. That is what makes
`SteamAPI_Init` fail when ossteam is not running.
`BReleaseSteamPipe(pipe)` closes the connection, drops the pipe's
queued callbacks and pending results, and returns true; it returns
false for a pipe that is not open. `BShutdownIfAllPipesClosed` returns
true when no pipe is open.

**Users.** `ConnectToGlobalUser(pipe)` returns user handle 1 when the
daemon's welcome (§3.3) says an account is active and the account owns
the app id the process claims. Otherwise it returns 0 and logs why. One
user exists per pipe. `ReleaseUser(pipe, user)` forgets it.
`CreateLocalUser` creates the users of game servers and anonymous
sessions, which are not in M1. It returns 0 and leaves `*phSteamPipe`
alone.

**Getters.** `GetISteamUser(user, pipe, version)` and the other
`GetISteam*` methods return the object of `version` for that pipe and
user, or NULL when:

- `version` is not a version of that getter's interface in the table
  (§2.4), which is also logged and recorded as an unknown version (§4);
- the pipe is not open;
- the interface belongs to a user and `user` is not the pipe's user.
  `ISteamUtils` and `ISteamNetworkingUtils` belong to the pipe and
  accept any user, since `steam_api` asks for them with user 0.

`GetISteamGenericInterface(user, pipe, version)` looks `version` up in
every interface. `GetISteamContentServer` and `GetIVAC` name interfaces
that no table version has, so they return NULL. VAC is M2 (SPEC §14).

A game may keep calling an object whose pipe was released or whose
connection closed. Its methods then answer as for a client that is
logged off: `BLoggedOn` is false, and calls that need the daemon fail.

### 2.4 The vtable table

`docs/spec/steamworks/vtables.txt` lists every interface version
ossteam serves. It is generated with `cargo xtask import-steamworks
<dir>` from the generated wrappers in Proton's `lsteamclient` directory,
and the file names the Proton commit it came from. Only facts are taken
from Proton: the version strings, the methods in vtable order, their
parameter names and their types. The import checks each method's
32-bit `thiscall` stack size against Proton's.

The file has one block per version. `[SteamUser023 ISteamUser]` starts
the block of version string `SteamUser023` of interface `ISteamUser`.
Each line after it is one slot: `<slot> <Method>(<name>: <type>, ...) ->
<type>`. The types are ABI types:

| Type | Meaning |
|---|---|
| `i8`, `u8`, `i16`, `u16`, `i32`, `u32`, `i64`, `u64` | integers, enums and handles of that size. A C++ `bool` is listed as `i8`; the interface pages say which parameters are booleans. |
| `f32`, `f64` | `float`, `double` |
| `ptr` | any pointer or reference, function pointers included |
| `str` | a returned `const char *` |
| `void` | no return value |
| `CSteamID`, `CGameID` | the 64-bit classes (§2.2) |
| `SteamIPAddress_t` | 20 bytes: 16 address bytes, then a 32-bit type; packed to 1 |
| `InputAnalogActionData_t`, `ControllerAnalogActionData_t` | 13 bytes: `i32` mode, `f32` x, `f32` y, `bool` active; packed to 1 |
| `InputDigitalActionData_t`, `ControllerDigitalActionData_t` | 2 bytes: `bool` state, `bool` active |
| `InputMotionData_t`, `ControllerMotionData_t` | 40 bytes: ten `f32` |
| `SteamPartyBeaconLocation_t`, `RemoteStorageUpdatePublishedFileRequest_t` | structures passed by value, 12 and 40 bytes on 32-bit Windows; never implemented in M1 |

**Overloads.** A few interfaces declare one method name more than once
(`ISteamUserStats::GetStat` for `int32` and for `float`, and others).
Proton names the second declaration `<Method>_2`, the third
`<Method>_3`, and lists the slots as MSVC lays them out: next to each
other, the last declaration first. The Itanium C++ ABI that GCC and
Clang follow on Linux and macOS keeps the declaration order. So on those
targets each run of overloads is reversed: `GetStat` (`int32`) takes the
first slot of its pair and `GetStat_2` (`float`) the second, where
Windows has them the other way round. Every overloaded method in the
table has its declarations next to each other, so reversing each run is
the whole difference. *Unverified* against a native game until the
acceptance runs; a swapped pair would read an `int32` stat as a `float`.

The table has 212 versions of 38 interfaces, 6555 slots. The official
client also answers versions that no published SDK declares
(`SteamClient022`, `SteamFriends016`, `SteamUtils001` and others found
in its library's strings). They are not in the table, so the getters
return NULL for them (FAITH-2).

**Every slot is filled (ABI-2, ABI-3).** A slot runs ossteam's
implementation when the interface's page lists the method as
implemented and the slot's signature is the one implemented. Every other
slot runs a stub that records the call (§4), logs the first one, and
returns the failure value of its return type:

| Return type | Failure value |
|---|---|
| `void` | none |
| integers, enums, handles, `bool` | 0: false, `k_uAPICallInvalid` for `SteamAPICall_t`, `k_HAuthTicketInvalid` for an auth ticket |
| `f32`, `f64` | 0.0 |
| `ptr` | NULL |
| `str` | an empty string, not NULL, so that a game that does not check does not crash (*unverified*: what the official client returns for unknown items) |
| `CSteamID` | `k_steamIDNil` (0) |
| `CGameID` and the structures | every byte 0 |

A stub cannot tell an `EResult` from other integers, so it returns 0
(`k_EResultNone`) for one. A method whose documented failure value is
something else gets an implementation that returns that value, listed on
its interface's page.

**Building.** The library's build script reads the table and
`crates/ossteam-steamclient/src/implemented.txt`, which maps a method,
by interface, name and signature, to the Rust function that implements
it. It generates one function for each slot with that slot's exact
signature, and the vtables. A method whose signature changed in some
version matches only the versions with the implemented signature. The
others keep the stub until an implementation for their signature is
added.

### 2.5 Callbacks and API calls (GIPC-3)

**Callbacks.** Each pipe has a queue of callbacks. The shim adds to it
when the daemon reports something (§3.4) and when an API call completes.
It hands callbacks to the game only from `Steam_BGetCallback(pipe,
msg)`, which `steam_api` calls when the game pumps callbacks
(`SteamAPI_RunCallbacks`, or the manual dispatch functions):

- If the queue is empty, it returns false.
- Otherwise it fills `msg` with the oldest callback and returns true.
  `CallbackMsg_t` is `{HSteamUser m_hSteamUser; int m_iCallback; uint8
  *m_pubParam; int m_cubParam}`, with the packing of §2.2. The user is
  the pipe's user, or 0. `m_pubParam` points to the shim's copy of the
  callback structure, which stays valid until the callback is freed.
- `Steam_FreeLastCallback(pipe)` removes the callback that
  `Steam_BGetCallback` returned and frees it. It returns true, or false
  when there was none. Calling `Steam_BGetCallback` again without
  freeing returns the same callback.

`Steam_BGetCallback` never waits for the daemon. When the queue is
empty, it only looks the pipe up and checks its queue (§20: at most
5 µs; `ossteam-steamclient/tests/pump.rs` measures it).

**API calls.** A method that returns `SteamAPICall_t` sends its request
to the daemon and returns a handle at once. Handles count up from 1 and
are unique in the process. When the daemon's answer arrives, the shim
stores the result structure and queues `SteamAPICallCompleted_t`
(callback 703) on the call's pipe:

| Field | Type |
|---|---|
| `m_hAsyncCall` | `uint64`: the handle |
| `m_iCallback` | `int`: the result's callback id |
| `m_cubParam` | `uint32`: the result's size |

`steam_api` reacts to callback 703 by calling `Steam_GetAPICallResult(
pipe, call, buffer, size, expected_id, failed)`. When the pipe holds a
result for `call`, its callback id is `expected_id` and its size is
`size`, the result is copied to `buffer`, `*failed` is set when the call
failed, the result is forgotten, and the function returns true.
Otherwise it returns false and keeps the result. `ISteamUtils` has the
same function (`GetAPICallResult`), together with `IsAPICallCompleted`
and `GetAPICallFailureReason` (`steamworks/isteamutils.md`).

When the connection closes, every call still pending completes as
failed with `k_ESteamAPICallFailureSteamGone`. Its result structure is
all zeros. Results that are never collected stay until the pipe is
released.

### 2.6 Panics and logging (ABI-4)

Every export and every implemented method catches panics. A panic is
logged, the call returns its failure value (§2.4), and the game goes on.
Stubs cannot panic.

The shim writes its log to standard error. Each line starts with
`[ossteam-steamclient]`. For a game that ossteam launched, standard
error is the game's log file (`launch.md` §3). `OSSTEAM_SHIM_LOG=debug`
adds a line for every call that reaches the daemon. By default the shim
logs warnings, refusals, and the first call of each unimplemented slot.

## 3. Game IPC (GIPC-1 to GIPC-3)

### 3.1 Endpoint

| OS | Endpoint |
|---|---|
| Linux | Unix socket `$XDG_RUNTIME_DIR/ossteam/game.sock`, and the abstract Unix socket `ossteam-game-<uid>` |
| macOS | Unix socket `$TMPDIR/ossteam/game.sock` |
| Windows | Named pipe `\\.\pipe\ossteam-game-<SID>` |

The directory, the socket mode and the pipe's security descriptor follow
`ipc.md` §1. The shim checks the directory before it connects, as leaf
clients do. On Windows it also checks that the process serving the pipe
(`GetNamedPipeServerProcessId`) runs as its own user, so that a pipe
another user created first is not trusted. The daemon checks that the
peer is the same user (IPC-2) and reads the peer's pid: `SO_PEERCRED`
on Linux, `LOCAL_PEERPID` on macOS, `GetNamedPipeClientProcessId` on
Windows. The daemon listens on the game endpoint for as long as it runs,
whether or not an account is logged in. Debug builds on Windows honour
`OSSTEAM_TEST_ROOT` for this pipe too (`ipc.md` §1).

**Linux containers.** A game in a Steam Linux Runtime container has a
private `$XDG_RUNTIME_DIR` but shares the network namespace, which is
where abstract socket names live (`compat-tools.md` §9). The shim
connects to the socket file when it exists and to the abstract socket
otherwise. An abstract socket has no permissions, so after connecting
the shim reads the listener's credentials (`SO_PEERCRED`) and closes the
connection, finding no client, unless the listener runs as its own
user. A name another user took first therefore hides the daemon, which
logs that it could not listen on it, but cannot impersonate it.

### 3.2 Framing and encoding

Each frame is a 4-byte little-endian length, then that many bytes of
one message. A frame is at most 16 MiB. The message's first byte is its
tag (§3.4). Its fields follow in the order the table gives, encoded as:

| Field type | Encoding |
|---|---|
| `u8` to `u64`, `i32` | little-endian, fixed width |
| `bool` | one byte, 0 or 1 |
| `string` | `u32` length, then that many bytes of UTF-8 |
| `list<T>` | `u32` count, then the items |

A message with an unknown tag, or one that does not decode, is a
protocol error: the receiver closes the connection. The shim and the
daemon are built together, so there are no optional fields. The protocol
version changes whenever a message changes.

### 3.3 Handshake

The shim's first message is `Hello`. The daemon answers `Welcome` or
`Refused` and, after `Refused`, closes the connection.

- `Hello.protocol` must be the daemon's protocol version, or the daemon
  refuses with a reason that names both versions. A game that is still
  running when a newer daemon starts gets this refusal, and its
  `steam_api` finds no client.
- `Hello.pid` must be the peer's pid where the OS reports one, or the
  daemon refuses.
- `Hello.app_id` is the app the process claims to be. The shim reads it
  from the environment variable `SteamAppId`, or else from the low 24
  bits of `SteamGameId`, or else uses 0. `steam_api` sets `SteamAppId`
  from `steam_appid.txt` before it loads the library (*unverified*).

`Welcome` carries the session (§3.4) and the app's state. From then on,
the shim sends `Request`s and `Report`s, and the daemon sends
`Response`s and updates.

### 3.4 Messages

Protocol version 4.

| Tag | Message | Direction | Fields |
|---|---|---|---|
| 1 | `Hello` | shim → daemon | `protocol: u32`, `pid: u32`, `app_id: u32`, `arch: string` (`x86_64`, `x86` or `aarch64`), `version: string` (the shim's crate version) |
| 2 | `Welcome` | daemon → shim | `session: Session`, `app: App` |
| 3 | `Refused` | daemon → shim | `reason: string` |
| 4 | `Request` | shim → daemon | `id: u32`, `body: RequestBody` |
| 5 | `Response` | daemon → shim | `id: u32`, `body: ResponseBody` |
| 6 | `SessionUpdate` | daemon → shim | `session: Session` |
| 7 | `AppUpdate` | daemon → shim | `app: App` |
| 8 | `Report` | shim → daemon | `interfaces: list<string>`, `unknown: list<string>`, `calls: list<Call>` (§4) |
| 9 | `Notice` | daemon → shim | `body: NoticeBody`, which starts with its own tag: what Steam said later about a request (`steamworks/isteamuser.md` §3) |

`Session`:

| Field | Type | Meaning |
|---|---|---|
| `active` | `bool` | an account is active (it may be disconnected) |
| `logged_on` | `bool` | the account is logged on to Steam right now |
| `steam_id` | `u64` | the active account, or 0 |
| `persona_name` | `string` | its persona name |
| `persona_state` | `u32` | its `EPersonaState`: 0 offline, 1 online, 7 invisible |
| `ip_country` | `string` | the two-letter country Steam gave at logon, or empty |
| `server_time` | `u32` | Steam's clock (Unix seconds) when the message was sent, from the logon response's time and the daemon's monotonic clock since; 0 before the first logon |
| `universe` | `u32` | 1 (public) |
| `ui_language` | `string` | the daemon's `language` setting |

`App`:

| Field | Type | Meaning |
|---|---|---|
| `app_id` | `u32` | the claimed app |
| `owned` | `bool` | the app is in the account's library |
| `launched` | `bool` | the process belongs to a game the daemon launched for this app (§3.5) |
| `installed` | `bool` | the app is installed |
| `install_dir` | `string` | its install directory, or empty |
| `build_id` | `u32` | the installed build, or 0 |
| `branch` | `string` | the installed branch; empty for `public` |
| `language` | `string` | the language the app is installed in (`content.language`), as a Steam language name |
| `languages` | `list<string>` | the languages the app info declares, in its order |
| `depots` | `list<u32>` | the installed depots |
| `dlc` | `list<Dlc>` | the app's DLC as its app info lists them |
| `user_dir` | `string` | `<data>/userdata/<account id>/<app id>`; empty when no account is active |
| `owner` | `u64` | the Steam id of the license owner: the active account in M1 |
| `low_violence` | `bool` | the license that grants the app carries the low-violence flag (`content.md` §6) |
| `cloud_quota` | `u64` | `ufs/quota` of the app's info (`pics.md`): its Steam Cloud bytes, 0 when it has none |
| `cloud_max_files` | `u32` | `ufs/maxnumfiles`: how many files it may keep there, 0 for no limit |

`Dlc`: `app_id: u32`, `owned: bool`, `installed: bool`, `available:
bool` (its info says it is released), `name: string` (empty when its
info is not known). A DLC is installed when it is owned, the app is
installed, and the DLC has no depots of its own in the app's info or
one of them is installed.

`RequestBody` and `ResponseBody` start with their own tag:

| Tag | Request | Response |
|---|---|---|
| 1 | `AppState{app_id: u32}` | `AppState{owned: bool, installed: bool, install_dir: string, build_id: u32, depots: list<u32>}` |
| 2 to 5 | the stats requests of `steamworks/isteamuserstats.md` §2 | their responses |
| 6 to 11 | the ticket requests of `steamworks/isteamuser.md` §3 | their responses; `Done{}` (tag 7) for the requests that have nothing to say |
| 255 | — | `Error{message: string}`: the request could not be answered |

`Call`: `interface: string` (a version string), `method: string`,
`count: u32`.

### 3.5 Identification and ownership (GIPC-2)

The daemon knows each connection's pid (§3.1) and the app id it claims.
The process is **launched** when it belongs to a game the daemon
launched for that app id and that is still running: on Linux a
descendant of the game's reaper, on Windows a process in the game's job
object, on macOS a member of the game's process group.

A process that is not launched gets the same answers, no more and no
fewer. Steam lets any process that loads its library ask about the app
id it claims. Ownership is checked against the account's licenses
(`owned`), and an app the account does not own gets no user (§2.3).

A launched process is tracked as part of its game (`launch.md` §4).
The game's end (`GameExited`) waits until every connection from the
game's processes has closed, at most 2 s after the tree is gone, so that
the last report arrives first (§4). Until then the app counts as
running, and `Launch` refuses it with `CONFLICT`. A process that is not launched is
not reported as played: play sessions come from launches only (PLAY-1).
*Unverified:* the official client reports a game started outside it as
being played once its `steam_api` connects.

### 3.6 What a game cannot do (GIPC-1)

The messages in §3.4 are everything the game endpoint understands. None
of them logs in or out, installs, uninstalls, verifies, changes a
setting, or reads tokens, credentials or other accounts. `AppState`
tells a game about another app only what Steam's own `ISteamApps` tells
any game. The stats and ticket requests act only on the connection's
own app id, and only for an app the account owns. A connection can
cancel only the tickets and end only the auth sessions it made.

### 3.7 The daemon's side

One thread accepts connections. Each connection has a reader thread and
a writer thread, as client connections do (`ipc.md` §7). The core
answers `Hello` and `Request`s from its own state. It sends
`SessionUpdate` to a connection when a field of its `Session` other than
`server_time` changes, or Steam's clock is taken from a new logon, and
`AppUpdate` when its `App` changes: after a change of the session or the
library, a finished job, or a setting.

The shim turns updates into callbacks on every open pipe:

| Change | Callback |
|---|---|
| `logged_on` becomes true | `SteamServersConnected_t` (101), no fields |
| `logged_on` becomes false | `SteamServersDisconnected_t` (103), `EResult m_eResult` = 3 (`k_EResultNoConnection`) |

*Unverified:* the official client's timing of these callbacks and the
`EResult` it reports.

When the daemon exits, the connections close. The shim then answers as
a client that is logged off, fails pending calls (§2.5), and does not
reconnect.

## 4. ShimReports (ABI-5)

The shim records, per process:

- every interface version it handed out (`interfaces`);
- every version it was asked for and does not have (`unknown`), from
  `CreateInterface`, the getters and `Steam_NotifyMissingInterface`;
- every call to a stub (§2.4) and to an unimplemented export (§2.1),
  counted per slot (`calls`).

It sends what changed in a `Report` on one of its open connections:
right away when something is recorded for the first time, otherwise at
most once a second from `Steam_BGetCallback`, and when a pipe is
released. What is recorded while no connection is open waits for the
next one. A process that dies loses at most the last second of counts.

The daemon adds the reports up per process. It sends `ShimReport` on
the `GAMES` topic (`ipc.md` §4):

- for a launched game, once, just before its `GameExited`, covering
  every process of the game;
- for a process that is not launched, when its last connection closes.

`ShimReport{app_id, pid, launched, interfaces, unknown_versions,
unimplemented}` lists the versions in the order they were first seen and
the unimplemented calls by count, highest first. For a launched game,
`pid` is the pid the daemon started. The CLI prints the report after the
game exits (`ossteam launch --wait`, `ossteam stop`).

## 5. Interfaces

Each interface with implemented methods has a page under
`docs/spec/steamworks/`. Every other interface is reachable through its
getter and fails under ABI-3.

| Interface | Page | M1 scope (SPEC §12.3) |
|---|---|---|
| `ISteamClient` | `isteamclient.md` | everything except game servers |
| `ISteamUser` | `isteamuser.md` | logon state, Steam id, tickets and auth sessions |
| `ISteamApps` | `isteamapps.md` | ownership, DLC, branch, build, install directory, languages |
| `ISteamUtils` | `isteamutils.md` | app id, country, server time, API-call plumbing, overlay state |
| `ISteamFriends` | `isteamfriends.md` | own persona name and state |
| `ISteamRemoteStorage` | `isteamremotestorage.md` | local files, nothing synced |
| `ISteamUserStats` | `isteamuserstats.md` | stats, achievements, schema, icons, global achievement percentages |

## 6. Verification against the official client

| Item | Status |
|---|---|
| The exports `steam_api` uses (§2.1) | found in the official library and in `libsteam_api.so`; signatures from Proton |
| The calling conventions (§2.2) | the platforms' C++ ABIs; Windows from Proton's wrappers |
| The vtables (§2.4) | from Proton's `lsteamclient`; to be checked against games |
| Pipe and user handle values (§2.3) | unverified |
| `ConnectToGlobalUser` for an app the account does not own (§2.3) | unverified |
| Results kept after a mismatched `Steam_GetAPICallResult` (§2.5) | unverified |
| The empty string for unimplemented `const char *` methods (§2.4) | unverified |
| `SteamServersConnected_t` and `SteamServersDisconnected_t` timing (§3.7) | unverified |
| Play sessions of games not started by the client (§3.5) | unverified |
| Overloads in declaration order on Linux and macOS (§2.4) | the Itanium C++ ABI; checked by the ABI test (§6.1) against GCC and Clang on Linux x86_64 and i686; still to be checked against a native game |
| Overloads in reverse order on Windows (§2.4) | the MSVC C++ ABI; checked by the ABI test (§6.1) with clang's MSVC ABI, x86_64 and i686, under Wine |
| Class returns and arguments, and callback packing (§2.2) | checked by the ABI test (§6.1) on Linux x86_64 and i686, and for Windows x86_64 and i686 (`thiscall`) under Wine |

### 6.1 The ABI test

SPEC §19.1 level 4. `ossteam-steamclient/tests/abi.rs` builds a small
C++ game, `tests/abi/game.cpp`, with the platform's C++ compiler (`$CXX`,
else `c++`), and runs it against the library and a fake daemon. The game
declares `ISteamClient`, `ISteamUser`, `ISteamUtils` and `ISteamUserStats`
(version 013) itself, in the order the Steamworks headers declare their
methods, overloads included, so the compiler lays out the vtables rather
than the shim's tables. It checks a `CSteamID` returned and passed by
value, `bool`, `float` and `uint64` results, overloads that only work in
the right slot (`GetStat`, `SetStat`, `GetUserStat`), and the sizes of
`UserStatsReceived_t` and `SteamAPICallCompleted_t` against what the
library queues, and where the Steam id sits in `UserStatsReceived_t`.

The declarations are our own. The test checks the platform rules of
§2.2 and §2.4 against a real compiler. It does not check that the
Steamworks headers declare the methods in that order; that needs a
native game. Run with `--target i686-unknown-linux-gnu` for the 32-bit
library; the game is then built with `-m32`. It passed on 2026-10-19
with GCC and Clang, x86_64 and i686. macOS first runs it on CI.

On Windows the game must be built with the MSVC C++ ABI: the test takes
a game built beforehand from `$OSSTEAM_ABI_GAME`, or builds one with
`$CXX`, else `clang-cl`. The game then includes no C library or SDK
headers, so that clang can build it for `*-pc-windows-msvc` without the
Windows SDK and MinGW can link it; `abi.rs` gives the commands for a
run under Wine. The library finds the fake daemon's named pipe through
`OSSTEAM_TEST_ROOT` (`ipc.md` §1), which only debug builds read. That run
passed on 2026-10-19 for x86_64 and i686, after it found the Steam id
of `UserStatsReceived_t` and `ValidateAuthTicketResponse_t` 4 bytes too
far on Windows. Building with `clang-cl` or MSVC on real Windows has not
been tried.
