# The graphical client

| | |
|---|---|
| Status | Draft for M1e. ossteam's own design; nothing here is observed from the official client |
| Requirements | SPEC §16 (GUI-1 to GUI-4), LEAF-1 to LEAF-3, CONC-3, D-2, §20 |
| Code | `crates/ossteam-gui` |

`ossteam-gui` is a leaf client (LEAF-1): it shows what the daemon reports
and asks the daemon for every change. This page covers how it talks to
the daemon, what it keeps, its views, the IPC each view uses, and its
keyboard map. The look of the official client is not a goal; what the
GUI shows about an account, an app or a job must be what the daemon
reports, and the same thing the CLI would print.

## 1. Window and navigation

One window. From top to bottom:

- a header with the three pages, Library, Downloads and Settings, and
  the account menu (§4.6);
- the page;
- the status bar (§4.7).

The App page (§4.3) opens from the library and replaces the Library
page until it is closed. Notices (§2.4) appear over the bottom right
corner of the page.

Two states replace the whole window content:

- **No daemon.** While the GUI connects, and whenever it cannot reach
  the daemon, it shows why instead of empty pages (§16.2): the
  connection error, or "ossteamd stopped" after `DaemonStopping`. A
  Retry button connects again and, unless the GUI was started with
  `--no-autostart`, starts the daemon (D-2).
- **No account.** While the daemon has no active account, or the active
  account needs to log in again (`SessionChanged.login_required`, or
  `GetStatus.session_reason` with no logged-on account), the window shows
  the Login page (§4.1). Downloads and Settings stay reachable from it,
  because folders and settings do not need an account.

## 2. The daemon connection

### 2.1 Threads

The GUI holds one `ossteam_ipc::Client` (CONC-3):

- A **reader thread** takes events from the client and forwards them into
  an async channel. A gpui task on the main thread drains it into the
  store (§3).
- **Calls** run on a pool of four GUI-owned threads, and their results
  come back to the view that made them through a oneshot channel awaited
  in `cx.spawn`. gpui's background executor is not used for calls: a
  call can block for its whole timeout (30 s, 90 s for logins), and the
  executor's threads also decode images, so blocked calls would stall
  the library's artwork.
- `Launch` waits without a timeout while compatibility tools install
  (`launch.md` §8), so each launch gets a thread of its own.

### 2.2 Connecting

On start, and on Retry, the connection thread connects (starting the
daemon unless `--no-autostart`), subscribes to every topic (`DAEMON`,
`SESSION`, `LIBRARY`, `JOBS`, `GAMES`, `NOTICES`), and then reads the
initial state: `GetStatus`, `GetVersion`, `ListAccounts`, `ListApps`,
`ListJobs`, `ListRunning`, `GetSettings` and `ListLibraryFolders`. It
subscribes first so that no change between a read and the
subscription is lost; an event about something not read yet is applied
to the read result once it arrives (§3).

When the connection closes, every page is replaced by the No daemon
state. The GUI does not reconnect on its own: a daemon that stopped may
have been stopped on purpose (ipc.md §6).

### 2.3 Errors

A failed call shows its `Error.message` next to the control that made
it, or as an error notice when there is no such place. `NOT_LOGGED_IN`
also shows the Login page. Errors are never retried silently.

### 2.4 Notices

`Notice` events, and errors that have no control to sit next to, are
listed in the bottom right corner, newest last. Info notices disappear
after 8 s; warnings and errors stay until dismissed.

## 3. The store

The store is a gpui entity holding the daemon's latest answers. It is
the GUI's only copy of daemon state, and it changes only by replies and
events (GUI-2):

| Store field | Read with | Kept current by |
|---|---|---|
| status | `GetStatus` | `SessionChanged` (connection, account, logged on, reason, persona state) |
| accounts | `ListAccounts` | read again after `SessionChanged` and `LoginEnded` |
| library | `ListApps` | read again after `AppChanged`, at most every 250 ms, and when the active account changes |
| app details | `GetApp`, `GetLaunchOptions`, for open App pages only | read again when `AppChanged` names the app, or a job of the app changes state |
| jobs | `ListJobs` | `JobStateChanged` replaces or adds the job; `JobProgress` replaces its progress; read again when the active account changes |
| running games | `ListRunning` | `GameStarted` adds, `GameExited` removes |
| shim reports | `GetApp.last_shim_report` (§5) | `ShimReport` replaces the app's |
| settings | `GetSettings` | `SettingChanged` replaces the setting |
| folders | `ListLibraryFolders` | read again after a job of any app changes state, and after adding or removing a folder |

Views keep only view state: the page shown, text typed, selection,
scroll position, which menus are open, and the download rates of §4.4.

When the active account changes, the store drops the library, jobs,
app details and shim reports of the old one and reads them again; an
open App page closes.

A reply and the event about the same change reach the store by
different paths (§2.1), so a reply can arrive after a newer event. A job
a reply carries (`Install`, `PauseJob` and the like) is therefore added
only when the store does not have that job yet; otherwise the job's
`JobStateChanged` decides. This shows a new job at once without letting
a late reply undo a newer state (GUI-2).

A login started here takes `LoginPrompt` events from the moment the
request is sent, for the same reason; prompts of a login another client
started are not shown.

## 4. Pages

### 4.1 Login

Three parts:

- **Accounts.** The accounts of `ListAccounts`, each with its persona
  name and account name. Choosing one calls `SwitchAccount`. An account
  whose token was rejected is not listed by the daemon; it logs in again
  through the form.
- **Sign in.** Two tabs. *Password*: account name, password and
  "Remember me" (default on), sent with `LoginWithCredentials`. *QR
  code*: `LoginWithQr`, then the QR code of each `LoginPrompt` with
  `QR_URL`, with the text "Scan with the Steam mobile app". The code is
  drawn from the URL in the GUI; it is never fetched.
- **Steam Guard.** A `LoginPrompt` replaces the form with what it asks:
  a code field for `EMAIL_CODE` (naming `hint`'s domain) or
  `DEVICE_CODE`, sent with `SubmitGuardCode` and the prompt's kind; for
  `DEVICE_CONFIRMATION`, the text that the sign-in waits for approval in
  the mobile app, or by email when `hint` says so. When both a code and
  a confirmation are offered, both show.

Cancel calls `CancelLogin`. `LoginEnded` with an error shows it above the
form and keeps the account name; with an account, the GUI goes to the
Library page. The password field is cleared as soon as the request is
sent, and its buffer is zeroed (IPC-10).

### 4.2 Library

The apps of `ListApps`, top-level only; DLC appear on their parent's App
page.

- **Search** sends `ListApps{filter}`, 150 ms after the last key press,
  so that the GUI matches the CLI's `library <filter>` exactly (LEAF-3).
- **Filters:** *Installed* (`installed_only`) and *Updates*
  (`AppSummary.update_available`, §5; filtered in the view, because
  `ListApps` has no such parameter).
- **Grid** (default) and **list** layouts, both virtualised with
  `uniform_list`: the grid as rows of capsules, the list as one row per
  app with its name and install state. (`AppSummary` has no size; the
  App page shows it.)
- **Artwork.** A grid cell asks for `GetAsset{CAPSULE}`, with `high_dpi`
  when the window's scale factor is above 1, only while the row is
  visible, at most eight requests at once, the most recently asked for
  first, and once per app and kind for the window's lifetime. Until the
  image arrives, and when `GetAsset` fails or the file does not decode,
  the cell shows the app's name on a plain background.
- **Capsule images** are decoded on a background thread and scaled to
  fill the cell in device pixels, cropping what overflows. The 256 most
  recently drawn are kept and the others are dropped from memory and
  from the GPU's atlas. gpui's own image loading keeps every image at
  full size until the app exits: scrolling 10,000 capsules with it held
  11.7 GB.
- While `ListApps.complete` is false the page says that the library is
  still loading. `packages_without_info` is shown as a count, with the
  ids in its tooltip.

Choosing an app opens its App page.

### 4.3 App

From `GetApp` and `GetLaunchOptions`, plus the app's jobs and running
game from the store:

- **Header:** `GetAsset{HERO}` with `LOGO` over it; `HEADER` when the app
  has no hero; the name alone when neither exists. These are decoded as
  capsules are (§4.2), at the size they are drawn in device pixels: the
  hero to fill the window's width, rounded up to 64 pixels, and 240
  points high; the logo to fit in 320 × 140 points without being
  enlarged; the header to fill 460 × 215 points. The eight most
  recently drawn are kept, for every App page together.
- **State and main action:**

  | State | Action |
  |---|---|
  | Not installed | Install, with a folder chooser (`ListLibraryFolders`, free space shown) and a branch chooser (`branches` that need no password; `public` first) |
  | A job is unfinished | The job's progress and Pause/Resume/Cancel, as on the Downloads page |
  | Installed, update available | Update, and "Play anyway" (`Launch{allow_stale}`) |
  | Installed | Play |
  | Running | Stop (`StopGame`) |

- **Launch options.** When `GetLaunchOptions.single` is false, Play opens
  a chooser of the options that qualify, with their descriptions. An
  "Arguments" field adds `extra_args`, split like a shell would split
  them, without expansion.
- **Details:** install path, folder, branch, build, size, depots, and the
  DLC with their install state.
- **Compatibility (Linux):** the tool from `GetApp.compat`, where it comes
  from, and its chain with each tool's install state. A chooser of
  `compat.choices` and "Default" sets `compat.tool.<app id>` with
  `SetSetting`, or resets it.
- **Manage:** Verify; Uninstall, which asks for confirmation, with "Also
  delete the Proton prefix" on Linux (`remove_compat_data`).
- **Last run:** the app's last `ShimReport` (§5): interfaces used, versions
  not provided, and unimplemented calls with their counts. Nothing is
  shown for an app with no report.

Escape or Back returns to the library with its scroll position and
search kept.

### 4.4 Downloads

The jobs of `ListJobs`, in two lists:

- **Queue:** unfinished jobs (`QUEUED`, `RUNNING`, `PAUSED`, `FAILED`) in
  the order the daemon runs them: priority, highest first, then creation
  time. Each shows the app's name, kind, phase, a progress bar of
  `bytes_done / bytes_total` (`downloaded / download_total` while
  `DOWNLOADING`), the rate, and the error of a failed job. Pause, Resume
  and Cancel call their methods.
- **Reordering.** Move up, Move down, and Move to top. The GUI computes
  the queue order it wants and the fewest `SetJobPriority` calls that
  produce it: from the bottom up, a job keeps its priority if it already
  sorts before the job after it, and is otherwise raised just enough
  (to that job's priority if it was created earlier, else one above).
  Jobs only rise, so what is below stays in order.
- **Rate** is view state: the change of `downloaded` (file bytes while
  not downloading) over the `JobProgress` events of the last 3 s. The
  time left is derived from it and shown only after 3 s of samples.
- **History:** `DONE`, `CANCELLED` jobs, newest first. The daemon keeps them
  until it restarts (ipc.md §5.6), and so does the page.
- **Update all** calls `Update{all}`.

### 4.5 Settings

Sections:

- **Library folders:** `ListLibraryFolders` with free space and app count;
  Add (a system folder picker, then `AddLibraryFolder`) and Remove.
- **Downloads:** `content.bandwidth_limit`, `content.concurrent_jobs`,
  `content.download_threads`, `content.auto_update`, `content.language`.
- **Compatibility (Linux):** `compat.default_tool`, and the
  `compat.tool.<app id>` overrides, each with its app's name and a reset.
- **Account:** persona state (`SetPersonaState`), Log out (`Logout`,
  confirmed).
- **Daemon:** version, pid, uptime and protocol; every setting not shown
  above; and Stop ossteamd (`Shutdown`; when it fails with `CONFLICT`
  because games run, the GUI offers to stop them too with `force`).

Every setting shows its value, where it comes from (default, file or
override) and, for an override, a reset. Booleans are switches; integers
and strings are text fields that send `SetSetting` on Enter or when focus
leaves, and show the daemon's `INVALID_ARGUMENT` message on failure.

### 4.6 Account menu

The active account's persona name and state; the other remembered
accounts (`SwitchAccount`); Add account (the Login page; starting a
login there logs the active account off first, ipc.md §5.2); Log out.

### 4.7 Status bar

The connection state, the active account and whether it is logged on,
a summary of unfinished jobs (the running job's app and percentage, and
how many more are queued), and the running games, each with Stop.

## 5. IPC additions

The pages need two fields the protocol did not have (ipc.md §9 allows
adding fields within major version 1):

- `AppSummary.update_available` (field 8): the installed app's branch has
  manifests other than the installed ones, as in
  `InstalledApp.update_available`. False when not installed. It feeds the
  Updates filter, and lets the CLI's `library` mark such apps.
- `GetAppResponse.last_shim_report` (field 11): the last `ShimReport` the
  daemon sent for the app, from any process. The daemon keeps one per app
  and account in its state database, so a report survives restarts of
  the daemon and of the GUI. `info` prints it as well, so the CLI keeps
  parity (LEAF-3).

## 6. Keyboard and accessibility (GUI-4)

Every action is reachable from the keyboard. Tab and Shift-Tab move
focus through the page in reading order; Enter and Space activate the
focused control; Escape closes a menu, a chooser or the App page. While
focus moves by keyboard, the focused control has a visible focus ring;
a text field shows it whenever it has focus.

| Keys (Linux, Windows) | macOS | Action |
|---|---|---|
| Ctrl-1, Ctrl-2, Ctrl-3 | Cmd-1, Cmd-2, Cmd-3 | Library, Downloads, Settings |
| Ctrl-F | Cmd-F | Focus the library search |
| Arrows, Home, End, Page Up, Page Down | same | Move the library selection |
| Enter | Return | Open the selected app |
| Ctrl-Enter | Cmd-Return | Play or install the selected or open app |
| Ctrl-Q | Cmd-Q | Quit (the daemon keeps running) |

Colours follow the system's light or dark appearance and change when it
does. Sizes are in logical pixels, and images are requested at double
size when the scale factor is above 1.

## 7. Testing

- View logic that needs no window (ordering and reprioritising the queue,
  rates, the store's event handling, argument splitting, QR matrices) is
  in plain functions with unit tests.
- Store tests drive the event handling with recorded events.
- An integration test runs the GUI's connection and store against a real
  daemon with the fake CM (FAITH-4), with gpui's test platform instead of
  a window.
- The §20 budgets are measured with `OSSTEAM_GUI_STARTUP_TIMING` (first
  frame) and with a library of 10,000 apps from the fake CM (scrolling),
  on the reference hardware. The library is served by the ignored daemon
  test `gui_bench_library_of_10k_apps`, which puts a capsule for every app
  in the daemon's image cache and prints how to point the GUI at it. With
  `OSSTEAM_GUI_BENCH=library-scroll` the Library page scrolls the grid,
  then the list, from top to bottom by one row per frame, prints each
  run's frame work (from the start of the frame to the end of the page's
  paint) and frame intervals, and quits. A frame interval over one and a
  half display refresh intervals counts as a missed frame. The budget is
  met when no frame's work is over 16 ms and no frame is missed.
- The M1a to M1d checklists are run through the GUI (`docs/acceptance/m1e.md`).
