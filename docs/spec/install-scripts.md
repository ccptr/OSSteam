# Install scripts

| | |
|---|---|
| Status | Draft for M1b; nothing yet tried against the official client |
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
only on 32-bit Windows) and `OSType` (a Windows version name such as
`WinXP` or `Win7`). An entry whose conditions do not hold is skipped.
*Unverified:* the full list of `OSType` names and whether a version
matches later versions too.

An entry runs when it has no `HasRunKey`, or when the value under that key
is missing or below `MinimumHasRunValue`. The client calls `CreateProcess`
with the process and its arguments, and falls back to `ShellExecuteEx` if
that fails. An exit code of 0 counts as success, and then the value under
`HasRunKey` is set to 1. Any other exit code leaves it unset, so the entry
runs again next time.

### 2.3 Firewall

Each key under `Firewall` names a program and its value is the program's
path. The client adds an inbound exception for it in Windows Firewall, so
the user is not asked when the game first listens. *Unverified:* the
profile and protocol of the rule the official client makes.

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

## 3. When scripts run

| Event | What happens |
|---|---|
| Install or update finishes | Nothing runs yet. The app's scripts are recorded with the install (§1) |
| The game starts (M1c) | Before the game process is created: Registry, then Firewall, then each Run Process entry that is due. The launch waits for them, and shows the step under way |
| Verify | Registry values the scripts wrote and the `HasRunKey` values (unless `NoCleanUp`) are removed, so everything runs again at the next start |
| Uninstall | Run Process On Uninstall entries run, then the same removal as verify, then the files are deleted |

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

Scripts are applied on native Windows only. Under Proton (M1d) they run
inside the prefix, through Wine's registry and process handling; that is
specified with the compatibility layer.

## 4. Elevation

A step needs elevation when it writes `HKEY_LOCAL_MACHINE`, adds a
firewall rule, or is a Run Process entry without `AsCurrentUser` whose
program asks for it in its manifest. ossteam gathers the steps that need
elevation for one start and runs them in a single elevated helper
(`ossteamd --script-helper`, started with `ShellExecuteEx` and the
`runas` verb), so the user sees one UAC prompt rather than one per step.
The helper reads its steps from a file the daemon writes, runs them and
writes their results back; it does nothing else. If the user declines,
the elevated steps are reported as not run, and they are tried again at
the next start. Steps that need no elevation run in the daemon's own
user context.

## 5. State

The daemon records, per installed app, the scripts it found (depot, path,
SHA-1) and, per script, the registry values it wrote, so verify and
uninstall remove exactly those. The `HasRunKey` values themselves stay in
the registry, where they are shared with the official client: a
redistributable it already installed is not installed again.

## 6. Errors

| Case | Behaviour |
|---|---|
| The script does not parse | Not run; logged with the depot and path |
| The file does not match the manifest | Not run; the launch reports that the app needs a verify |
| An unknown action key | Ignored and logged |
| A Run Process program is missing | That entry fails; the others run |
| The user declines UAC | The elevated steps do not run; the launch continues |

## 7. Verification against the official client

| Item | Status |
|---|---|
| Section and key names, case handling | from the documentation |
| `process N` beyond 1 | unverified |
| `OSType` names | unverified |
| Variables beyond those listed | unverified |
| Registry and Firewall at every start | unverified |
| Order of scripts | unverified |
| Firewall rule shape | unverified |
| Signature algorithm | unknown; not checked (§2.6) |
