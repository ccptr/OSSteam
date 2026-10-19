# Content: depots, manifests, chunks and the install engine

| | |
|---|---|
| Status | Draft for M1b; nothing yet tried with an account |
| Requirements | SPEC §10 (CONT-1 to CONT-7, CONT-9), §17, D-6, D-10, SEC-3, SEC-5, §19.3, §20 |
| Code | `crates/ossteam-content`; the daemon side in `crates/ossteamd/src/content/` |

An app's files are published in **depots**. A depot has one **manifest**
per build. The manifest lists the depot's files, and each file is split
into **chunks** of at most 1 MiB. Chunks are stored on content servers
(the **CDN**), encrypted with the depot's key and compressed. A chunk is
named by a hash of its content, so a chunk shared by two files, or two
builds, is one object on the CDN. This page describes how the client
finds servers, obtains keys and manifests, downloads and checks chunks,
and how ossteam lays them out on disk. The message schemas are in the
vendored `steammessages_contentsystem.steamclient.proto`,
`steammessages_clientserver_2.proto` and `content_manifest.proto` (SPEC
LIC-5), so fields are named here but not restated.

**Verification.** As in `cm.md` and `pics.md`, this draft comes from
public community documentation and community client implementations,
read for behaviour and restated in our own words (SPEC §2.6). Every
statement must be confirmed against the official client on our test
account before it is relied on; §14 tracks which have been. Rows marked
*unverified* must not be treated as fact.

## 1. Content servers (CONT-1)

The unified method `ContentServerDirectory.GetServersForSteamPipe` (sent
over CM, `cm.md` §3.5) returns the servers to use. ossteam sets
`cell_id` to the cell the CM gave at logon (`cm.md` §4) and leaves
`max_servers` at its default (20). Each returned server has, among
others:

| Field | Use |
|---|---|
| `type` | `CDN` (Valve's or a partner's network), `SteamCache` (an ISP or LAN cache Valve lists), `OpenCache`, and possibly others |
| `host`, `vhost` | the address; requests go to `vhost` |
| `https_support` | `mandatory`, `optional` or `none` (*values unverified*) |
| `allowed_app_ids` | when non-empty, the server serves only these apps |
| `weighted_load` | lower is better |
| `num_entries_in_client_list` | how many times the server should appear in the client's rotation |
| `use_as_proxy`, `proxy_request_path_template` | a server that forwards to others |
| `steam_china_only` | for the Steam China realm |

**Selection.** ossteam keeps the servers whose `type` is `CDN` or
`SteamCache`, that support HTTPS (`mandatory` or `optional`), that are
not `steam_china_only`, and whose `allowed_app_ids` is empty or contains
the app being installed. It orders them by `weighted_load` and lists each
`num_entries_in_client_list` times (at least once). Requests rotate
through the list. A server that fails a request is moved to the end of
the rotation and penalised for 60 s; while every server is penalised the
least recently failed one is used. Proxy servers and `OpenCache` are not
used in M1.

**HTTPS only.** The official client fetches from many servers over plain
HTTP, which is safe for integrity because every chunk is checked (§5).
ossteam uses HTTPS for every request (SEC-3) and so skips servers that
do not offer it; ADR 0003 records this deviation. If the selection is
empty the job fails with "no content server offers HTTPS for this app"
(exit code 9) rather than falling back to HTTP.

The list is fetched when the first job of a session needs it and again
after an hour, after every server has failed, or when a new session
starts. It is not written to disk.

*Unverified:* the official client's selection, ordering and penalty
rules; how it uses `priority_class`.

## 2. Depot keys (CONT-1)

`ClientGetDepotDecryptionKey` (EMsg 5438) is a job with `depot_id` and
`app_id` (the app being installed). The reply,
`ClientGetDepotDecryptionKeyResponse` (EMsg 5439), carries `eresult`,
`depot_id` and the 32-byte `depot_encryption_key`. The key is AES-256;
it decrypts chunks (§5) and encrypted file names (§4).

The server grants a key only for a depot the account is entitled to, so
this request is the entitlement check (CONT-1). Any `eresult` other than
OK, or a key that is not 32 bytes, means the depot is not available to
the account: it is dropped from the install (CONT-2, §6). If that leaves
no depot, the job fails as not entitled (exit code 6) and names the
depots and results.

Keys are held in memory for the session, in a type that zeroes them on
drop and never prints them (D-10). They are not stored: the official
client keeps them in its config, but they can be asked for again at any
time, and keeping them only in memory means a stolen state directory
holds none.

*Unverified:* which `eresult` values the server uses for unowned,
unreleased and region-locked depots (expected: `AccessDenied` (15) or
`Blocked`).

## 3. Manifest request codes

A manifest is fetched with a request code from the unified method
`ContentServerDirectory.GetManifestRequestCode`, which takes `app_id`,
`depot_id`, `manifest_id` and `app_branch`. For the public branch,
`app_branch` is left out. `branch_password_hash` is for
password-protected branches, which are M3 (CONT-9). The reply's
`manifest_request_code` is a `u64`.

Codes expire after a few minutes (*unverified*), so ossteam asks for one
right before each manifest download and never stores it. A code of 0
means the server refused; the manifest is then not available to the
account and the job fails as not entitled.

## 4. Manifests

**Download.** `GET https://<vhost>/depot/<depot>/manifest/<manifest>/5/<code>`
with the manifest id in decimal. `5` is the manifest format version.
The body is a zip archive (§5.3) with exactly one entry, the manifest.

**Format.** The manifest is a sequence of sections. Each starts with a
little-endian `u32` magic number. All but the last continue with a
little-endian `u32` length and that many bytes of a protobuf message:

| Magic | Section | Message |
|---|---|---|
| `0x71F617D0` | payload | `ContentManifestPayload` |
| `0x1F4812BE` | metadata | `ContentManifestMetadata` |
| `0x1B81B817` | signature | `ContentManifestSignature` |
| `0x32C415AB` | end of manifest | none; nothing follows |

All three message sections must be present. Any other magic number is
an error. An older binary manifest format (version 4) starts with a
different magic number; ossteam does not read it and reports the
manifest as unsupported.

**Metadata.** `depot_id` and `gid_manifest` must match what was asked
for. `filenames_encrypted` says whether names are encrypted.
`cb_disk_original` is the total size of the files, and
`cb_disk_compressed` the total size of the unique chunks as stored. The
CRCs and the signature are not checked: *how they are computed is
unverified*, and every chunk is checked on its own (§5).

**Files.** Each `FileMapping` in the payload has:

| Field | Meaning |
|---|---|
| `filename` | path relative to the install directory, with `\` separators |
| `size` | file size in bytes |
| `flags` | bit set, below |
| `sha_filename` | SHA-1 of the lower-cased name with `\` separators (*unverified*) |
| `sha_content` | SHA-1 of the whole file; all zero for an empty file and for directories (*unverified*) |
| `chunks` | the file's chunks: `sha` (20-byte chunk id), `crc`, `offset`, `cb_original`, `cb_compressed` |
| `linktarget` | for a symbolic link, its target |

| Flag | Meaning |
|---|---|
| `0x001` | user configuration file: install once, never overwrite |
| `0x002` | versioned user configuration file: overwrite only when the manifest's version of it changes |
| `0x004` | encrypted (old content; not seen in current manifests) |
| `0x008` | read-only |
| `0x010` | hidden |
| `0x020` | executable |
| `0x040` | directory |
| `0x080` | custom executable |
| `0x100` | install script (`install-scripts.md`) |
| `0x200` | symbolic link |

*The meaning of each flag bit is unverified;* the executable, directory
and symbolic link bits are the ones the engine acts on.

**Encrypted names.** When `filenames_encrypted` is set, `filename` and
`linktarget` are base64 text. Decoded, the first 16 bytes are the IV,
encrypted on its own with AES-256-ECB under the depot key; the rest is
AES-256-CBC under the depot key with that IV and PKCS#7 padding. The
plaintext is the UTF-8 name, possibly followed by NUL bytes, which are
removed. ossteam decrypts all names before using a manifest.

**Paths.** A name is split on `\` and `/` into components. A name is
refused, and the job fails, if it is empty, absolute, has a drive or UNC
prefix, or has a component that is empty, `.`, `..`, or contains NUL. On
Windows, components that Windows cannot hold (`< > : " | ? *`, control
characters, a trailing space or dot, reserved device names such as `CON`)
are also refused. Names are used as given, including their case; a depot
built on a case-insensitive system can name `Data/a` and `data/b`, which
are two directories on Linux. *What the official client does on Linux
with such depots is unverified.*

**Chunk layout.** A file's chunks must lie within `[0, size)` and must
not overlap; a manifest that breaks this is refused. Bytes not covered by
any chunk are zero. *Whether manifests ever leave gaps is unverified.*

**Stored form.** After a successful install, ossteam stores each
manifest it installed in `<library>/.ossteam/manifests/<depot>_<manifest>.manifest`,
in the same section format but with names decrypted and
`filenames_encrypted` cleared, so that updates and verify need neither
the network nor the key to read it. The official client keeps manifests
in a similar per-library depot cache.

## 5. Chunks (CONT-3)

**Download.** `GET https://<vhost>/depot/<depot>/chunk/<id>`, where `<id>`
is the chunk's 20-byte `sha` in lower-case hex. The response length must
equal the chunk's `cb_compressed`.

**Decryption.** As for names (§4): the first 16 bytes are the AES-256-ECB
encrypted IV, the rest AES-256-CBC with PKCS#7 padding, both under the
depot key. The plaintext is one of three compressed forms, told apart by
its first bytes.

### 5.1 VZip (LZMA)

| Offset | Size | Content |
|---|---|---|
| 0 | 2 | `VZ` |
| 2 | 1 | version, `a` |
| 3 | 4 | a CRC-32 or a timestamp; ignored |
| 7 | 5 | LZMA properties: one byte of lc/lp/pb, then the dictionary size (`u32` LE) |
| 12 | n | raw LZMA stream, without the 13-byte `.lzma` header |
| end − 10 | 4 | CRC-32 of the decompressed data (LE) |
| end − 6 | 4 | decompressed size (LE) |
| end − 2 | 2 | `zv` |

### 5.2 VZstd

| Offset | Size | Content |
|---|---|---|
| 0 | 4 | `VSZa` |
| 4 | 4 | CRC-32 of the decompressed data (LE) |
| 8 | n | one Zstandard frame |
| end − 15 | 4 | CRC-32 of the decompressed data again (LE) |
| end − 11 | 4 | decompressed size (LE) |
| end − 7 | 4 | unknown; ignored |
| end − 3 | 3 | `zsv` |

### 5.3 Zip

A zip archive (`PK\x03\x04`) with exactly one entry, stored (method 0)
or deflated (method 8). ossteam reads it through the central directory
at the end of the archive, so data descriptors and local headers with
zero sizes are handled. The entry's CRC-32 must match its decompressed
data. Manifests (§4) use this form too.

**Checks.** After decompression, a chunk is accepted only if:

1. its length is `cb_original`;
2. its Adler-32 checksum, computed with both sums starting at 0 (not the
   usual 1), equals `crc`;
3. its SHA-1 equals the chunk id. *Unverified: that the id is the SHA-1
   of the decompressed data. Community clients check only (2). If the
   acceptance run shows otherwise, this check is the one to change.*

The CRC-32 values inside VZip and VZstd frames are not checked:
community clients skip them for chunks, *for reasons not known*, and the
three checks above cover the same data. A chunk that fails any check is discarded and fetched again from another server
(§7); it is never written to a file.

## 6. Choosing depots (CONT-2)

The app's info (`pics.md` §3) lists its depots under `depots/<id>`. For an
install on branch `b`, a depot is part of the install when all of these
hold:

1. **Platform.** `config/oslist` is empty or contains the host OS:
   `windows`, `macos` or `linux`. On Linux, an app whose compatibility
   tool runs Windows programs installs its Windows depots, and the
   platform is recorded with the install (`compat-tools.md` §4).
2. **Architecture.** `config/osarch` is empty or equals the host's
   pointer width (`64` on every PLAT-1 target). *Unverified: whether the
   official client also installs `32` depots on a 64-bit host when no
   `64` depot replaces them.*
3. **Language.** `config/language` is empty or equals the
   `content.language` setting (default `english`).
4. **Low violence.** A depot with `config/lowviolence` set is chosen only
   when the license that grants the app carries the low-violence flag
   (`pics.md` §1). *Unverified.*
5. **DLC.** A depot with `dlcappid` is chosen only when the library holds
   that app (`pics.md` §7).
6. **Content on the branch.** `manifests/<b>` names a manifest. It is
   either a string with the manifest id or an object whose `gid` holds it.
   A depot without a manifest for `b` has no files on that branch and is
   skipped. A depot whose entry is only under `encryptedmanifests` belongs
   to a password-protected branch (M3) and makes the install fail as
   unsupported (exit code 7).
7. **Shared depots.** A depot with `depotfromapp` and no `manifests` takes
   its manifests from that app's `depots/<id>`, looked up the same way
   (one level only: if that entry is shared from yet another app, the
   install fails). With `sharedinstall` set, its
   files go to the other app's install directory, not this app's (§8).
8. **Entitlement.** The depot key request succeeds (§2). Depots whose key
   is refused are dropped, and the job reports them.

Keys under `depots` that are not numbers (`branches`, `baselanguages`,
`workshopdepot`, ...) are not depots. `depots/<id>/maxsize`, when given,
is the depot's size and is used only for the estimate before manifests
are fetched.

A branch other than `public` must exist under `depots/branches` and must
not have `pwdrequired` set.

*Unverified:* the whole rule set above against the official client's
choice for the acceptance titles (`m1b.md`).

## 7. Fetching and retries

A job downloads up to `content.download_threads` chunks at a time
(default 8, 1 to 64; CONC-1), all jobs together limited to
`content.bandwidth_limit` KiB/s (0 means no limit). Each request has a
10 s connect timeout and a 60 s timeout for the body.

A request that fails (a connection error, a timeout, an HTTP status other
than 200, a wrong length, a chunk that fails its checks) is retried on
the next server in the rotation, up to 5 attempts per chunk; then the job
fails with a network error (exit code 9) and can be resumed.

**403.** A server can require a CDN auth token. On a 403, ossteam asks
`ContentServerDirectory.GetCDNAuthToken` with `depot_id`, `host_name`
(the server's `host`) and `app_id`, and repeats the request with the
returned `token` as the URL's query string, until `expiration_time`.
*Unverified:* when servers ask for tokens and the exact query form.

## 8. On disk (CONT-6)

A library folder (SPEC §17) holds:

```
<library>/
  common/<installdir>/            an app's files
  .ossteam/library.toml           marks the folder; holds its id
  .ossteam/manifests/<depot>_<manifest>.manifest
  .ossteam/staging/<appid>/       files being written
```

`<installdir>` is the app's `config/installdir`. An app without one cannot
be installed. Its value must be a single path component, checked as in
§4. Depots with `sharedinstall` go to `common/<installdir of the other
app>`; that app is recorded as installed with the depot, and it is
removed only when no installed app uses it any more.

**Disk space** is checked before a job writes anything: the free space on
the folder's file system must exceed the sum of the sizes of the files
the job will write (new and changed files in full, less the chunks of
them already in staging), plus 1 % of that.

**Staging.** A file the job writes is assembled in
`.ossteam/staging/<appid>/<depot>/<hex SHA-1 of its path>`, then renamed
into place once all its chunks are written. Staging is on the same file
system as the install, so the rename is atomic: a reader sees the old
file or the new one, never a mix. The executable flag sets the owner,
group and other execute bits on Unix; read-only sets the file read-only.
Directories are created as the manifest lists them and as files need
them. Symbolic links are created on Unix; on Windows, creating one needs a
privilege the user may not have, and the job then fails and says so.
Links are made after every file is in place.

**No writes through links.** A depot can contain a symbolic link to a
place outside its install directory. ossteam never writes, deletes or
reads chunks through a link inside the install directory: every
component of a path below the install directory that already exists must
be a real directory, or the job fails ("a symbolic link or a file is
where the depot has a directory"). *What the official client does here
is unverified.*

**User configuration files** (flag `0x001`) are written only if the file
does not exist yet.

## 9. Jobs and the journal (CONT-4)

A job is an install, an update, a verify or an uninstall of one app. Jobs
are persisted in the state database (D-6) and run one at a time by
default (`content.concurrent_jobs`). A job is `queued`, `running`,
`paused`, `done`, `failed` or `cancelled`. Jobs that were `running` when
the daemon stopped are `queued` again at the next start and resume.

The **journal** records, per job, the manifests it is installing, the
chunks already written to staging and the bytes downloaded so far, so
that a resumed job's progress counts what its earlier runs downloaded. It is written in batches, at most once
per second, so it may lag the files, and once more when the download step
ends, whether or not it succeeded. A journal made for other manifests
than the job's is discarded with the staging. On resume, each journaled chunk is
read back from staging and checked (§5 checks 1 and 2) before it is
trusted; a chunk that fails is fetched again. A staged file the journal
does not know is deleted.

## 10. Updates (CONT-4)

An update installs the branch's current manifests over the installed
ones, depot by depot:

- A file whose size, `sha_content` and flags are unchanged is left alone.
- A file whose only change is its flags gets the new flags.
- Otherwise the file is rebuilt in staging. Each of its chunks is copied
  from the installed files when any installed file holds a chunk with the
  same id (read at the old manifest's offset and checked as in §5 before
  it is used, since the file may have changed under us), and downloaded
  otherwise. The new manifest's offsets are tried too, since a resumed
  update may have put some new files in place already. Every local copy
  is made before any file is renamed into place, so the copies read the
  old files.
- Files and directories that the new manifest no longer lists are
  deleted after every new file is in place, except user configuration
  files (either flag). A directory is deleted only once it is empty, so
  files the user put there keep it.
- A depot that is no longer chosen (§6) has its files deleted the same
  way; a newly chosen depot is installed in full.

The installed state is updated in the database only when the whole job
is done. A crash in between leaves a mix of old and new files, which the
resumed job completes.

**Update detection.** When product info or the library changes
(`pics.md` §4), ossteam chooses the depots of each installed app again
(§6) and compares them with the installed ones. If an installed depot now
has another manifest on the installed branch, or is no longer chosen, the
app has an update. A depot that is chosen but not installed does not mark
it: that is also what a depot whose key Steam refused (§2) looks like, and
it would mark the app for ever. An update job installs such a depot all
the same, since it chooses depots afresh. When no depot is chosen at all,
as for a platform the app has no depots for, the app is not marked:
there is nothing an update could install. On Linux, an installed app
whose platform is no longer its tool's (`compat-tools.md` §4) has an
update even when the depots are the same. Changing a `compat.*` setting
runs the detection again. With `content.auto_update` set
to `always` (the default) an update job is queued; with `never` the app
is only marked. *Unverified: the official client's per-app policies
(always, on launch, high priority) are not reproduced in M1.*

## 11. Verify (CONT-5)

Verify reads every file of the installed manifests. A file whose size and
SHA-1 match `sha_content` is good. Otherwise each chunk is read and
checked (§5 checks 1 and 2), the file is rebuilt in staging from its good
chunks plus downloads of the bad ones, and renamed into place. Missing
files and directories are recreated, and links whose target differs are
made again. User configuration files (either flag) are checked only for
existence. Files the manifests do not list are left
alone. The job reports how many files it repaired.

## 12. Uninstall

Uninstall deletes `common/<installdir>`, the app's stored manifests and
staging, and its rows in the state database. Shared-install depots are
kept while another installed app uses them (§8). On Linux the app's
`compatdata/<appid>` holds the user's saves and settings under Proton and
is left in place unless the uninstall asks for its removal
(`compat-tools.md` §6.1).

## 13. Settings (CONT-7)

| Key | Default | Meaning |
|---|---|---|
| `content.bandwidth_limit` | 0 | KiB/s for all downloads together; 0 is no limit |
| `content.concurrent_jobs` | 1 | jobs that run at once (1 to 8) |
| `content.download_threads` | 8 | chunk downloads at once per job (1 to 64) |
| `content.auto_update` | `always` | `always` or `never` (§10) |
| `content.language` | `english` | the language whose depots are installed (§6) |

## 14. Verification status

| Statement | Status |
|---|---|
| Server list request and fields (§1) | unverified |
| `https_support` values (§1) | unverified |
| Depot key request, reply and refusal results (§2) | unverified |
| Manifest request codes and their lifetime (§3) | unverified |
| Manifest URL, zip wrapping and section format (§4) | unverified |
| File flags (§4) | unverified |
| Encrypted names (§4) | unverified |
| Chunk URL, encryption and the three compressed forms (§5) | unverified |
| Adler-32 seeded with 0 (§5) | unverified |
| Zip entries carry a correct CRC-32 (§5.3) | unverified |
| Chunk id is the SHA-1 of the decompressed data (§5) | unverified |
| Depot selection rules (§6) | unverified |
| CDN auth tokens (§7) | unverified |
