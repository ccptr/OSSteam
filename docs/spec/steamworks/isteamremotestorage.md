# ISteamRemoteStorage

| | |
|---|---|
| Status | M1c, first part: cloud state. Local files follow. |
| Requirements | SPEC §12.3, FAITH-2 |
| Versions | `STEAMREMOTESTORAGE_INTERFACE_VERSION001` to `016` (`vtables.txt`) |
| Code | `crates/ossteam-steamclient/src/remote_storage.rs` |

| Method | ossteam |
|---|---|
| `IsCloudEnabledForAccount` | False: ossteam has no Steam Cloud until M3. |
| `IsCloudEnabledForApp` | False, for the same reason. |

Everything else fails under ABI-3 for now. The file methods (`FileWrite`,
`FileRead`, `FileExists`, `GetFileSize`, `GetFileCount`, ...) will read
and write locally under the cloud root that the app's info defines, and
will say that nothing is synced, as SPEC §12.3 requires. Until then they
fail, so a game that keeps its saves only in remote storage cannot save.
