# ISteamAppDisableUpdate

| | |
|---|---|
| Status | Implemented: a game holds back its app's automatic updates |
| Requirements | SPEC §12.3, GIPC-1, FAITH-1 to FAITH-3 |
| Versions | `SteamAppDisableUpdate001`, reached through `ISteamClient::GetISteamGenericInterface` (`vtables-observed.txt`) |
| Code | `crates/ossteam-steamclient/src/apps.rs` (`set_update_hold`); the daemon's side in `crates/ossteamd/src/core/update_hold.rs` |

A game that must not be updated under its own feet, as during a match,
tells Steam so for a number of seconds. No published SDK declares this
interface; Proton does not carry it either.

**Verification.** The layout comes from the official client's
`steamclient.so` (structure only, `research-sources.md`): its adapter
class for `SteamAppDisableUpdate001` has one virtual method, which
passes the running game's app id and its one `int32` argument to an
internal `SetAppUpdateDisabledSecondsRemaining`; that internal
interface also has `GetAppUpdateDisabledSecondsRemaining`, for the
client itself. Counter-Strike 2 (`libclient.so`) asks for the interface
by its version string, calls the slot with 900 while it logs "Telling
Steam not to update the app for the next %d seconds", and with 0 while
it logs "Telling Steam it is safe to update the app". It ignores the
return value. What the official client does with the time is
*unverified* beyond the method's name.

## 1. The method

| Slot | Method | What it does |
|---|---|---|
| 0 | `void SetAppUpdateDisabledSecondsRemaining(int32 nSeconds)` | asks the daemon to hold the connection's app's automatic updates for `nSeconds` from now; 0 or less ends the hold |

The shim sends the request without waiting for its answer (§2). With no
pipe, or a pipe the daemon has closed, the call does nothing.

## 2. Game IPC

| Tag | Request | Response |
|---|---|---|
| 13 | `UpdateHold{seconds: i32}` | `Done{}` |

The request acts on the connection's own app id, which the account must
own, as the stats requests do (`steamworks-abi.md` §3.6); otherwise the
daemon answers `Error`.

## 3. The daemon

The daemon keeps one deadline per app, in memory: now plus the seconds,
or none for 0 or less. A later request replaces it, whichever
connection sends it. The deadline does not outlive the daemon, and does
not end when the game does: a game that dies holding a hold leaves it to
run out, as its own clock would.

While an app's deadline lies ahead:

- update detection (`content.md` §10) still marks the app, but queues
  no automatic update job for it;
- a job the user asks for, as `ossteam update`, runs at once
  (*unverified*: whether the official client holds those too);
- a job already queued or running is left alone.

When the deadline passes, or a request ends the hold, the daemon queues
the update that detection would have queued, if the app still has one,
`content.auto_update` is `always`, and no job works on the app. The
daemon logs each change at info level.

## 4. Checks

| Item | Status |
|---|---|
| Slot layout and argument | from the official library's adapter and Counter-Strike 2's calls |
| Counter-Strike 2 calls it under ossteam | not seen: it calls it only under a condition its code checks, apparently in a match, and the live runs stay in the main menu. Not called at the menu on 2026-11-04. |
| What the official client holds back (automatic updates only, or user-started ones too) | unverified |
