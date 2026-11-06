# Install scripts

| | |
|---|---|
| Status | Draft; verify and uninstall built in M1b, the start in M1c; nothing yet tried against the official client |
| Requirements | SPEC §10 (CONT-8), SEC-3, §19.3 |
| Code | parsing and planning in `crates/ossteam-content/src/script.rs`; running them on Windows in `crates/ossteamd/src/content/script/` |

A depot can carry **install scripts**: KeyValues files that ask the client
to write registry values, add firewall exceptions and run installers
(DirectX, the MSVC runtime, a game's own prerequisites) before the game
first starts, and to clean up when it is uninstalled. They are a Windows
feature. The Steamworks documentation ("Install Scripts") states that
there is no install script support on Linux, and on macOS only file
permissions and symlinks, which ossteam already gets from the manifest
(`content.md` §4).

**Verification.** This draft comes from Valve's Steamworks documentation,
which describes the format for developers, and from scripts as they ship
in public depots. The documentation describes what a script asks for, not
every detail of how the client does it, so the client's behaviour where
the documentation is silent is marked *unverified* and must be checked
against the official client (§7) before it is relied on.

## 1. Finding the scripts

A script is a file in a depot whose manifest flags include `0x100`
(`InstallScript`, `content.md` §4). The developer marks it in the depot's
build script. The scripts of an app are the flagged files of every depot
installed for it, including depots installed from another app's info
(`depotfromapp`, `content.md` §6), which is how the Steamworks Common
Redistributables (app 228980) bring theirs.

Each script is read from the installed file. Before it is used, the file's
size and SHA-1 must match the installed manifest's `sha_content`; a file
that does not match is not run, and the app needs a verify.

## 2. Format

The file is text KeyValues (`keyvalues.md`) with one root key,
`InstallScript`. Keys and values use the escape sequences, so a
backslash in a path is written `\\`. Each key under the root names a kind
of action:

```
"InstallScript"
{
    "Registry"
    {
        "HKEY_LOCAL_MACHINE\\Software\\Example\\Game"
        {
            "string" { "InstallPath" "%INSTALLDIR%" }
            "dword"  { "Language" { "english" "1" "french" "2" } }
        }
    }
    "Run Process"
    {
        "DirectX"
        {
            "HasRunKey"     "HKEY_LOCAL_MACHINE\\Software\\Valve\\Steam\\Apps\\CommonRedist\\DirectX\\Jun2010"
            "process 1"     "%INSTALLDIR%\\_CommonRedist\\DirectX\\Jun2010\\DXSETUP.exe"
            "command 1"     "/silent"
            "NoCleanUp"     "1"
        }
    }
    "Firewall" { "Example Game" "%INSTALLDIR%\\game.exe" }
    "Run Process On Uninstall" { ... }
}
"kvsignatures" { "InstallScript" "<hex>" }
```

Keys are matched without regard to case, as KeyValues keys are.

### 2.1 Registry

Each key under `Registry` is a full registry key path, starting with the
hive (`HKEY_LOCAL_MACHINE`, `HKEY_CURRENT_USER`; *unverified*: whether
other hives and abbreviations such as `HKLM` are accepted). Under it:

- `string`: value name to string data (`REG_SZ`).
- `dword`: value name to a decimal number (`REG_DWORD`).
- The value name `(Default)` sets the key's default value.
- A value can be an object keyed by language name instead of a string.
  The entry for the account's language (`content.language`) is used,
  falling back to `english`. *Unverified:* what the client does when
  neither is present; ossteam then writes nothing for that value.

The official client is a 32-bit program, so on 64-bit Windows its writes
under `HKEY_LOCAL_MACHINE\Software` land in the 32-bit view
(`Wow6432Node`), as the Steamworks FAQ notes. ossteam is 64-bit, so it
opens these keys with `KEY_WOW64_32KEY` to write where games expect
them.

### 2.2 Run Process

Each key under `Run Process` is one entry, named by the developer. Its
keys:

| Key | Meaning |
|---|---|
| `process 1` | Required: the program to run |
| `command 1` | Arguments for it |
| `HasRunKey` | A registry key path. A value under it records that the entry has run; ossteam reads the value named after the entry (*unverified*: which value name the official client uses) |
| `MinimumHasRunValue` | The entry runs unless the recorded value is at least this (default 1) |
| `NoCleanUp` | When `1`, the record is kept when the app is uninstalled or verified |
| `AsCurrentUser` | When `1`, the program runs as the user; otherwise the client may run it elevated |
| `Requirement_OS` | Conditions on Windows; see below |

*Unverified:* whether `process 2`, `command 2` and higher are honoured,
and in which order. The documentation and the scripts seen so far use
only `1`.

`Requirement_OS` holds `Is64BitWindows` (`1` or `0`: only on 64-bit, or
only on 32-bit Windows) and `OSType` (a Windows version name). An entry
whose conditions do not hold is skipped. The .NET 4.7 script of the
Steamworks Common Redistributables (depot 229006) writes `OSType` as
`Windows XP`, where this page first assumed `WinXP`. ossteam compares
the names without spaces and case, with a leading `Windows` read as
`Win`, so both forms match its `WinXP`. *Unverified:* the full list of `OSType` names and
whether a version matches later versions too.

Two entries may have the same name: that .NET script has two `4.7.2`
entries with the same `HasRunKey`, the first only for Windows XP (it
runs `noop.cmd`), the second for every Windows. Each is an entry of its
own, taken in the order of the file.

An entry runs when it has no `HasRunKey`, or when the value under that key
is missing or below `MinimumHasRunValue`. Proton's prefixes come with
such records for the redistributables Wine provides itself, in the
form ossteam reads: under
`HKEY_LOCAL_MACHINE\Software\Wow6432Node\Valve\Steam\Apps\CommonRedist\.NET\4.7.2`
the `REG_DWORD` value `4.7.2` = 1, and likewise for the other .NET
versions and XNA 4.0 (a fact from Proton's prefix, not from the official
client). The client calls `CreateProcess`
with the process and its arguments, and falls back to `ShellExecuteEx` if
that fails. An exit code of 0 counts as success, and then the value under
`HasRunKey` is set to 1. Any other exit code leaves it unset, so the entry
runs again next time.

### 2.3 Firewall

Each key under `Firewall` names a program and its value is the program's
path. The client adds an inbound exception for it in Windows Firewall, so
the user is not asked when the game first listens. *Unverified:* the
profile and protocol of the rule the official client makes.

ossteam adds the rule with Windows' own `netsh`:
`netsh advfirewall firewall add rule name="<name>" dir=in action=allow
program="<path>" enable=yes`, for every profile and protocol. A rule of
that name for that program is looked for first (`netsh advfirewall
firewall show rule name="<name>" verbose`); when there is one, the step
is not planned.

### 2.4 Run Process On Uninstall

The same entries as `Run Process`, run when the app is uninstalled, before
its files are deleted. The documentation tells developers not to remove
shared redistributables here.

### 2.5 Variables

These are replaced in paths, arguments and string data before use:

| Variable | Value |
|---|---|
| `%INSTALLDIR%` | The app's install directory, no trailing backslash |
| `%ROOTDRIVE%` | The drive of the install directory (`D:`) |
| `%STEAMPATH%` | The client's install directory; for ossteam, the generated client-install directory (SPEC §17) |
| `%WinDir%` | The Windows directory |
| `%APPDATA%`, `%LOCALAPPDATA%` | The user's roaming and local application data |
| `%USER_MYDOCS%`, `%COMMON_MYDOCS%` | The user's and the public Documents folder |

*Unverified:* the full list of "standard Windows shell folder" variables
the documentation mentions, and how a variable it does not know is left.
ossteam leaves an unknown `%NAME%` as it is and logs it.

### 2.6 Signatures

At build time Steam signs the script and adds a `kvsignatures` block with
an `InstallScript` value in hex. The official client checks the signature
before it runs a script, and requires it for writes to
`HKEY_LOCAL_MACHINE`. The signature algorithm and Valve's key are not
documented (*unverified*), so ossteam cannot check the signature.

ossteam relies on the content checks instead. The script file must match
the installed manifest (§1), and the manifest was checked when it came
from the CDN (`content.md` §4). The signature also guards a privilege
that ossteam does not have. The official client writes `HKEY_LOCAL_MACHINE`
through an elevated service, without asking the user. ossteam has no such
service: every elevated step shows a UAC prompt (§4), so the user agrees
to each one. A script without a `kvsignatures` block gets no
`HKEY_LOCAL_MACHINE` writes, as in the official client.

The maintainer accepted this for now on 2026-11-03, and will look into
how the signature works later. Until then the question stays open: if
the algorithm and key can be learned faithfully, ossteam checks the
signature as well.

## 3. When scripts run

| Event | What happens |
|---|---|
| Install or update finishes | Nothing runs yet. The app's scripts are recorded with the install (§1) |
| The game starts (M1c) | Before the game process is created: Registry, then Firewall, then each Run Process entry that is due (§3.1). The launch waits for them, and shows the step under way |
| Verify | After the files are repaired: registry values the scripts wrote and the `HasRunKey` values (unless `NoCleanUp`) are removed, so everything runs again at the next start |
| Uninstall | Run Process On Uninstall entries that are due run, then the same removal as verify, then the files are deleted. A failed entry or a declined prompt does not stop the uninstall |

The Steamworks documentation states that the client "scans mounted
depots for flagged files and runs them" when a game starts, and that Run
Process entries run before the first launch. *Unverified:* whether
Registry and Firewall entries are applied again at every start or only
the first, and the order of scripts within an app. ossteam applies them at
every start, which is harmless because the values are the same, and runs
scripts in order of depot id, then path.

A Run Process entry that fails does not stop the launch: the game may
work without it, and the entry runs again next time. *Unverified:* what
the official client shows or does in that case.

Scripts are applied on native Windows, and on Linux to the prefix of an
app launched through Proton (`compat-tools.md` §7).

### 3.1 At the start

Once the launch's checks have passed and its tools are installed
(`launch.md` §2), the daemon reads the app's scripts (§1) and plans:

1. **Registry.** Every value of every script, in order. A value under
   `HKEY_LOCAL_MACHINE` is left out when its script has no
   `kvsignatures` block (§2.6). A value that already holds what the
   script writes (the same type and data) is left out.
2. **Firewall.** Every rule that is not there yet (§2.3).
3. **Run Process.** Every entry whose conditions hold and that is due
   (§2.2).

The steps then run as §4 says. The values written are recorded with the
install (§5), whether the launch then goes on or not. Nothing in the plan
stops the launch: a step that fails, or one the user declines, is logged
and runs again at the next start, and the game starts after the last
step.

A script that does not match the installed manifest stops the launch
with `NOT_FOUND` and a message that the app needs a verify, as a missing
executable does (`launch.md` §2). One that does not parse is logged and
left out (§6).

While the steps run, the launch is under way, as one waiting for its
tools is: its reply waits, and a second `Launch` of the app is refused
with `CONFLICT`. Each step that starts is announced on `GAMES` as
`LaunchStep` (`launch.md` §8). The daemon's shutdown does not wait for
them: the connection closes without a reply, as for a launch waiting for
its tools, and a program under way runs on, unwatched.

## 4. Elevation

A step needs elevation when it writes or deletes a value under
`HKEY_LOCAL_MACHINE`, adds a firewall rule, or is a Run Process entry
without `AsCurrentUser`. The official client runs those entries through
its elevated service; ossteam has none, so it runs them elevated rather
than guess from the program's manifest which ones need it. `HasRunKey`
is set by the process that ran the entry.

ossteam gathers the elevated steps of one start, verify or uninstall
and runs them in a single elevated helper (`ossteamd --script-helper`,
started with `ShellExecuteEx` and the `runas` verb), so the user sees one
UAC prompt rather than one per step. Steps are taken in order, except
that every elevated step runs in the helper at the place of the first.
Steps that need no elevation run in the daemon's own user context: a
user who answers the prompt with an administrator's credentials gets the
helper in that account, where `HKEY_CURRENT_USER` is another user's.

The helper's steps travel on its command line, JSON in hex, which the
daemon sets and nothing can change once the helper starts; a file could
be changed between the daemon writing it and the helper reading it. The
helper writes each step's outcome to a file the daemon names and reads
back, and does nothing else. If the user declines, the elevated steps are
reported as not run, and they are tried again next time.

A step is not planned when it would change nothing: a value to delete
that is not there, or (M1c) a value that already holds what the script
writes. So a verify of an app whose scripts have not run asks for no
elevation.

## 5. State

The daemon records, per installed app, the scripts it found (depot, path,
size, SHA-1) and the registry values they wrote, so verify and uninstall
remove exactly those. A value whose removal did not happen (the user
declined, or it failed) stays recorded for the next verify. The `HasRunKey` values themselves stay in
the registry, where they are shared with the official client: a
redistributable it already installed is not installed again.

## 6. Errors

| Case | Behaviour |
|---|---|
| The script does not parse | Not run; logged with the depot and path |
| The file does not match the manifest | Not run; the launch stops with `NOT_FOUND` and says that the app needs a verify (§3.1); verify and uninstall go on without it |
| An unknown action key | Ignored and logged |
| A Run Process program is missing | That entry fails; the others run, and the launch goes on |
| The user declines UAC | The elevated steps do not run; the launch continues |

## 7. Verification against the official client

| Item | Status |
|---|---|
| Section and key names, case handling | from the documentation |
| `process N` beyond 1 | unverified |
| `OSType` names | `Windows XP` seen in depot 229006; the rest unverified |
| Variables beyond those listed | unverified |
| Registry and Firewall at every start | unverified |
| Order of scripts | unverified |
| Firewall rule shape | unverified; ossteam's in §2.3 |
| Signature algorithm | unknown; not checked (§2.6) |
| The value name under `HasRunKey` | the entry's name in Proton's prefixes (§2.2); the official client's own writes unverified |
| Entries with the same name | seen in depot 229006; each taken on its own |
| Whether `HasRunKey` applies to Run Process On Uninstall entries | unverified; ossteam applies it |
| Text encodings of script files | unverified; ossteam reads UTF-8 and UTF-16LE with a byte order mark |
| `OSType` of Windows 8.1 and 11 | unverified; ossteam uses `Win81` and `Win11` |
| Whether uninstall removes the firewall rules | unverified; ossteam leaves them |
