# ISteamClient

| | |
|---|---|
| Status | Implemented for M1c, except game servers |
| Requirements | SPEC §12.3, ABI-1 to ABI-3 |
| Versions | `SteamClient006` to `SteamClient021` and `SteamClient023` (`vtables.txt`) |
| Code | `crates/ossteam-steamclient/src/client.rs` |

`CreateInterface` returns this interface (`steamworks-abi.md` §2.1). It
creates pipes and users and hands out every other interface. Pipes,
users and getters are described in `steamworks-abi.md` §2.3.

| Method | ossteam |
|---|---|
| `CreateSteamPipe` | Opens a connection to the daemon (§2.3). |
| `BReleaseSteamPipe(pipe)` | Closes it (§2.3). |
| `ConnectToGlobalUser(pipe)` | User 1, or 0 (§2.3). |
| `CreateLocalUser`, `CreateGlobalUser` | 0, logged: game servers and anonymous users are not in M1. |
| `ReleaseUser(pipe, user)` | Forgets the user. |
| `GetISteam*`, `GetISteamGenericInterface` | §2.3. `GetISteamGameServer`, `GetISteamGameServerStats` and `GetISteamMasterServerUpdater` hand out their objects too; every method of those fails under ABI-3. |
| `SetLocalIPBinding` | Accepted. It has no effect, because the shim opens no network connections of its own. |
| `RunFrame` | Nothing to do: the shim needs no frame calls. |
| `GetIPCCallCount` | The number of requests the process sent to the daemon since the previous call. |
| `SetWarningMessageHook(hook)` | Stored. The shim reports nothing through it (its log is on standard error, §2.6). |
| `BShutdownIfAllPipesClosed` | True when no pipe is open. |
| `Set_SteamAPI_CCheckCallbackRegisteredInProcess(func)` | Stored and never called: the shim queues every callback, whether or not one is registered. *Unverified:* the official client uses it to skip callbacks nobody listens to. |
| `DEPRECATED_Set_SteamAPI_CPostAPIResultInProcess`, `DEPRECATED_Remove_SteamAPI_CPostAPIResultInProcess`, `Set_SteamAPI_CPostAPIResultInProcess`, `Remove_SteamAPI_CPostAPIResultInProcess` | Accepted and ignored: results reach the game through callback 703 (§2.5). |
| `DestroyAllInterfaces` | Nothing: objects stay valid (§2.3), so there is nothing to destroy. |
| `GetIVAC` | NULL, logged: VAC is M2. |

Every other method of an older version (`GetUniverseName`, `Init`,
`Shutdown`, `SetOverrideMode`, the controller methods of
`SteamClient012`) fails under ABI-3.
