# ISteamUser

| | |
|---|---|
| Status | M1c, first part: logon state, Steam id and the data folder. Auth session tickets and encrypted app tickets follow. |
| Requirements | SPEC §12.3, FAITH-1 to FAITH-3, NET-4 |
| Versions | `SteamUser004` to `SteamUser023` (`vtables.txt`) |
| Code | `crates/ossteam-steamclient/src/user.rs` |

| Method | ossteam |
|---|---|
| `GetHSteamUser` | The object's user handle. |
| `BLoggedOn` | True when the pipe is open, an account is active, and it is logged on to Steam right now (`Session.logged_on`). False while the daemon reconnects (NET-4). |
| `GetSteamID` | The active account's Steam id, also while it is not logged on. `k_steamIDNil` when no account is active. |
| `GetUserDataFolder(buffer, size)` | `<user_dir>/local` (`steamworks-abi.md` §3.4), created when missing. False, with an empty buffer, when no account is active, the folder cannot be created, or the path does not fit with its NUL. *Unverified:* the official client's `<Steam>/userdata/<account id>/<app id>/local` is the model; whether it creates the folder is not known. |

Everything else fails under ABI-3 for now. In particular:

- `GetAuthSessionTicket`, `GetAuthTicketForWebApi`, `BeginAuthSession`,
  `EndAuthSession`, `CancelAuthTicket`, `UserHasLicenseForApp`,
  `RequestEncryptedAppTicket` and `GetEncryptedAppTicket` return their
  failure values until tickets are built (SPEC §12.3). Games that need
  a ticket to join a server fail to join, honestly.
- Voice (`StartVoiceRecording`, `GetVoice`, ...) is not provided.
- `GetPlayerSteamLevel`, `GetGameBadgeLevel`, `BIsBehindNAT`, the phone
  and two-factor queries, `GetMarketEligibility`, `GetDurationControl`
  and `RequestStoreAuthURL` need data the daemon does not fetch yet.
