# Status

Where each milestone stands, what is left, and what waits on the
maintainer. Commit hashes point to the details. Keep this page current:
it is how a new session (agent or human) picks up work without re-reading
the diffs. See [AGENTS.md](../AGENTS.md) for the rules.

Last updated 2026-11-02 (origin/main at efc15b5; local main is ahead with docs and the macOS discovery fix, not pushed).

## Waiting on the maintainer

Almost all remaining work needs one of these.

- **CI was red on main; a fix waits on a push.** On efc15b5 both macOS
  runners failed `discovery::macos::tests::serves_steam_api` and
  `a_service_someone_else_serves_is_left_alone`: no reply from the Mach
  service. Cause: steam_api's `msgh_bits` 0x1413 makes a *send* right for
  its reply port (0x14 = MAKE_SEND; checked in the SDK 1.65a dylib's
  x86_64 and arm64 code), but steamworks-abi.md §1.3.1 called it
  MAKE_SEND_ONCE and the server answered only send-once rights. Fixed in
  the commit after 661ce95 (the server answers both, and the spec is
  corrected); only type-checked here, so the next CI run is its test.
- **The test account is logged out**, since a failed resume lost its
  token on 2026-10-18 (cause fixed in e4019cc; the fix is verified only
  against the fake CM). Still logged out on 2026-11-02. Every live run
  and every logged-in GUI check waits on it.
- **Discovery entries on the dev machine.** An official Steam install
  owns `~/.steam/sdk64`, so the daemon makes no discovery entries and
  every launch fails with exit 8. M1c/M1d live runs need a decision.
- **install-scripts.md §2.6.** `kvsignatures` on install scripts can't be
  checked (the algorithm is unpublished). §2.6 proposes relying on the
  manifest checks plus a UAC prompt per elevated step. Blocks M1c's
  launch-time HKLM writes and M1d's prefix install scripts; M1b's removal
  does not depend on it.
- **SPEC CONC-2 was reworded**, with an amendment to ADR 0001 (471d6d3).
  Not yet reviewed.
- **reference-hw.md §5** (a5e57fc) asks 5 questions. No §20 row can be
  ticked until a reference machine is chosen.
- **Spec reviews for M2 and M3** (see those sections): drm.md §5, vac.md
  §7, cloud.md §9, friends.md §11, betas.md §9, workshop.md §11,
  offline.md §10.
- **A Mac** for the macOS acceptance runs, and a real Windows machine for
  what Wine can't show.
- **Decisions taken along the way that the maintainer hasn't confirmed:**
  - vtables come from Proton (facts only). `vtables.txt` is in MSVC
    order; build.rs reverses overload runs (GetStat/GetStat_2…) on
    non-Windows targets (00dca80). Unverified against a native game.
  - ConnectToGlobalUser refuses apps the account doesn't own (unverified;
    matters for Spacewar 480).
  - Processes ossteam didn't launch get no play session (PLAY-1).
  - No offline stats cache in M1. UpdateAvgRateStat is unimplemented.
  - Compat tools in the tool list never get a mapping (otherwise the
    default would map SLR itself to Proton).
  - `Client::launch` waits with no timeout, because tool downloads can be
    long (ipc.md §7, launch.md §8).
  - Update detection skips apps whose current depot set is empty, and
    counts a platform change.
  - Native Linux games get `native` = scout SLR at priority 75, as the
    official client's log shows.

## M0: Foundations

Open items 1–2 in `docs/acceptance/m0.md` are closed (ace42f0). The
clipboard failure was Wayland's rule that a selection needs a press
serial; the spike now takes `wtype a` plus `OSSTEAM_SPIKE_CLIPBOARD`. The
two-frame light flash at startup is fixed by `theme::StartupAppearance`
(its own portal query on a thread, waiting at most 50 ms). It was found
with a 165 Hz screen recording and ffmpeg's `signalstats` YAVG. Not
checked on a light desktop.

The DPI item is done on Wayland via a headless sway (efc15b5). Images
were requested once per (app, kind), so a rise in scale kept the 1x art;
bench apps now carry `image2x`. Wayland's first frame is still drawn at
scale 1.

## M1a: Session and library

Idle wakeups (§20): the CM I/O thread capped every poller wait at the 1 s
CONC-2 slice, so it woke 1.03 times a second when logged on and idle.
`CancelToken::on_cancel` (ossteam-types) now wakes the poller: 0.14/s for
CM I/O, 0.23/s for the whole daemon (471d6d3). Dev-box numbers are in
m1a.md (CLI p95 2.4 ms), labelled as not results. The GUI row was dry-run
on the gui_states bench (4a986d3).

Left: the live runs.

## M1b: Content

Committed: engine, downloader, depot selection, daemon jobs and folders,
CLI, fake-CM/CDN daemon tests (`crates/ossteamd/src/testing/m1b.rs`), and
install scripts at verify/uninstall (`crates/ossteamd/src/content/script/`,
schema v5): 7450efa, 62acc30, 5b6db26, 8cdc09f, 8189514. Runbook:
`docs/acceptance/m1b.md`. GUI row dry-run on the bench (abf4bdb..e86b78f;
the bench gained Fresh Extra DLC with its own depot). A resumed job's
progress continues (4d9e34a; journal OSJ2 stores bytes downloaded, OSJ1 is
still read).

Left:
- The Windows script code (registry, CreateProcess/ShellExecuteEx,
  `--script-helper` via runas) passes under Wine but has never run on
  real Windows.
- Launch-time script steps (registry writes, firewall, run process)
  belong to M1c.
- No live runs or captures against Steam yet.

## M1c: Play (native)

Committed: launch layer (90664ad), shim slice 1 (6c41bfa), overload slot
order (00dca80), ISteamUserStats (b4b2808), RemoteStorage local files
(d6acdae), tickets (99a9895). Game IPC is protocol 4. Runbook:
`docs/acceptance/m1c.md` (d0b9ff2).

- b4b2808: daemon `stats.rs` (CM 818/5466, Player.GetGameAchievements,
  icons from
  `shared.steamstatic.com/community_assets/images/apps/<app>/<file>`,
  zune-jpeg); shim `userstats.rs` and `images.rs`; spec
  `docs/spec/steamworks/isteamuserstats.md`. Pipes have `send_then`
  closures; a pipe the daemon closed keeps callbacks and failed results
  (§2.5).
- d6acdae: `remote_storage.rs` writes `<user_dir>/remote/`. The quota
  comes from PICS `ufs` via App.cloud_quota / cloud_max_files. Async
  calls complete locally via `Pipe::local_call`. Nothing is synced (M3).
- 99a9895: daemon `tickets.rs` and `core/tickets.rs` (GC tokens 779,
  ownership ticket 857, ClientAuthList 5432 / ack 5575,
  TicketAuthComplete 5429, encrypted 5526); shim `tickets.rs`; Notice
  message (tag 9). Layout from SteamKit2/node-steam-user. The session
  header words are unverified. An unacknowledged ticket gives
  Timeout(16).
- `cargo xtask build-steamclient [--release] [--target t]` builds 64-bit
  plus `lib32/` (44c244f).
- The §20 pump bench (`ossteam-steamclient/tests/pump.rs`) runs at about
  30 ns on the dev box (b73115c). Unix only. The real figure needs the
  reference hardware.
- C++ ABI test (463253d, SPEC §19.1 level 4, steamworks-abi.md §6.1):
  `tests/abi.rs` plus `tests/abi/game.cpp`. Passes with GCC and Clang,
  x86_64 and i686. Gotchas: CSteamID is pack(1) in the SDK;
  `#pragma pack(push, MACRO)` pushes a label and doesn't expand the macro.
- 49b90f8: the ABI test also runs for Windows x86_64/i686 under Wine
  (game built with `clang --target=*-pc-windows-msvc`, linked by
  `*-w64-mingw32-g++`). It found Windows UserStatsReceived_t and
  ValidateAuthTicketResponse_t with CSteamID at offset 16 instead of 12
  (Proton's generated struct layouts agree on 12; fixed). abi.rs had been
  loading a stale `target/<profile>/` copy of the library; it loads
  `deps/` now.
- Windows job-object, registry and named-pipe code passes under Wine
  only.

Left: launch-time install-script steps (§2.6); live runs (Spacewar 480,
a 32-bit title, playtime on the account).

## M1d: Compat tools (Linux)

Committed: spec `docs/spec/compat-tools.md` (682de1c); tool list,
mappings, manifests and chains in `ossteam-launch/src/compat/` (02c4a02);
settings and install platform, schema 6 (4e7086a); chain launch with a
deferred Launch reply while tool jobs run (e46bba8); GetApp `compat`,
`info` output and `uninstall --remove-compat-data`, schema 7 (67d6d46);
steamclient.so copies and the abstract socket `ossteam-game-<uid>`
(a5fbf2f); runbook `docs/acceptance/m1d.md` (3b87938). Daemon tests:
`crates/ossteamd/src/testing/m1d.rs`.

Left:
- Install scripts in the prefix (compat-tools §7), blocked on §2.6.
- Live runs. Nothing has run under a real Proton, SLR or container yet.
- The `use_tool_subprocess_reaper` SIGINT stop path is untested against a
  real tool.

## M1e: GUI and macOS

Committed: spec `docs/spec/gui.md` and IPC schema 8 (6e8c005); GUI
connection, store, pages and widgets (3ce5c9a); measurable §20 GUI budgets
and runbook `docs/acceptance/m1e.md` (c21e8f8); macOS discovery (daca257);
App page artwork through the bounded cache (df34a07); gui.md §7
integration test `testing::gui` in ossteamd (145c5e2: ossteam-gui is now a
lib plus a thin bin and a dev-dependency of ossteamd; the proptest git
source is patched to crates.io). Compare LibraryFolder without
`free_bytes`, which changes under the parallel suite.

- macOS discovery (daca257, SPEC Q1, steamworks-abi.md §1.3): macOS
  steam_api finds the client via the Mach service
  `com.valvesoftware.steam.ipctool` (Version 100001 → 104; GetSteamPath 14
  → pid and path; replies of 32 and 544 bytes), not via files.
  `discovery/macos.rs` checks the name in with `bootstrap_check_in` and
  serves it on a thread. The library is copied to
  `<data>/steam-sdk/osx/steamclient.dylib`; xtask runs lipo to make it
  universal. The CI run on efc15b5 showed that launchd accepts check-in
  of an undeclared name (check-in succeeded and a look-up found it).
  Unverified: that destroying the receive right frees the name; the
  tests check it after the reply fix (see the top of this page).
- Keyboard-only row ✅ on Linux (887ef16) after many focus fixes (9db2176,
  092afb6, a0656d5, 758b0aa, bd1bff5, …). Buttons, switches and choices
  own a focus handle; controls that replace each other (Pause/Resume,
  Play/Play anyway/Stop) share an id so focus survives.
  `widgets/reveal.rs` keeps the focused control in view;
  `widgets/page_scroll.rs` handles PgUp/PgDn/Home/End.
- No-daemon/Retry row ✅ on Linux on the real install (bc81f09).
- The M1a–M1d rows were dry-run on the gui_states bench (4a986d3,
  943535e, 22fe26d, abf4bdb..e86b78f) and recorded in m1e.md. Each still
  needs its live run.
- Seen once and not reproduced in 4 tries: after erin's authenticator
  sign-in, the window took no keys at all (not even Ctrl-2) though it
  still redrew. Ctrl-2 is bound with no context on the root div, and gpui
  falls back to the root when the focused handle is gone, so the window
  most likely lacked the compositor's keyboard focus. Next time, ask the
  compositor which window has focus before restarting it. The login path
  opens no portal; only Settings' path picker does.
- Windows cross-clippy of the workspace is clean (706f5d8). The fake CM's
  QR auto-approval is Linux-only.

Left: the logged-in GUI checks (the account is logged out), the macOS
acceptance runs, and the §20 rows (reference-hw.md).

## M2: Steam DRM and VAC

SPEC §13/§14: no DRM or VAC code lands before `docs/spec/drm.md` and
`vac.md` are written and reviewed. Both are drafted; neither is reviewed.

- drm.md (13de005). The SteamStub key is in the wrapper's own header (the
  client hands over no key). Flag bits match Valve's documented wrapping
  options 0/6/32/38. Proton's lsteamclient has no wrapper code, which
  suggests the wrapper uses only public exports (unverified). Proton's
  steam.exe creates `Steam3Master_SharedMemLock` and
  `Global\Valve_SteamIPC_Class`. CEG: app info `config/cegpublickey` and
  `checkguid` (730 and 570 → client.dll; 41040 is free on demand). EMsgs
  5485/5486/5487/5574/851/896/897. Risk: verify could "repair" a
  CEG-customised file. §5 asks 4 questions (an observation build
  `steamstub-observe`, not decrypting the helper, test titles, CM
  capture).
- vac.md (d66f0d2). Modules arrive as ClientServiceModule 830, calls as
  ClientServiceCall 831 → ClientServiceCallResponse 832 (the host fills
  in OS/system info, load address and exception record). FileVerification
  service (SignatureCheck, SteamServiceCheck). ClientVACBanStatus 782's
  layout is unclear (SteamKit2 and VacBanBlock_t disagree). Without VAC
  the server sees auth response 5, VACCheckTimedOut. The official Linux
  host is the 32-bit `ubuntu12_32/steamservice.so`, loaded into the
  `steam` process (dlopen, `runfunc`, VacProcessMonitor reading /proc);
  the Windows host (SteamService.exe) is known only from community
  guides. Plan: a separate `ossteam-vac-host`, the daemon relays
  unchanged, a `vac` build feature. §7 asks 5 questions (reading
  steamservice.so's code, logging unhandled EMsgs, calls while VAC is
  off, ban risk to the test account, Linux-first scope).
- Research sources: SteamDatabase Protobufs, Steamless (facts only),
  Proton, SteamKit2, DepotDownloader. Clone them again as needed.

## M3

All M3 spec pages are drafted. None may get code before the maintainer
answers its questions and its live observations are made.

- cloud.md (ade70b9). Sources: SteamDatabase protobufs (Cloud.*
  service), Valve's Steam Cloud docs, the official client's
  `logs/cloud_log.txt` and `userdata/<id>/<app>/remotecache.vdf`, the
  community root-number table (SteamCloudFileManager
  ROOT_PATH_MAPPING.md, facts only), `ufs` keys in
  `~/.cache/ossteam/pics/app/*.vdf` (binary KV). Spacewar 480 has quota
  4096 / 10 files: the API test title. Open: compressed (file_size < raw)
  and encrypted downloads, Auto-Cloud names on Steam (path prefixes),
  Proton roots and platform change, the conflict rule, client_id. §9 asks
  4 questions (SteamCloudDocuments `~/.SteamCloud` vs DIRS-1, writing to
  the test account's cloud, CLI conflict UX, M3 order). Gate: §9 answers
  and the §8 observations.
- friends.md (a25ba36). Friends list 767 (unrequested after logon);
  Player.AddFriend/RemoveFriend/IgnoreFriend; nicknames 5587/5588;
  categories via FriendsList.GetCategories; ClientPersonaState 766
  status_flags table (the split is partly unverified);
  ClientRequestFriendData 815; rich presence 7501–7503 plus
  Community.GetAppRichPresenceLocalization; chat via the FriendMessages
  service (community clients send logon `chat_mode` 2; ossteam sends none
  today); text filtering follows Player.GetCommunityPreferences. The
  official client stores coplay locally (`config/coplay_<steamid>.vdf`).
  Proposes in-memory-only friends data and no chat log on disk. §11 asks
  5 questions. Gate: §11 and §10, which need the account logged in plus a
  second account.
- betas.md (360123c). Today's PICS has no `pwdrequired` or
  `encryptedmanifests` (0 of 201 cached apps); 32 apps carry
  `depots/privatebranches = 1` and list no private branch names. Flow:
  ClientCheckAppBetaPassword 5450/5451 (code → beta name plus hex key) →
  ClientPICSPrivateBetaRequest 8907/8908 (key bytes plus app access token
  → a text-KV depot section with a NUL). DepotDownloader sends no
  branch_password_hash. Proposes keeping the key in the keyring
  (`ossteam-beta`), never the code; SetBranch/UnlockBetas/ForgetBetas
  IPC; ISteamApps GetNumBetas/GetBetaInfo/SetActiveBeta; refusing
  `lcsrequired` branches (seen on 220070 `local`) as an M1 fix (Q5). Gate:
  §9 and §8 (needs a published beta code).
- workshop.md (91a4a54). `depots/workshopdepot` is the app's own id (6
  cached apps, including Spacewar 480). Item content is the manifest
  `hcontent_file` in that depot (normal key, request code and CDN path);
  legacy items use `file_url`. The official client keeps
  `steamapps/workshop/{appworkshop_<app>.acf,content/<app>/<item>}` and
  `logs/workshop_log.txt` (daily subscription list, GetItemInfo full
  list, DownloadItem commits while the game runs). Proposes
  `<library>/workshop/content/<app>/<item>` beside `common/`; publishing
  is left out (no upload observed). §11 asks 5 questions. Gate: §11 and
  §10 (subscribing shows on the profile: ask first).
- offline.md (1e770e4). Gap today: licenses are held in memory only, so a
  daemon started offline has an empty library. The official client uses
  loginusers.vdf WantsOfflineMode/SkipOfflineModeWarning and a
  localconfig.vdf `Offline` block (Ticket 11 B, Signature 256 B,
  EncryptedTicket 317 B, EncryptedSignature 256 B) from
  `Offline.GetOfflineLogonTicket`, which ossteam proposes not to use.
  steamui_login.txt shows a failed logon leading to "Start offline";
  `Player.RecordDisconnectedPlaytime` records offline playtime; appcache
  and stats are kept. Proposes a stored license list (changes pics.md
  §6), an offline stats merge (changes isteamuserstats.md) and a
  SetOfflineMode IPC. Gate: §10 and §9.
