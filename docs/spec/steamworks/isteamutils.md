# ISteamUtils

| | |
|---|---|
| Status | Implemented for M1c, except text filtering and the gamepad text input |
| Requirements | SPEC §12.3, GIPC-3, FAITH-2 |
| Versions | `SteamUtils002` to `SteamUtils011` (`vtables.txt`) |
| Code | `crates/ossteam-steamclient/src/utils.rs` |

`ISteamUtils` belongs to a pipe, not a user (`steamworks-abi.md` §2.3).

| Method | ossteam |
|---|---|
| `GetAppID` | The app id the process claims, or 0. |
| `GetConnectedUniverse` | 1 (`k_EUniversePublic`) while an account is active, otherwise 0 (`k_EUniverseInvalid`). |
| `GetServerRealTime` | Steam's clock: `Session.server_time` plus the time since the shim received it. 0 before the first logon. |
| `GetIPCountry` | The country Steam gave at logon, or an empty string. |
| `GetSteamUILanguage` | The daemon's `language` setting. |
| `GetCurrentBatteryPower` | The battery's charge in percent while the computer runs on its battery, otherwise 255 (`k_unBatteryPowerAC`). Linux reads `/sys/class/power_supply`: 255 when a `Mains` or `USB` supply is online, otherwise the lowest `capacity` of the system batteries whose `status` is `Discharging`, otherwise 255. Windows uses `GetSystemPowerStatus`: the percentage when `ACLineStatus` is 0 and the percentage is known. macOS has no implementation until M1e: it answers 255 and logs once that the value is not measured. |
| `IsAPICallCompleted(call, failed)` | True once the call's result is stored or the call failed (§2.5). `*failed` says which. False for a call that is pending or unknown. |
| `GetAPICallFailureReason(call)` | -1 (`None`) for a call that is pending or completed, 0 (`SteamGone`) for one that failed when the connection closed, 2 (`InvalidHandle`) for an unknown handle. |
| `GetAPICallResult(...)` | As `Steam_GetAPICallResult` (§2.5). |
| `RunFrame` | Nothing to do. |
| `GetIPCCallCount` | As `ISteamClient::GetIPCCallCount`. |
| `SetWarningMessageHook` | Stored, never called. |
| `SetOverlayNotificationPosition`, `SetOverlayNotificationInset` | Accepted and ignored: ossteam shows no notifications, since it has no overlay. |
| `IsOverlayEnabled`, `BOverlayNeedsPresent` | False: there is no overlay. |
| `IsSteamRunningInVR`, `IsSteamInBigPictureMode`, `IsSteamChinaLauncher` | False. |
| `IsSteamRunningOnSteamDeck` | False: ossteam is not the Steam Deck's session (`launch-env.md` §2). |
| `GetImageSize`, `GetImageRGBA` | The achievement icons' images (`isteamuserstats.md` §4). |

Everything else fails under ABI-3: `GetSecondsSinceAppActive` and
`GetSecondsSinceComputerActive` (no idle tracking), `GetCSERIPPort`, `CheckFileSignature`, the gamepad text input methods,
the VR methods that change something, `InitFilterText`, `FilterText`,
`GetIPv6ConnectivityState` and `SetGameLauncherMode`.
