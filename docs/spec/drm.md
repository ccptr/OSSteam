# Steam DRM

| | |
|---|---|
| Status | Draft for M2; awaiting review. No protected title has been run yet, and no DRM code may land until this page is reviewed (SPEC §13) |
| Requirements | SPEC §13 (DRM-1 to DRM-3), §19.3, FAITH-1 to FAITH-4 |
| Code | none yet. M1c's refusal of wrapped executables is in `crates/ossteam-launch/src/steamstub.rs` (`launch.md` §7) |

Steam DRM has two parts that touch the client. **SteamStub** is a wrapper
that Valve's tools put around a Windows executable. **CEG** (Custom
Executable Generation) makes a file of the game specific to the user. In
both cases the protection's own code does the protecting. The client
answers its questions and delivers the files Valve's servers hand out.
This page describes those answers and deliveries, as far as they are
known, and what must be found out before any of them is built.

**Verification.** This draft draws on the following sources:

- Valve's Steamworks documentation of the wrapper ("DRM", partner site);
- Proton's open-source `lsteamclient` and `steam.exe` helper, which run
  Windows games, wrapped ones included, against the Linux client;
- the message definitions that SteamDatabase extracts from the official
  client (`enums_clientserver.proto`, `steammessages_clientserver_2.proto`);
- public analyses of the wrapper's header, used for facts only (their code
  and text are not copied);
- app info in ossteam's own PICS cache.

Nothing here has been checked against the official client, so every
statement about the client's or the protection's behaviour is
*unverified* unless it names its source. §4 lists what has to be observed
before code is written.

## 1. SteamStub

### 1.1 The wrapper

The wrapper adds a section named `.bind` to the PE image and moves the
entry point into it (`launch.md` §7). According to the public analyses,
the section starts with a header that holds:

- the app id the executable was wrapped for;
- the flags chosen when wrapping (§1.2);
- an embedded helper library (`SteamDRMP.dll`), which performs the checks;
- the key and IV that decrypt the game's code section, and the first bytes
  of that section, which the wrapper removed.

So the material that decrypts the game ships inside the executable, and
the client hands over no key. The wrapper decrypts the game with its own
code once its checks pass. When ossteam starts the file unchanged, DRM-2
is met by construction: ossteam never holds the key, never runs the
decryption, and has no reason to touch the image.

### 1.2 Flags

| Bit | Meaning (public analyses) |
|---|---|
| `0x02` | no module verification |
| `0x04` | no encryption of the code section |
| `0x10` | no ownership check |
| `0x20` | no debugger check |
| `0x40` | no error dialog |

Valve's documentation offers the wrapping options 0 (everything), 6
("compatibility mode", which "will disable DRM capabilities of the
wrapper"), 32 (no debugger check) and 38 (both). These match the bits:
6 = `0x02 | 0x04` and 38 = `0x02 | 0x04 | 0x20`. Neither source shows that
anything but `0x10` concerns the client: without that bit, the wrapper
checks ownership through the client. What "module verification" verifies
is unknown (§4.1).

### 1.3 What the wrapper needs from the client

Valve's documentation says the wrapper "verifies game ownership and
ensures that Steamworks features work properly by launching Steam before
launching the game". How it asks is not published. Two observations
narrow it down:

1. **Proton serves wrapped titles with no wrapper-specific code.**
   `lsteamclient` forwards `CreateInterface` and the other exports of
   `steamworks-abi.md` §2.1 to the Linux client library and wraps only the
   Steamworks interface versions it knows. Nothing in it or in Proton's
   `steam.exe` mentions the wrapper. Wrapped Windows titles are widely
   reported to run under Proton (*unverified* here). If so, the helper
   library asks its questions through the client library's public exports
   and Steamworks interfaces. The shim already serves those
   (`steamworks-abi.md` §2), with answers that come from the session
   (ownership, `isteamapps.md`).
2. **Proton's `steam.exe` sets up what a game expects of a running client.**
   Before it starts a game, it creates the events `Steam3Master_SharedMemLock`
   and `Global\Valve_SteamIPC_Class` (the latter "for 2K Launcher"), writes
   the client's pid to `HKCU\Software\Valve\Steam\ActiveProcess\pid`, and
   sets `SteamPath` and `ValvePlatformMutex` in the environment. Which of
   these the wrapper looks at to decide whether the client is running is
   *unverified*. ossteam's Windows discovery entries
   (`steamworks-abi.md` §1.2) write `ActiveProcess` but create none of the
   events.

One more fact from Proton affects Windows in general, not just the
wrapper. Mafia II requires the `ISteamClient` pointer it gets to lie inside
`steamclient.dll`'s image, so `lsteamclient` places that object in the
DLL's `.data` section. Whether any wrapper checks where interface objects
live is *unverified*.

What the wrapper does when it finds no running client is also
*unverified*. The documentation says it "launches Steam". On Windows that
would be the program the registry names as the Steam install. That is not
ossteam's, and DISC-3 keeps ossteam from claiming those keys. §4.1 has to
establish what the wrapper looks for and whether ossteam's entries
satisfy it without impersonating an official install.

### 1.4 Planned behaviour

This plan depends on §4.1 confirming observation 1:

- The wrapper is treated like any other caller of the client library. Its
  calls are served by the shim's existing code paths with answers from
  the session, and no code path exists for the wrapper alone. If §4.1
  finds a call the shim does not implement, that call gets an interface
  page and an implementation, like any other Steamworks gap (ABI-3, ABI-5).
- M1c's refusal (`launch.md` §7) stays in place until §4.1 shows that every
  call a wrapped title makes is implemented. After that the `.bind` check
  no longer refuses; the daemon still records that the executable is
  wrapped, and `ossteam info` and the App page show it.
- An account that does not own the app gets the shim's truthful answer.
  The wrapper then shows its own failure, which must match what it shows
  under the official client (DRM-3, §19.3).
- On Linux, a wrapped Windows title runs under its compatibility tool as
  any other Windows title does (`compat-tools.md`). The wrapper reaches the
  shim through Proton's `lsteamclient`.

## 2. CEG

### 2.1 Which apps use it

An app that uses CEG carries two keys under `config` in its app info:

| Key | Value |
|---|---|
| `cegpublickey` | hex of a DER `SubjectPublicKeyInfo` holding a 1024-bit RSA key |
| `checkguid` | the name of the file that CEG covers |

ossteam's PICS cache holds four such apps: Counter-Strike 2 (730) and
Dota 2 (570), both with `checkguid` `client.dll`; Serious Sam HD: The
Second Encounter Editor (41040), free on demand, with
`SamHD_TSE_SeriousEditor.exe`; and its dedicated server (41005). The
`.dll` name suggests that for 730 and 570 CEG covers the Windows build
only. Whether their Linux builds use CEG is *unverified*. Seen on
2026-11-04 for Counter-Strike 2's Linux build: it has no `client.dll`;
its `libclient.so` and `cs2` carry only the usual ELF sections; and its
files match the official client's install byte for byte, but for files
the game writes. CEG therefore did not change its installed files on
Linux, at least by then.

### 2.2 Messages

The client's message list has a group for downloading DRM material. Their
names and fields are in the SteamDatabase dumps. When they are sent, and
what they carry beyond their field names, is *unverified*.

| EMsg | Name | Fields |
|---|---|---|
| 5485 | `ClientDRMDownloadRequest` | not in the dumps |
| 5574 | `ClientDRMDownloadRequestWithCrashData` | `download_flags`, `download_types_known`, `guid_drm`, `guid_split`, `guid_merge`, `module_name`, `module_path`, `crash_data` |
| 5486 | `ClientDRMDownloadResponse` | `eresult`, `app_id`, `blob_download_type`, `merge_guid`, `download_file_dfs_ip`, `download_file_dfs_port`, `download_file_url`, `module_path` |
| 5487 | `ClientDRMFinalResult` | `eResult`, `app_id`, `blob_download_type`, `error_detail`, `merge_guid`, `download_file_dfs_ip`, `download_file_dfs_port`, `download_file_url` |
| 851 | `ClientDRMProblemReport` | not in the dumps |
| 896, 897 | `ClientDRMBlobRequest`, `ClientDRMBlobResponse` | not in the dumps |

The fields suggest a flow, but it is *unverified*. The client names a
module by its path and by GUIDs. The server answers with a URL to a blob
and a "merge" GUID. The client fetches the blob, combines it with the
installed file and reports the outcome. Several questions are open:

- whether this happens at install time, at launch or when the game asks;
- whether the request comes from the client or from code in the game;
- where the result is stored, and how `cegpublickey` is used to check it.

### 2.3 Consequences for the content engine

If CEG changes the file named by `checkguid` on disk, that file no longer
matches the SHA-1 in the depot manifest. ossteam's verify (`content.md` §11)
would then call it corrupt and "repair" it back to the generic file, and
an update would treat it the same way. Before an app with
`cegpublickey` can be verified or updated faithfully, §4.2 must establish
what the official client does with that file during verify and update.
Until then, ossteam's handling of these apps is the M1 behaviour, which
is correct only if CEG leaves the installed file unchanged.

## 3. What ossteam never does (DRM-2)

- Unpack, decrypt, strip or patch a wrapped executable or a CEG file, on
  disk or in memory, or start a game from a modified copy.
- Answer a protection's question with anything but the session's truthful
  answer, or answer for an app the account does not own.
- Set a wrapper flag, an environment variable or a setting that makes the
  protection skip a check.
- Read the memory of a game process (VAC-4 applies here too).

## 4. Investigation before code

### 4.1 SteamStub

1. **Observation build.** A build-time feature, `steamstub-observe`, would
   let the daemon launch a wrapped executable instead of refusing it. The
   file is still started unchanged. The feature is off by default, and the
   release checklist would require it to be absent from release builds. A
   ShimReport (ABI-5) then lists every interface version the wrapper asked
   for and every unimplemented call it made. The shim's log shows the
   exports it called.
2. **Titles.** Two wrapped titles are needed, on Windows natively and on
   Linux under Proton: one the test account owns and one it does not. The
   daemon's `.bind` check identifies wrapped executables among installed
   apps.
3. **Comparison.** Run the same titles under the official client. Record
   whether each starts, what it shows when it fails, and the client's logs.
4. **Running-client test.** Establish which of the §1.3 markers the wrapper
   needs, by starting it with each one missing in turn under the
   observation build.

### 4.2 CEG

1. With the official client, install and launch 41040 (free on demand) and
   the Windows build of 730. Hash the `checkguid` file before and after the
   launch, and compare it with the depot manifest's SHA-1.
2. Look in the official client's logs (`content_log.txt`, `console_log.txt`)
   for the §2.2 messages and when they occur.
3. Only if 1 and 2 leave the flow unclear, capture the official client's CM
   traffic. Its channel is encrypted, so this needs a method that the user
   approves.

## 5. Questions for review

1. Is the observation build (§4.1 step 1) acceptable, given that it
   launches wrapped executables before the shim is known to serve them?
2. Should the investigation stop at observation? The alternative would be
   reading the embedded helper library's imports. That requires decrypting
   it with the key in the header, in a separate tool, never written into
   the game. Observation alone is recommended.
3. Which wrapped titles can be used, owned and not owned by the test account?
4. Is a CM traffic capture of the official client acceptable for §4.2
   step 3, and by what method?

## 6. Verification against the official client

| Item | Status |
|---|---|
| `.bind` section and header fields | from public analyses |
| Flag bits | from public analyses; options 0, 6, 32 and 38 from Valve's documentation agree |
| The wrapper asks only through the client library's public exports | inferred from Proton; unverified |
| Which running-client markers the wrapper checks | unverified |
| What the wrapper does without a running client | unverified |
| `cegpublickey`, `checkguid` | seen in app info |
| CEG message numbers and fields | from the client's message dumps |
| CEG flow, timing and storage | unknown |
| Whether CEG changes the installed file | unknown |
