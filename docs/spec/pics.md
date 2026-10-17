# Licenses, product info (PICS) and persona state

| | |
|---|---|
| Status | Draft for M1a; §1 to §8 implemented, nothing yet tried with an account |
| Requirements | SPEC §9.3 (PICS-1 to PICS-3), §9.4 (PLAY-2), D-7, §19.3 |
| Code | `crates/ossteam-pics`; the daemon side in `crates/ossteamd/src/{pics,library}.rs` and the session worker |

After logon the CM tells the client which **licenses** the account holds.
A license names a **package** (also called a sub). The Product Info and
Change System (**PICS**) describes packages and apps: which apps and
depots a package grants, and each app's name, type, depots, branches and
launch options. The client keeps a cache of that information and polls
for changes. This page describes these exchanges in our own words. The
message schemas are in the vendored `steammessages_clientserver*.proto`
files (SPEC LIC-5), so fields are named here but not restated.

**Verification.** As in `cm.md`, this draft comes from public community
documentation. Each statement must be confirmed against a capture of the
official client on our test account before it is relied on, and §8 tracks
which have been. Rows marked *unverified* must not be treated as fact.

## 1. Licenses (PICS-1)

The server sends `ClientLicenseList` on its own, shortly after a
successful logon, and again whenever the account's licenses change (a
purchase, a free licence, a refund). Each message carries the complete
list, so the newest one replaces the previous one. `eresult` other than
OK means the list could not be produced; the daemon keeps the last list
it had and logs the failure.

Each license gives, among other things, the `package_id`, the `flags`,
the `license_type`, the `owner_id` (the account id that owns it), and the
package's `access_token` (§2).

**Which licenses grant something.** `flags` is a bit set. The bits
ossteam knows, in our own names:

| Bit | Meaning |
|---|---|
| `0x001` | renews automatically |
| `0x002` | the last renewal failed |
| `0x004` | pending: the purchase has not completed |
| `0x008` | expired |
| `0x010` | cancelled by the user |
| `0x020` | cancelled by Steam support |
| `0x040` | low-violence content |
| `0x080` | imported from the older Steam2 system |
| `0x100` | the launch must check restrictions |
| `0x200` | the region restriction expired |
| `0x400` | cancelled by a fraud lock |
| `0x800` | not activated yet |

A license grants its package unless it is pending, expired, cancelled
(any of the three cancel bits) or not activated. *Unverified: the
acceptance run compares the resulting library with the official client's
on the same account (`m1a.md`).*

**Borrowed licenses.** A license whose `owner_id` is non-zero and differs
from the logged-on account's id comes from Family Sharing. Family Sharing
is outside M1 (SPEC §21, "Later"), so these licenses are counted and
logged but grant nothing in ossteam's library. This is a known difference
from the official client until Family Sharing is built.

## 2. Access tokens (PICS-2)

Some packages and apps are described fully only to a client that presents
an access token for them. Package tokens arrive in the license list. App
tokens are requested with `ClientPICSAccessTokenRequest`, a job whose
reply is `ClientPICSAccessTokenResponse`. The reply lists a token for
each app that has one (0 means none is needed) and the ids for which a
token was denied. Tokens are kept in memory for the session and never
written to disk: they are per-account and the server can hand them out
again at any time.

ossteam asks for tokens for every app that its packages grant, in batches
of 256 ids per request. *The batch size is ossteam's own choice; the
official client's is not captured.*

## 3. Product info

`ClientPICSProductInfoRequest` lists apps and packages, each with its
access token when one is known. It is a job, and the server may answer it
with **several** `ClientPICSProductInfoResponse` messages, all targeting
the same job: every one but the last sets `response_pending`. ossteam
waits for each with a 30 s timeout and treats the exchange as complete
when a response arrives without `response_pending`.

Each response entry has an id, the object's `change_number`, its `sha`
(the SHA-1 of the buffer), `missing_token` when the info is limited
because no token was given, and the `buffer`. Ids the server does not
know come back in `unknown_appids` and `unknown_packageids`.

**`meta_data_only`.** With this set, the entries carry the change number
and hash but no buffer. ossteam uses it to find out which cached entries
are stale (§5).

**HTTP delivery.** A response may name an `http_host` and an
`http_min_size`, which suggests that large buffers can be fetched over
HTTP instead of arriving inline. *Unverified.* ossteam does not ask for
HTTP delivery, and an entry with an empty buffer but a non-zero `size` is
logged and treated as unavailable rather than guessed at.

**App buffers** are text KeyValues (`keyvalues.md` §1), ended by a NUL
byte that is not part of the text. The single root key is `appinfo`, and
its children are `appid` and the sections `common`, `extended`,
`config`, `depots`, `ufs` and others. ossteam reads:

| Path | Meaning |
|---|---|
| `common/name` | display name |
| `common/type` | the app type as a word: `Game`, `Application`, `Tool`, `Demo`, `DLC`, `Music`, `Video`, `Config`, ... Compared without regard to case |
| `common/parent` | for DLC, demos and soundtracks: the app they belong to |
| `common/oslist` | comma-separated platforms: `windows`, `macos`, `linux` |
| `extended/listofdlc` | comma-separated DLC app ids |
| `config/installdir` | the directory name under `common/` |
| `config/launch/<n>` | launch options: `executable`, `arguments`, `description`, `type`, and `config/oslist`, `config/osarch`, `config/betakey` |
| `depots/<id>` | one depot: `name`, `config/oslist`, `config/osarch`, `config/language`, `dlcappid`, `depotfromapp`, `sharedinstall`, `maxsize` |
| `depots/branches/<name>` | one branch: `buildid`, `description`, `pwdrequired`, `timeupdated` |

**Package buffers** start with a 4-byte little-endian field that ossteam
skips (*its meaning is unverified*), followed by binary KeyValues
(`keyvalues.md` §2). The single root key is the package id in decimal,
and its children include `packageid`, `billingtype`, `licensetype`,
`status`, `extended`, `appids`, `depotids` and `appitems`. `appids` and
`depotids` are lists whose keys are indexes (`0`, `1`, ...) and whose
values are the ids.

Requests carry at most 256 apps or packages each (ossteam's choice).

## 4. Changes (PICS-2)

Steam numbers every change to product info with a global, increasing
**change number**. `ClientPICSChangesSinceRequest` asks what changed after
a given number; it is a job answered by `ClientPICSChangesSinceResponse`.
ossteam sets `send_app_info_changes` and `send_package_info_changes`. The
response gives the current change number and the apps and packages that
changed, each with its own change number. `force_full_update`,
`force_full_app_update` or `force_full_package_update` means the gap was
too large to list; the client must then check every object it cares about
as in §5.

While logged on, ossteam polls every 60 s, starting from the current
change number it learned at logon. For each changed object that the
account's library uses, it compares the new change number with the cached
one and fetches the info again when they differ. A changed package can
grant different apps, so the library is recomputed after packages are
refreshed. Update detection for installed apps (M1b) hooks in here.

## 5. Bringing the cache up to date at logon

When a license list arrives, the daemon:

1. asks for changes since its last known change number (0 the first
   time) only to learn the current change number for later polls (§4);
2. asks for the granted packages' metadata (`meta_data_only`) and fetches
   the full info of every package that is not cached or whose cached
   change number differs;
3. collects the apps those packages grant, requests their access tokens
   (§2), and does the same check and fetch for the apps.

Checking metadata at every logon, rather than trusting the change number
saved by an earlier run, keeps entries correct even when they were cached
for another account and changed while nobody tracked them.

## 6. The cache (D-7)

The cache lives in the cache directory under `pics/` and can be deleted at
any time; it is rebuilt from the network.

```
pics/app/<appid>.vdf        one app
pics/package/<packageid>.vdf one package
pics/state.vdf              the last change number seen
```

Each entry file is binary KeyValues with one root object holding
`change_number` (uint64), `sha` (lower-case hex string), `missing_token`
(int32, 0 or 1) and `data`, the object parsed from the buffer (for apps,
the contents of `appinfo`; for packages, the contents of the root key).
Text conditionals do not survive this (`keyvalues.md` §2) and app info
does not use them. Files are written to a temporary name and renamed into
place. A file that does not parse is deleted and fetched again.

Licenses and access tokens are not cached.

## 7. The library (PICS-3)

The library is derived, never stored:

- The **granted packages** are those of the licenses that grant something
  (§1).
- The **granted apps** are the union of those packages' `appids`.
- Each app's name and type come from its cached info. An app whose info is
  not available yet, or was denied, is still listed, by id, with an empty
  name and type, so the list never hides something the licenses grant.
- **DLC grouping.** An app of type DLC is listed under its `common/parent`
  app. If the parent is not itself granted, the DLC is listed on its own
  and says which app it belongs to.

*Unverified:* whether the official client hides some types (for example
`Config`) from its library view. IPC returns every granted app with its
type so that clients can filter; the acceptance run settles what the CLI
shows by default.

Install state comes from the install engine (M1b). Until it exists, no
app is installed, which is the true state.

## 8. Persona state (PLAY-2)

Persona state is what friends see. ossteam lets the user choose one of
three, stored per account in the state database (default: online):

| ossteam | Steam persona state |
|---|---|
| online | 1 |
| invisible | 7 |
| offline | 0 |

Steam's other states (2 busy, 3 away, 4 snooze, 5 looking to trade, 6
looking to play) are not offered in M1.

After every successful logon, and whenever the user changes it while
logged on, the daemon sends `ClientChangeStatus` with `persona_state` set
and `persona_set_by_user` true. The message is not a job and has no reply
that ossteam waits for; the server reflects the change in
`ClientPersonaState` messages to friends. *Unverified: the fields the
official client sets, and whether it sends this before or after other
post-logon requests.*

## 9. App images

An app's info names its library and store images under `common`, each as
a path per language:

| Image | Where in the info |
|---|---|
| Header | `header_image/<language>` |
| Capsule (portrait) | `library_assets_full/library_capsule/image/<language>`, and `image2x` for double size |
| Hero, blurred hero | `library_assets_full/library_hero/...`, `library_hero_blur/...`, same shape |
| Logo | `library_assets_full/library_logo/...`, same shape, plus a `logo_position` the GUI places it by |

A path is either a file name (`library_600x900.jpg`) or a content hash
directory and a file name (`<40 hex digits>/library_capsule.jpg`).
`common/store_asset_mtime` versions the images. The URL of an image is

    https://shared.steamstatic.com/store_item_assets/steam/apps/<appid>/<path>?t=<store_asset_mtime>

with `t=0` when the info has no `store_asset_mtime`. The CDN answers 404
for an image that does not exist. Many apps, mostly DLC and tools, name
no library images at all; some name only a header.

The daemon picks the path in the client language, falling back to
`english`. Until a language setting exists it always uses `english`. With
`high_dpi` it prefers `image2x` and falls back to `image`; the header has
no double size. It accepts only plain relative paths: `/`-separated names
of ASCII letters, digits, `.`, `_` and `-`, none of them empty, `.` or
`..`, so a path from the network can neither leave the app's directory on
the CDN nor in the cache. An image the info does not name, or names with
any other path, is reported as not found.

Images are cached in `<cache>/assets/<appid>/<store_asset_mtime>/<path>`,
written under a temporary name and renamed into place. After an image of a
new version is stored, the app's directories for other versions are
removed. A cached file never changes, so a client may keep a path it was
given until it asks again.

*How this was found:* the official client's own library cache
(`appcache/librarycache/<appid>/<path>`) mirrors the info's paths
exactly. Its UI builds library image URLs as
`<store app images base>/<appid>/<path>?t=<store asset mtime>`, and the
native client's default for that base is the URL above. A capsule and a
logo fetched from it on 2026-10-17 matched the official client's cached
files byte for byte. The official client's logs show it using
`shared.fastly.steamstatic.com` instead; how it picks a CDN host is not
known. It also falls back to a DLC's parent app for a missing hero,
which ossteam leaves to the GUI.

## 10. Verification status

| Statement | Status |
|---|---|
| License list pushed after logon and on changes (§1) | unverified |
| License flag bits and which ones grant nothing (§1) | unverified |
| `owner_id` marks Family Sharing licenses (§1) | unverified |
| Access-token request and reply (§2) | unverified |
| Product info split across responses with `response_pending` (§3) | unverified |
| App buffer: text KeyValues with a trailing NUL, root `appinfo` (§3) | unverified |
| Package buffer: 4-byte prefix, then binary KeyValues (§3) | unverified |
| `meta_data_only` entries carry change numbers (§3) | unverified |
| Changes-since request and the force-full flags (§4) | unverified |
| Persona state numbers and `ClientChangeStatus` (§8) | unverified |
| Image paths in the info, the URL scheme and `t` (§9) | verified 2026-10-17 against the official client's cache and CDN |
| How the official client picks a CDN host (§9) | unknown |
