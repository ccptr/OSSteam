# ISteamRemoteStorage

| | |
|---|---|
| Status | Implemented for M1c: cloud state and local files. Nothing is synced until M3. |
| Requirements | SPEC §12.3, FAITH-1, FAITH-2 |
| Versions | `STEAMREMOTESTORAGE_INTERFACE_VERSION001` to `016` (`vtables.txt`) |
| Code | `crates/ossteam-steamclient/src/remote_storage.rs` |

Games keep files in Steam Cloud through this interface. The official
client writes them to disk first, under
`<Steam>/userdata/<account id>/<app id>/remote/`, and syncs them with
Steam in the background. ossteam writes and reads the same files under
`App.user_dir` (`steamworks-abi.md` §3.4), in `<user_dir>/remote/`, and
syncs nothing: Steam Cloud is M3 (SPEC §12.3). A game therefore sees its
files from earlier sessions on this computer, as it would with Steam
Cloud turned off.

**Verification.** The directory comes from the official client's files.
The limits come from the SDK headers (`k_unMaxCloudFileChunkSize`,
`k_cchFilenameMax`) and the app's info (`ufs`, `pics.md`). How the
official client treats a name with `/` or `\`, and whether it compares
names without regard to case, is *unverified*. When a method fails and
what it returns then follow the Steamworks documentation of each method.

## 1. Files

**Names.** A name is at most 259 bytes of UTF-8 (`k_cchFilenameMax` is
260 with the NUL). `/` separates directories under `remote/`, and `\` is
taken as `/`. A name that is empty, starts with `/`, has an empty, `.`
or `..` component, holds a `:`, or ends in `.ossteam-part` (§2) is
refused: the method fails. A name that is not UTF-8 is refused too.
Names are compared exactly; *unverified:* the official client may
compare them without regard to case.

**The quota.** The app's info gives `ufs/quota` (bytes) and
`ufs/maxnumfiles`, which the daemon passes as `App.cloud_quota` and
`App.cloud_max_files`. A write fails when the files would then take more
than the quota, or when it creates a file beyond `maxnumfiles` (0: no
limit on the count). An app whose info gives no quota has no Steam
Cloud, so every write fails, as under the official client. Reads,
deletes and the listing work whatever the quota.

**Writing.** A write goes to a temporary file in the same directory,
which then replaces the file, so a reader sees the old file or the new
one. One write is at most 100 MiB (`k_unMaxCloudFileChunkSize`).

There is no account while `user_dir` is empty; every method then fails.

## 2. Methods

| Method | ossteam |
|---|---|
| `FileWrite(name, data, size)` | Writes the file (§1). False when the name is refused, `size` is negative or above 100 MiB, the quota would be exceeded, or writing fails. |
| `FileRead(name, buffer, size)` | Reads at most `size` bytes from the start into `buffer`. Returns how many were read, 0 when the file does not exist. |
| `FileWriteAsync(name, data, size)` | As `FileWrite`, as an API call whose result is `RemoteStorageFileWriteAsyncComplete_t` (1331) `{EResult m_eResult}`, 4 bytes. `k_uAPICallInvalid` (0) where `FileWrite` refuses; otherwise the write happens before the method returns, and the result, `k_EResultOK` or `k_EResultFail` when writing failed, is queued at once. |
| `FileReadAsync(name, offset, size)` | Reads `size` bytes from `offset` and keeps them. An API call whose result is `RemoteStorageFileReadAsyncComplete_t` (1332) `{SteamAPICall_t m_hFileReadAsync; EResult m_eResult; uint32 m_nOffset; uint32 m_cubRead}`, 20 bytes on Linux and macOS, 24 on Windows: `k_EResultOK`, or `k_EResultFail` with nothing read when reading failed. `k_uAPICallInvalid` when the name is refused, there is no file, `size` is 0, or `offset + size` is past the file's end. |
| `FileReadAsyncComplete(call, buffer, size)` | Copies what `FileReadAsync` read, at most `size` bytes, and forgets it. False for an unknown call or a null buffer. |
| `FileExists(name)` | Whether the file exists. |
| `GetFileSize(name)` | Its size, or 0. |
| `GetFileTimestamp(name)` | Its modification time in Unix seconds, or 0. |
| `FilePersisted(name)` | False: no file is in Steam Cloud. |
| `FileForget(name)` | True when the file exists: there is nothing in Steam Cloud to remove, and the local file stays, as the SDK documents. |
| `FileDelete(name)` | Deletes the file. True when it existed. |
| `FileWriteStreamOpen(name)` | Starts writing the file in chunks: a handle (counting up from 1), or `k_UGCFileStreamHandleInvalid` (all bits set) when the name is refused. |
| `FileWriteStreamWriteChunk(handle, data, size)` | Appends to the stream's temporary file. False for an unknown handle, a negative `size`, or a stream that would exceed 100 MiB or the quota. |
| `FileWriteStreamClose(handle)` | Replaces the file with what was written, checking the quota as `FileWrite` does. |
| `FileWriteStreamCancel(handle)` | Drops the stream and its temporary file. |
| `GetFileCount` | The number of files under `remote/`. |
| `GetFileNameAndSize(i, size *)` | File `i` of the listing, sorted by name, as its name, with its size in `*size`. An empty string past the end. The listing is read again when `GetFileCount` is called. |
| `GetQuota(total *, available *)` | The quota and what is left of it, 0 and 0 for an app without one. `int32 *` up to version `013`, `uint64 *` from `014`, as the SDK headers declare; an `int32` saturates. True. |
| `IsCloudEnabledForAccount`, `IsCloudEnabledForApp` | False: there is no Steam Cloud until M3. |
| `GetLocalFileChangeCount`, `GetLocalFileChange` | 0, and an empty string with both enums `Invalid` (0): nothing is synced, so no file changes on its own. |
| `BeginFileWriteBatch`, `EndFileWriteBatch` | Only kept as a flag, since there is no upload to group: `Begin` is false while a batch is open, `End` false while none is. |

A temporary file is named `.<file name>.<process id>-<n>.ossteam-part`,
next to the file. It is not listed, and a name ending in
`.ossteam-part` is refused, so a game cannot see or touch one.

Everything else fails under ABI-3: `FileShare`, `SetSyncPlatforms` and
`GetSyncPlatforms` (nothing is synced), `SetCloudEnabledForApp`, and
the UGC and Workshop methods (M3).
