# The Steamworks client ABI

| | |
|---|---|
| Status | M1c. §1 (discovery entries), §2 (the library), §3 (game IPC) and §4 (ShimReports) implemented; the interface pages in `steamworks/` grow with the shim |
| Requirements | SPEC §12 (ABI-1 to ABI-5, GIPC-1 to GIPC-3, DISC-1 to DISC-3), PLAY-1, LAUNCH-4 |
| Code | discovery entries in `crates/ossteam-launch/src/discovery/`; the library in `crates/ossteam-steamclient`; the game IPC in `crates/ossteam-gameipc`; the daemon's side in `crates/ossteamd/src/launch.rs`, `crates/ossteamd/src/gameipc.rs` and `crates/ossteamd/src/core/{games,shim}.rs` |
| Interfaces | one page each under `docs/spec/steamworks/`; the vtables in `docs/spec/steamworks/vtables.txt` (§2.4) |

A game links Valve's `steam_api` library and ships it. `steam_api` finds
a running client, loads that client's library (`steamclient.so`,
`steamclient.dll` or `steamclient64.dll`, `steamclient.dylib`) and talks
to it through versioned interfaces. `ossteam-steamclient` is ossteam's
client library.

## 1. Discovery entries (DISC-1 to DISC-3)

**Verification.** §1.1 comes from the official Linux client's files in
the home directory and from the strings in a shipped `libsteam_api.so`
(`%s/.steam/%s`, the message that `SteamAPI_IsSteamRunning()` found no
running client, and the message that a local `steamclient.so` was tried
first). How `steam_api` checks that the client is running is
*unverified*. The Windows entries in §1.2 are *unverified*.

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
`~/.steam/sdk64/steamclient.so` (or `sdk32` for a 32-bit game). Before
that it checks for a running client through `~/.steam/steam.pid`
(*unverified*: that it checks the pid names a live process).

ossteam creates only `sdk64`, `sdk32` and `steam.pid`:

- `~/.steam/sdk64` and `~/.steam/sdk32` are symlinks to
  `<data>/steam-sdk/sdk64` and `<data>/steam-sdk/sdk32`. Those
  directories belong to ossteam, and each holds a `steamclient.so`
  symlink to the matching library (§1.3).
- `~/.steam/steam.pid` holds the daemon's pid.

`~/.steam` is created with mode 0755 when it does not exist, as the
official client creates it.

An entry is ossteam's when `sdk64` or `sdk32` is a symlink whose target
is the matching directory under `<data>/steam-sdk`. `steam.pid` is
ossteam's when `sdk64` is. ossteam writes `steam.pid` last and removes
it first.

**Conflict (DISC-3).** If `sdk64`, `sdk32` or `steam.pid` exists and is
not ossteam's, the daemon leaves all three alone. This is the case when
an official client is installed, whether or not it is running, because
its symlinks stay when it exits. A launch then fails with `CONFLICT`,
and the message names the entry and says that an official Steam client
owns it. ossteam never removes, renames or replaces it. Using ossteam to
play on a machine with the official client installed means removing that
client's `~/.steam/sdk32`, `sdk64` and `steam.pid` while it is not
running. The official client recreates them when it starts.

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

### 1.3 The client libraries

The daemon finds its client libraries in the directory given by
`OSSTEAM_STEAMCLIENT_DIR`, or else in the directory of its own
executable:

| OS | 64-bit | 32-bit |
|---|---|---|
| Linux | `libossteam_steamclient.so` | `lib32/libossteam_steamclient.so` |
| Windows | `ossteam_steamclient.dll` | `lib32\ossteam_steamclient.dll` |

The 64-bit library is required. Without it, no entries are created, and
a launch fails with `UNSUPPORTED`, with a message that the client
library is not installed. The 32-bit library is optional. Without it,
`sdk32` (Linux) and `SteamClientDll` (Windows) are not created, and a
32-bit game's `steam_api` finds no client, as under an official client
that lacks one.

### 1.4 Lifetime

The daemon creates the entries once the session has an active account,
and removes them when it has none any more (a logout) and when the
daemon exits (DISC-1). An account stays active while the connection is
down and the daemon reconnects, so the entries stay too: the game's
`steam_api` then finds a client that reports being logged off (NET-4),
not a missing one. On start, before anything else, the daemon removes
entries that are ossteam's but hold a pid that is not its own, which a
crash leaves behind (DISC-2).

macOS entries are established in M1e. Until then a launch on macOS fails
with `UNSUPPORTED`.

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
  are packed to 1 byte.

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
empty, it locks one mutex and checks one queue (§20: at most 5 µs).

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
| Linux | Unix socket `$XDG_RUNTIME_DIR/ossteam/game.sock` |
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
whether or not an account is logged in.

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

Protocol version 1.

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

`Dlc`: `app_id: u32`, `owned: bool`, `installed: bool`, `available:
bool` (its info says it is released), `name: string` (empty when its
info is not known). A DLC is installed when it is owned, the app is
installed, and the DLC has no depots of its own in the app's info or
one of them is installed.

`RequestBody` and `ResponseBody` start with their own tag:

| Tag | Request | Response |
|---|---|---|
| 1 | `AppState{app_id: u32}` | `AppState{owned: bool, installed: bool, install_dir: string, build_id: u32, depots: list<u32>}` |
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
any game. Requests that change something, such as storing stats (M1c),
act only on the connection's own app id.

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
| `ISteamUser` | `isteamuser.md` | logon state and Steam id; tickets follow |
| `ISteamApps` | `isteamapps.md` | ownership, DLC, branch, build, install directory, languages |
| `ISteamUtils` | `isteamutils.md` | app id, country, server time, API-call plumbing, overlay state |
| `ISteamFriends` | `isteamfriends.md` | own persona name and state |
| `ISteamRemoteStorage` | `isteamremotestorage.md` | cloud state; local files follow |
| `ISteamUserStats` | follows | stats and achievements |

## 6. Verification against the official client

| Item | Status |
|---|---|
| The exports `steam_api` uses (§2.1) | found in the official library and in `libsteam_api.so`; signatures from Proton |
| The calling conventions (§2.2) | the platforms' C++ ABIs; Windows from Proton's wrappers |
| The vtables (§2.4) | from Proton's `lsteamclient`; to be checked by the ABI tests against games |
| Pipe and user handle values (§2.3) | unverified |
| `ConnectToGlobalUser` for an app the account does not own (§2.3) | unverified |
| Results kept after a mismatched `Steam_GetAPICallResult` (§2.5) | unverified |
| The empty string for unimplemented `const char *` methods (§2.4) | unverified |
| `SteamServersConnected_t` and `SteamServersDisconnected_t` timing (§3.7) | unverified |
| Play sessions of games not started by the client (§3.5) | unverified |
