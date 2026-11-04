# VAC

| | |
|---|---|
| Status | Draft for M2; awaiting review. No VAC-secured title has been run through ossteam, and no VAC code may land until this page is reviewed (SPEC §14) |
| Requirements | SPEC §14 (VAC-1 to VAC-6), ARCH-4, §19.3, FAITH-1 to FAITH-4, open question 4 (SPEC §24) |
| Code | none yet. `GetIVAC` returns NULL (`steamworks-abi.md` §2.3) and `BIsVACBanned` fails under ABI-3 (`isteamapps.md`) |

Valve Anti-Cheat runs on the player's machine as native code that Valve's
servers send to the client while the player is online. The client loads
that code, calls it when the servers ask, and sends back what it returns.
A VAC-secured game server learns from Steam, through the ticket auth that
ossteam already implements (`isteamuser.md` §2), whether the player's
client has done this. So hosting VAC faithfully means running Valve's
code exactly as the official client runs it, and reporting nothing it did
not produce.

**Verification.** This draft draws on the following sources:

- the message definitions that SteamDatabase extracts from the official
  client (`enums_clientserver.proto`, `steammessages_clientserver_2.proto`,
  `steammessages_vac.steamclient.proto`, revision 89b7738);
- the Steamworks SDK headers that Proton's `lsteamclient` vendors (SDK 1.65);
- the names of symbols, classes and strings in the official Linux client's
  `ubuntu12_32/steamservice.so` and `steamclient.so` (build 1791918634),
  and the memory map of a running official client. No code was
  disassembled;
- SteamKit2's reading of the ban status message, used for facts only;
- community troubleshooting guides for Windows, which are weak evidence and
  are marked as such.

Nothing here has been checked by watching the official client host VAC.
Every statement about how the client or VAC behaves is *unverified* unless
it names its source. §6 lists what has to be observed before code is
written.

## 1. Messages

### 1.1 Module calls

Three messages in the client's message list carry code and calls. Their
fields are in the dumps:

| EMsg | Name | Fields |
|---|---|---|
| 830 | `ClientServiceModule` | `module_crc`, `module_hash`, `module_content` |
| 831 | `ClientServiceCall` | `sysid_routing`, `call_handle`, `module_crc`, `module_hash`, `function_id`, `cub_output_max`, `flags`, `callparameter`, `ping_only`, `max_outstanding_calls`, `app_id` |
| 832 | `ClientServiceCallResponse` | `sysid_routing`, `call_handle`, `module_crc`, `module_hash`, `ecallresult`, `result_content`, `os_version_info`, `system_info`, `load_address`, `exception_record`, `portable_os_version_info`, `portable_system_info`, `was_converted`, `internal_result`, `current_count`, `last_call_handle`, `last_call_module_crc`, `last_call_sysid_routing`, `last_ecallresult`, `last_callissue_delta`, `last_callcomplete_delta` |

The fields suggest a flow, which is *unverified*. The server asks the
client to call function `function_id` of the module named by its CRC and
hash, with `callparameter` as input and at most `cub_output_max` bytes of
output. If the client does not have that module, the server sends it in a
`ClientServiceModule`. The client answers each call with the output and a
result code, and also with facts about itself: the OS version, system
information, the address the module was loaded at, and, if the call
faulted, the exception. The `last_*` fields describe the previous call,
so the server can tell whether calls are lost or out of order.

The response fields are the host's own data, not the module's. Their
formats are unknown. `os_version_info` and `system_info` look like
Windows structures, and the `portable_*` fields like their counterparts
on other systems. Producing them is part of hosting (§2.3).

The neighbouring messages 600 to 610 (`ReqChallenge`, `VACResponse`,
`VSMarkCheat`, ...) and 704 (`ClientVACResponse`) look like an older VAC
protocol. Nothing shows that the current client uses them.

### 1.2 File verification

The unified service `FileVerification` (`steammessages_vac.steamclient.proto`)
has two methods:

- `SignatureCheck`: the client reports a file by name, size, timestamps,
  SHA-1, app id and the result of a signature check. The answer can tell
  it to deny the operation (`deny_operation`). Which files, which
  signature check and which operation are *unverified*.
- `SteamServiceCheck`: the client reports the state of its service
  (`service_status`, `service_repair`), its package version, launcher type
  and OS type. The answer can ask it to repair the service
  (`attempt_repair`). This fits Windows, where the service is a separate
  program (§2.2).

### 1.3 Ban status

`ClientVACBanStatus` (782) is not a protobuf message. SteamKit2 reads it
as a `u32` count followed by that many `u32` app ids. The official
`steamclient.so` has a type named `VacBanBlock_t` and a job named
`CClientJobVACBanStatus2`, which suggests blocks, perhaps ranges of app
ids, rather than single ids. The layout is *unverified*. It is the
source of `ISteamApps::BIsVACBanned` and of the client's ban display.

### 1.4 What a game server learns

A game server checks a player with `BeginAuthSession`. Steam's answer to
the player's client arrives as `ClientTicketAuthComplete` (5429), whose
`eauth_session_response` the client passes to the game. The SDK
(`steamclientpublic.h`) defines, among others:

| Value | Name | SDK comment, paraphrased |
|---|---|---|
| 3 | `k_EAuthSessionResponseVACBanned` | the user is VAC banned for this game |
| 5 | `k_EAuthSessionResponseVACCheckTimedOut` | VAC could not perform its checks on this user |

So a client that does not host VAC is visible to the server as response
5. This is the failure VAC-3 means: the server decides, from Steam's
answer, and ossteam has nothing to report but the truth.

`ClientGamesPlayed` carries `is_secure` for each game, which is whether
the game says its server is secure. `InitiateGameConnection` passes the
same flag (`isteamuser.h`). How it affects VAC is *unverified*.

## 2. The host in the official client

### 2.1 Linux

The 32-bit `steamservice.so` is mapped into the 32-bit `steam` process
(seen in `/proc/<pid>/maps` of a running official client). It contains:

- a module manager (`CClientModuleManager`, `CClientModuleManagerAsync`,
  source files `servicemodulemanagerasync.cpp` and
  `servicemodulemanagerbase.cpp`) with its own thread pool;
- imports of `dlopen`, `dlsym`, `mmap` and `mprotect`, and the strings
  `runfunc` and `/runfunc %u %u %u %u`. A module is most likely a shared
  object with an export named `runfunc` that takes four integers or
  pointers. The module format and that signature are *unverified*;
- `VacProcessMonitor` with a provider class (`CProvider`), maps of
  `ActiveProcessData_t` and `DynamicModule_s`, and the strings
  `/proc/self/maps` and `/proc/%u/status`. So the host itself tracks the
  game's processes and the modules they load. What it reports, and
  whether through the modules or directly, is *unverified*.

`steamclient.so` holds the client's side of the messages
(`CClientJobServiceModuleCall`, `CClientJobSendSteamServiceCheck`,
`ISteamService`).

Because the host is 32-bit, the modules it loads are presumably 32-bit
too, for 64-bit games such as Counter-Strike 2 as well. *Unverified.*

Proton runs a Windows game against this same Linux client, and there is
no Windows host in a Proton prefix. So for a Windows game under Proton,
VAC is whatever the Linux host and its modules do. ossteam would be in the
same position.

### 2.2 Windows

According to community troubleshooting guides, the Windows client has a
service named "Steam Client Service" (`SteamService.exe`) that runs as
the system account, and the usual fix for the error "VAC was unable to
verify your game session" is `SteamService.exe /repair`. That suggests the
modules run in, or depend on, the elevated service, and it fits
`SteamServiceCheck` (§1.2). All of this is *unverified*. It is the
question SPEC §24 Q4 asks: does faithful hosting need an elevated
component?

### 2.3 macOS

Nothing is known yet. Until it is, VAC is unavailable on macOS (VAC-3).

## 3. Planned behaviour

This plan depends on §6 confirming §1 and §2.1.

### 3.1 Structure

- **A separate host process, `ossteam-vac-host`.** The daemon never loads
  module code into itself. A module can fault (the response has an
  `exception_record` field), and a fault must not take the daemon down. On
  Linux the host is built for the architecture of the official host
  (i686 today), which the M1c lib32 build already supports.
- **The daemon only relays.** It passes `ClientServiceModule` and
  `ClientServiceCall` to the host unchanged and sends back what the host
  returns, unchanged. It never reads module content or call results to
  decide anything. It may log their sizes, CRCs and result codes, which
  appear in the clear in the messages anyway.
- **The host does what the official host does, and no more.** It loads a
  module the way the official host loads it, calls `runfunc` with the
  arguments the official host passes, and fills the response's own
  fields (§1.1) with true values in the official formats. It runs with
  the privileges the official host has on that platform: the user's own
  on Linux; on Windows to be decided by §6.3, with VAC-6 applying if they
  are elevated.
- **Module checks.** VAC-1 requires checking modules the way the official
  client does. At least the CRC and hash must match the content before it
  is loaded. Which hash `module_hash` is, and whether there is more (a
  signature), is *unverified*.
- **Process monitor.** If §6 shows that the official host's
  `VacProcessMonitor` produces data that reaches Valve, ossteam must
  produce the same data from the same sources. ossteam already knows its
  game processes (`launch.md`); it must not hide any of them from the
  monitor or add any.
- **File verification and ban status.** `SignatureCheck` and
  `SteamServiceCheck` are sent when and with what the official client
  sends; a `deny_operation` answer is obeyed. `ClientVACBanStatus` feeds
  `BIsVACBanned` and the App page, once its layout is known.

### 3.2 While VAC is not hosted

VAC-5 keeps hosting behind a build-time feature, `vac`, until M2's
acceptance passes. Without the feature, and on any platform where hosting
is not established:

- the daemon reports VAC as unavailable in `ossteam status`, `ossteam
  info` for VAC-secured apps, and the GUI;
- a game server then gets response 5 for the player, and the game fails
  the way it does when VAC cannot verify a session;
- `GetIVAC` keeps returning NULL.

What the daemon does with a `ClientServiceCall` while hosting is off is
open (§7 Q3). Either it does not answer, which is what a client without
the module manager would do, or it answers with a failure result. Which of
the two the servers expect is *unverified*, and neither may claim that a
call ran.

## 4. Risks

- **Bans.** A host that returns wrong or partial results could get an
  account VAC banned. VAC bans are permanent and cannot be appealed.
  Development and acceptance therefore use only the test account, never
  the user's own. A ban on the test account would end live VAC testing
  with it.
- **Running downloaded code.** The host runs native code that arrives over
  the CM connection. This is the same trust the official client places in
  the same channel. The host process limits the damage a crash does; it is
  not a sandbox, and VAC-2 forbids one that would change what the module
  observes.
- **Undocumented host interface.** Faithful hosting needs the exact
  interface between the official host and its modules: the loading
  method, the `runfunc` arguments, any callbacks the host provides, and
  the formats of the response fields. Names and strings do not give
  these. They have to be learned from the official host (§6.2, §7 Q1).

## 5. What ossteam never does (VAC-2, VAC-4)

- Change, patch, skip or reorder a module, a call or a result, or answer a
  call without running it.
- Filter, delay or sandbox what a module can see, or offer any setting
  that does.
- Claim that VAC is running, or that a check passed, when the host did not
  run the call.
- Inject into, hook or read the memory of a game process from ossteam's
  own code. What a module does is Valve's code, run unchanged.
- Run a game that ossteam starts in a way meant to hide it from VAC, or
  change system settings (such as Yama's `ptrace_scope`) in either
  direction.

## 6. Investigation before code

### 6.1 Passive observation in ossteam

The daemon does not record which unsolicited messages it receives. A
debug-level log line per unhandled EMsg (number and size only) would show
whether 782, 830 and 831 arrive at logon, only while a game runs, or only
while the game is on a secure server. This logs nothing a module returns
and changes no behaviour, but it is still VAC-related code, so it waits
for review like the rest (§7 Q2).

### 6.2 The official client on Linux

With the test account logged in to the official client:

1. Start Counter-Strike 2 (free) and join a VAC-secured server.
2. Without attaching to any process, record the `steam` process's
   `/proc/<pid>/maps` every second, to see whether modules are mapped as
   files, as anonymous memory or not at all, and when.
3. Record the official client's logs for the session.

Attaching a debugger or `strace` to the official client while VAC is
active could itself look like cheating to VAC and is not proposed.
Learning the host interface (§4) beyond this needs reading
`steamservice.so`'s code (§7 Q1).

### 6.3 Windows

Repeat §6.2 on Windows: whether the module calls happen in `Steam.exe` or
in `SteamService.exe`, and whether VAC-secured play works with the
service stopped. This answers SPEC §24 Q4.

### 6.4 macOS

Find whether the macOS client has a host at all, and where.

### 6.5 Acceptance

M2's exit for VAC is differential (§19.3): with the `vac` feature, the
test account joins a VAC-secured server through ossteam and is admitted;
without it, it gets the same refusal as a client that cannot verify the
session. Both are compared with the official client on the same server.

## 7. Questions for review

1. **Reading the official host.** Faithful hosting needs the host-module
   interface (§4), which only the official `steamservice.so` shows. Is
   static reading of its code acceptable, for interop and in our own words
   (no Valve code copied)? Without it, VAC stays unavailable on every
   platform under VAC-3, which is a faithful outcome but not M2's goal.
2. **Logging unhandled messages** (§6.1): acceptable before the rest of
   this page is settled?
3. **Calls while hosting is off** (§3.2): no answer, or a failure answer?
   Observation may settle this; until then, no answer is recommended,
   since it claims nothing.
4. **Ban risk.** Is the maintainer's test account acceptable for live VAC
   runs, knowing that a mistake can ban it for good?
5. **Scope.** Should M2 aim for Linux only first, with Windows and macOS
   reporting VAC as unavailable until §6.3 and §6.4 are done?
6. **Games in a namespace of their own.** On Linux every launch runs in a
   user and mount namespace with a private `~/.steam`
   (`steamworks-abi.md` §1.1.2, DISC-4). Does VAC need to know, and is a
   VAC-protected native game acceptable in it, or should such games use
   the real entries?

## 8. Verification against the official client

| Item | Status |
|---|---|
| Module call messages (830, 831, 832) and fields | from the client's message dumps |
| Module call flow | inferred from fields; unverified |
| Formats of the response's host fields | unknown |
| `FileVerification` methods and fields | from the client's message dumps |
| When `SignatureCheck` and `SteamServiceCheck` are sent | unknown |
| `ClientVACBanStatus` layout | SteamKit2 and the client's type names disagree; unverified |
| Auth session responses 3 and 5 | from the SDK headers |
| Linux host is `steamservice.so` in the 32-bit `steam` process | seen in the process map |
| Module manager, `dlopen`, `runfunc`, `VacProcessMonitor` | from symbol and string names |
| Module format and `runfunc` signature | unverified |
| What `VacProcessMonitor` reports | unknown |
| Windows host is the elevated service | community guides only; unverified |
| macOS host | unknown |
