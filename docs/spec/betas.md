# Password-protected betas

| | |
|---|---|
| Status | Draft for M3; awaiting review. Nothing has been built, and §8 lists what must be observed first |
| Requirements | SPEC §10 (CONT-9), M3, §12.3, §18 (SEC-2), FAITH-1 to FAITH-4, D-10 |
| Code | none yet. M1 refuses these branches in `crates/ossteam-content/src/depots.rs` (`check_branch`, `manifest_of`), and `crates/ossteam-content/src/request.rs` sends no `branch_password_hash` |

CONT-9 leaves password-protected betas to M3. A beta is a branch of an
app (`content.md` §6) that the developer publishes beside `public`. Some
branches anyone who owns the app can pick. Others need a code the
developer hands out: a "beta password" in the official client's
Properties → Betas. This page says how a client learns which private
branches a code unlocks, how it gets their manifests, what it keeps,
and how ossteam would do the same. It also covers switching the branch
of an installed app, which M1 does not have and which betas are mostly
used for.

**Verification.** This draft draws on the following sources:

- the message definitions that SteamDatabase extracts from the official
  client (`steammessages_clientserver_2.proto`,
  `steammessages_clientserver_appinfo.proto`,
  `steammessages_contentsystem.steamclient.proto`,
  `enums_clientserver.proto`);
- SteamKit2's `SteamApps` handler and DepotDownloader's
  `ContentDownloader`/`Steam3Session`, for the order of the requests and
  what each reply holds. Facts only, as with every community client;
- the Steamworks SDK headers that Proton ships (`isteamapps.h`,
  `steamclientpublic.h`, SDK 1.60 to 1.65): the beta methods and
  `EBetaBranchFlags`;
- the app info of the 201 apps in the development computer's PICS cache
  (`~/.cache/ossteam/pics/app/`), and the official client's
  `steamapps/appmanifest_*.acf`, `config/config.vdf` and
  `userdata/*/config/localconfig.vdf`. Only their structure is used here.

Nobody has unlocked a beta with a code on the development computer, so
everything about how the official client stores an unlocked beta is
*unverified*, and §8 lists the captures that would settle it.

## 1. Branches in the app info

`depots/branches/<name>` lists the branches that the app info shows to
every owner (`pics.md` §3). The keys seen in the cache are `buildid`,
`description`, `timebuildupdated`, `timeupdated` and, on one app,
`lcsrequired` (§1.2).

### 1.1 Private branches are not listed

`pics.md` §3 and M1's `check_branch` expect a password-protected branch
to appear under `branches` with `pwdrequired` set, and a depot to name
its manifest for it under `encryptedmanifests/<name>`. Neither appears
anywhere in the cache: no branch has `pwdrequired`, and no depot has
`encryptedmanifests`. Instead, 32 of the 133 apps that have branches
also carry `depots/privatebranches` = `1`, and their `branches` lists
only `public` and the open betas.

So today's app info does not name private branches. It only says that
the app has some. Their names, descriptions, build ids and manifests
arrive only after a code is accepted (§2).

`pwdrequired` and `encryptedmanifests` are older forms. Community
clients still read them, and `encrypted_gid_2` held the manifest id
encrypted with the key that §2.1 returns. As no app in the cache uses
them, ossteam would keep M1's handling: a branch with `pwdrequired` and
a depot whose manifest is only under `encryptedmanifests` fail the
install as unsupported, with a message that names the older form. It
would implement them only once an app that uses them turns up (§9 Q4).

### 1.2 Branches that need a local content server

A branch with `lcsrequired` set (seen as `local`, described as "Local
Content Server") is served only from a content server on the local
network, which ossteam does not use (`content.md` §7). M1's
`check_branch` accepts such a branch today. Its depots then most likely
name no manifest for it and the install chooses nothing. ossteam should
refuse it before the job starts, as `UNSUPPORTED` with a message that
names the reason. That is a small M1 fix and does not wait for the rest
of this page (§9 Q5).

## 2. Unlocking with a code

### 2.1 Checking the code

The client sends `ClientCheckAppBetaPassword` (EMsg 5450) as a job:

| Field | Value |
|---|---|
| `app_id` | the app |
| `betapassword` | the code as the user typed it |
| `language` | an `ELanguage`; community clients leave it out. *Unverified:* whether it chooses the language of `betadescription` |

`ClientCheckAppBetaPasswordResponse` (5451) carries `eresult` and a
list of `betapasswords`. Each entry has the branch's name
(`betaname`), its description (`betadescription`) and a key
(`betapassword`) as a hex string. SteamKit2 decodes the hex into bytes,
and so would ossteam. One code can unlock more than one branch, and a
response that does not name the branch the user wanted means that the
code does not unlock it. *Unverified:* the `eresult` of a wrong code.
DepotDownloader looks only at the list, so a wrong code may come back as
OK with no entries.

The key is not the code. It is what the client sends from then on (§2.2,
§3), so a client that keeps the key never needs the code again.
*Unverified:* whether the official client keeps the key, the code, or
both (§8).

### 2.2 The private depot section

With the key, the client asks for the branch's part of the app info with
`ClientPICSPrivateBetaRequest` (8907), also a job:

| Field | Value |
|---|---|
| `appid` | the app |
| `access_token` | the app's PICS access token (`pics.md` §2), or 0 |
| `beta_name` | the branch's name from §2.1 |
| `password_hash` | the key from §2.1, as bytes |

`ClientPICSPrivateBetaResponse` (8908) carries `eresult` and
`depot_section`: a text KeyValues document followed by a NUL byte, which
is dropped before parsing (`keyvalues.md` §1). It has the same shape as the
app info's `depots` object. A depot that has content on the branch lists
it under `<depot>/manifests/<branch>/gid`, which is where DepotDownloader
looks. SteamKit2 calls the section "to be merged into main appinfo".
*Unverified:* whether it also holds `branches/<branch>` with the build
id, or only the depots. If it does not, the build id comes from
somewhere else, or ossteam shows none (§8).

The section is fetched again each time the app's info changes
(`pics.md` §4), as the branch's manifests change with it. ossteam would
never cache it on disk: it is as private as the key that unlocks it, and
it can be fetched again whenever the key is still good.

### 2.3 Choosing depots on a private branch

`content.md` §6 runs unchanged, with one difference: rule 6 reads
`manifests/<branch>` from the depot entry in the private section, and
falls back to the app info's entry for the depot's other keys
(`config/oslist`, `dlcappid`, `depotfromapp` and so on). A depot that
the private section does not list has no content on the branch and is
skipped, as rule 6 already says. *Unverified:* whether a shared depot
(rule 7) on a private branch takes its manifest from the other app's
private section, which would need that app's key too. Until an app shows
this, such a depot fails the install as unsupported.

## 3. Downloading

The depot keys (`content.md` §2) do not depend on the branch.

`ContentServerDirectory.GetManifestRequestCode` takes `app_branch` and
`branch_password_hash` (a string). DepotDownloader sends the branch name
and no hash, and its private-branch downloads work. ossteam would do the
same at first. If the server answers 0 for a private branch, the next
thing to try is the key as the lowercase hex string that §2.1 received.
*Unverified:* which of the two is right. Either way the code is never
sent here.

Chunks and CDN tokens do not depend on the branch.

## 4. What ossteam keeps

- **The key.** An unlocked branch's key is a secret (SEC-2). It is
  stored like guard data (`auth.md` §5): in the OS keyring, under
  the service `ossteam-beta` with the key `<account>/<app>/<branch>`, or
  in `secrets.json` with the same fallback setting. It never appears in
  logs or in IPC replies (D-10). An account that logs in with
  `--no-remember` keeps its keys in memory only.
- **The code.** Never stored. It is needed only to obtain the keys, and
  it is the same for everyone the developer gave it to. *Unverified:*
  whether the official client stores it.
- **Which branches are unlocked.** The database records, per account and
  app, the names of unlocked branches with their descriptions from
  §2.1. That is what lists them after a restart without asking Steam
  again. The keys live only in the keyring.
- **Revocation.** A developer can change a code. When
  `ClientPICSPrivateBetaRequest` with a stored key fails, the daemon
  marks the branch locked again and keeps the installed files. An
  installed app on that branch then shows that its branch needs a new
  code. It is not moved to `public` behind the user's back.
  *Unverified:* which `eresult` a changed code gives.

## 5. Choosing the branch of an installed app

M1 picks the branch only at install time (`ipc.md` §5.6). The official
client lets the user change it at any time in Properties → Betas, and
then updates the app to the new branch. The installed state already
records the branch.

ossteam would add `SetBranch{app_id, branch}` to `ipc.md`. It checks
the branch like `Install` does, with private branches allowed when
unlocked, records it as the app's *selected* branch, and queues an
update. `content.md` §10 handles the files: an update to another branch
is an update whose manifests come from that branch. The installed
branch changes only when the job is done. A failed or cancelled job
leaves the app on its old branch and the selection where the user put
it, as the official client's Betas tab does. *Unverified:* that the
official client keeps the selection when the switch fails.

Update detection (`content.md` §10) compares against the *selected*
branch. An app whose selected and installed branches differ always has
an update.

## 6. IPC, CLI and GUI

**IPC** (schema change; exact fields come with the code):

| Method | Does |
|---|---|
| `UnlockBetas{app_id, code}` | Sends §2.1. Stores the keys and replies with the branches the code unlocked. `NOT_FOUND` when the app is not in the library; `INVALID_ARGUMENT` when the code unlocks nothing; `NOT_LOGGED_IN` when no account is logged on. The code is a secret type in the daemon and is dropped after the request |
| `ForgetBetas{app_id}` | Deletes the app's keys and unlocked branches. Refused with `CONFLICT` while an installed app or a job uses one |
| `SetBranch{app_id, branch}` | §5 |

`GetApp`'s `branches` gains each branch's `private`, `unlocked`,
`selected` and `installed` flags, and `lcsrequired` (§1.2). Private
branches that are not unlocked are not listed, as they have no names,
but the app gets `has_private_branches` from `depots/privatebranches`.

**CLI:** `ossteam beta unlock <APP>` prompts for the code without
echoing it, like `login` prompts for a password, so that it never
lands in shell history. `ossteam beta forget <APP>`, and `ossteam
branch <APP> <NAME>` to switch an installed app. `ossteam info` already
lists branches and would show the new flags (§9 Q3).

**GUI:** the App page's branch chooser lists unlocked private branches
with the others, marks them as private, and offers "Enter beta code…"
when the app has private branches. Choosing another branch for an
installed app calls `SetBranch` after the same confirmation the
official client asks for.

## 7. `ISteamApps`

The beta methods are SDK 1.60 and later. M1 makes them fail under
ABI-3 (`steamworks/isteamapps.md`). With this page they would answer:

| Method | ossteam |
|---|---|
| `GetCurrentBetaName(name, size)` | Unchanged: the installed branch and true, or false for `public` |
| `GetNumBetas(available, private)` | The number of branches that `GetApp` lists, `public` included; `available` counts those the account can select, and `private` those that are private and unlocked. *Unverified:* whether the official client counts locked private branches, which it cannot name |
| `GetBetaInfo(i, flags, build, name, nsize, desc, dsize)` | Branch `i` in the order `public` first, then by name: `EBetaBranchFlags` (`Default` 1 for `public`, `Available` 2, `Private` 4, `Selected` 8, `Installed` 16), its build id, name and description, each truncated to fit with its NUL. False when `i` is out of range |
| `SetActiveBeta(name)` | `SetBranch` (§5) for the running game's own app. The update waits until the game exits (`content.md` §10 never updates a running app). True when the daemon accepted the request |

`SetActiveBeta` changes what the next start installs, so it is only
honoured for the game's own app id. The game IPC (`steamworks-abi.md` §3)
carries it as a request the daemon checks like `SetBranch`, never as a
command the shim carries out itself (SEC-4).

## 8. Investigation before code

1. **A code to test with.** Some developers publish beta codes. A free
   app with a published code would let the test account unlock a branch
   without spending anything. Candidates still need to be found. Spacewar
   (480) has none in the cache.
2. **The official client unlocking it** on the test account, with its
   logs (`content_log.txt`, `appinfo_log.txt`) and config files compared
   before and after, to see what it stores (key, code, or both, and
   where) and whether the private section holds the build id.
3. **A wrong code**, to see its `eresult`, and a code that has been
   changed, if the test title allows that.
4. **A download on the private branch** with and without
   `branch_password_hash` (§3).

ossteam's own requests for 2 to 4 come from ossteam with the user
watching. Nothing here is sent before the user has approved it.

## 9. Questions for review

1. **Storing the key** (§4): in the keyring, as proposed, so that an
   unlocked beta survives a restart without the code? The alternative is
   to keep it in memory and ask for the code again after each restart,
   which the official client does not seem to do (*unverified*, §8).

   *Open (2026-11-03):* the maintainer has left this open.

2. **Switching branches** (§5): in M3 with betas, as proposed? Without
   it a private branch can only be chosen at install time, and
   `SetActiveBeta` stays unimplemented.

   *Answer (2026-11-03):* yes, in M3 with betas.

3. **The CLI** (§6): `ossteam beta unlock|forget <APP>` and `ossteam
   branch <APP> <NAME>`? Or `--branch` on a new `ossteam update`?

   *Answer (2026-11-03):* `ossteam beta unlock|forget <APP>` and `ossteam
   branch <APP> <NAME>`.

4. **The older forms** (§1.1): leave `pwdrequired` and
   `encryptedmanifests` unsupported until an app that uses them turns up,
   as proposed?

   *Open (2026-11-03):* the maintainer has left this open.

5. **`lcsrequired`** (§1.2): fix it now, as an M1 fix, so that such a
   branch is refused before the job starts?

   *Open (2026-11-03):* the maintainer has left this open.

## 10. Verification against the official client

| Item | Status |
|---|---|
| Message fields, EMsg 5450/5451/8907/8908 | from the client's message dumps |
| Order: code check, then private section | SteamKit2 and DepotDownloader; unverified for the official client |
| Key is hex in the reply and bytes in 8907 | SteamKit2; unverified |
| `depots/privatebranches` and no `pwdrequired`/`encryptedmanifests` today | 201 cached apps on the development computer |
| `depot_section` is text KeyValues with a trailing NUL | SteamKit2; unverified |
| What the section holds besides manifests | unverified |
| `branch_password_hash` | not sent by DepotDownloader; unverified |
| What the official client stores, and where | unverified (§8) |
| `eresult` for a wrong or changed code | unverified |
| `EBetaBranchFlags` and the `ISteamApps` beta methods | SDK headers |
| `GetNumBetas` counting | unverified |
