# ISteamFriends

| | |
|---|---|
| Status | M1 scope implemented: the account's own persona name and state |
| Requirements | SPEC §12.3; the rest is M3 |
| Versions | `SteamFriends001` to `SteamFriends018` (`vtables.txt`) |
| Code | `crates/ossteam-steamclient/src/friends.rs` |

| Method | ossteam |
|---|---|
| `GetPersonaName` | The active account's persona name, or an empty string. |
| `GetPersonaState` | The account's `EPersonaState` while it is logged on (1 online, 7 invisible), otherwise 0 (offline). |

Everything else fails under ABI-3 until M3. The friends list is empty
because the methods that enumerate it return 0, and the overlay methods
(`ActivateGameOverlay*`) do nothing, which matches `IsOverlayEnabled`
being false.
