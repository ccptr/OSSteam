# Workshop (UGC)

| | |
|---|---|
| Status | Draft for M3; awaiting review. Nothing has been built, and §10 lists what must be observed first |
| Requirements | SPEC M3, §10 (CONT-1 to CONT-6), §12.3, §12.4 (GIPC-1, GIPC-2), FAITH-1 to FAITH-4, ABI-3 |
| Code | none yet. `ISteamUGC` is reachable through its getter and fails under ABI-3 (SPEC §12.3) |

The Workshop is where players publish content for a game, such as maps,
mods and item skins, and where other players *subscribe* to it. Steam then
keeps a copy of each subscribed item on the computer and up to date, and
the game finds it through `ISteamUGC`. A game can also ask for an item
it is not subscribed to (a map the server is running, say), which Steam
downloads and keeps as a *cached* item.

This page covers the consumer side: which items an account has, where
their files go, how they are downloaded and updated, and what the game
sees. Publishing (§8) is described only as far as the sources go, and §11
asks whether it belongs in M3 at all.

**Verification.** This draft draws on the following sources:

- the message definitions that SteamDatabase extracts from the official
  client (`steammessages_publishedfile.steamclient.proto`,
  `steammessages_clientserver_ucm.proto`,
  `steammessages_workshop.steamclient.proto`,
  `steammessages_cloud.steamclient.proto`, `enums_clientserver.proto`);
- DepotDownloader's `ContentDownloader` (`-pubfile` and `-ugc`), for how
  an item's details lead to its content. Facts only;
- the Steamworks SDK headers that Proton ships (`isteamugc.h`,
  `isteamremotestorage.h`, SDK 1.52 to 1.65);
- the app info of the 201 apps in the development computer's PICS cache;
- the official client's `steamapps/workshop/` tree,
  `appworkshop_730.acf`, `logs/workshop_log.txt` and
  `logs/content_log.txt` on the development computer. They record one
  subscribed and one cached item of Counter-Strike 2 (730). Only their
  structure is used here, never their values.

Nothing has been subscribed to with the test account, so the official
client's requests are inferred from its log lines and are *unverified*
until §10 is done.

## 1. Items

An item is a *published file*, named by a 64-bit `publishedfileid`.
`PublishedFile.GetDetails` returns its `PublishedFileDetails`. The fields
this page uses are:

| Field | Use |
|---|---|
| `consumer_appid`, `creator_appid` | The app that uses the item, and the app it was made with (often the same) |
| `file_type` | `EWorkshopFileType`: 0 Community (an ordinary item), 2 Collection, 3 Art, 5 Screenshot, 10 IntegratedGuide, 12 ControllerBinding, and others that carry no files to install |
| `hcontent_file` | For a depot-based item, the manifest id of its content (§3) |
| `file_url`, `filename`, `file_size` | For a *legacy* item (one file, made with `ISteamRemoteStorage`), the URL its file is downloaded from, its name and its size (§3.3) |
| `time_updated` | When the item last changed |
| `children` | A collection's items, and an item's dependencies |
| `visibility`, `banned`, `can_subscribe` | Whether the account may see or subscribe to it |
| `revision`, `available_revisions` | `EPublishedFileRevision`: 0 Default, 1 Latest, 2 and 3 approved snapshots, 6 author snapshot. ossteam asks for Default, as the requests in the logs do (§10) |

Only items with files are installed: Community, and the other types that
DepotDownloader also treats as downloadable (Art, Screenshot, Merch,
IntegratedGuide, ControllerBinding). A collection has no files; its
`children` are the items. *Unverified:* whether subscribing to a
collection subscribes the account to each child, as the website's
"Subscribe to all" does.

## 2. Which items an account has

### 2.1 The app info

`depots/workshopdepot` names the depot that holds the app's items, and
`common/workshop_visible` says the app has a Workshop. In the cache, 6
apps have a `workshopdepot`, and each names the app's own id as the depot:
730, 570, 480 (Spacewar), 440, 250820 and 241100. That is also what
DepotDownloader expects. An app whose info has no `workshopdepot` has no
items to install, and its `ISteamUGC` content methods fail (§7).

`workshopdepot` is not a depot to choose for the app (`content.md` §6
already skips it).

### 2.2 Subscriptions

The official client's log asks for the account's subscriptions once a
day per installed app ("Requesting updated subscription list", and
"Skipping subscription list update. Last updated less than a day ago"
in between). The request is most likely
`ClientUCMEnumerateUserSubscribedFilesWithUpdates` (EMsg 7378):

| Field | Value |
|---|---|
| `app_id` | the app |
| `start_index` | 0, then the number of results so far, for paging |
| `start_time` | 0 for the whole list. *Unverified:* whether the client passes the last update time to get only changes |
| `desired_revision` | 0 |

Its response (7379) has `eresult`, `total_results` and
`subscribed_files`, each with `published_file_id`, `rtime32_subscribed`,
`appid`, `file_hcontent` (the manifest), `file_size`,
`rtime32_last_updated`, `is_depot_content` and `author_snapshots`. One
response therefore gives everything needed to install or update the
items without asking for their details. *Unverified:* that the official
client sends this rather than `PublishedFile.GetUserFiles` with
`type` = `mysubscriptions`, which returns the same items as details.
ossteam would use 7378 and fall back to `GetUserFiles` if the server no
longer answers it.

ossteam would ask for the list at logon and then once a day per app that
has a `workshopdepot` and is installed, as the official client does.

### 2.3 Changes while logged on

Three messages tell a logged-on client about changes:

- `PublishedFileClient.NotifyFileSubscribed` (a service notification to
  the client): an item was subscribed to, with `app_id`,
  `file_hcontent`, `file_size`, `rtime_updated`, `is_depot_content` and
  per-revision data. This is what makes a subscription made on the
  website start a download at once (`Detected workshop change : added
  subscribed item` in the log).
- `PublishedFileClient.NotifyFileUnsubscribed` and `NotifyFileDeleted`.
- `ClientUCMPublishedFileUpdated` (EMsg 7381): an item has new content,
  with `hcontent`, `time_updated` and `file_size`.

*Unverified:* that the server sends `NotifyFileSubscribed` only when the
subscribe request asked for `notify_client`, and whether 7381 arrives
for cached items as well as subscribed ones.

### 2.4 Checking for updates

The log shows two more checks, at logon and when an app starts:

- "Item change feed returned Ignored (revision 0, cursor N)": most
  likely `PublishedFile.GetItemChanges` with the app and the time of the
  last check. It lists items of the app that changed since then
  (`published_file_id`, `time_updated`, `manifest_id`). `Ignored` (32)
  seems to mean "nothing new". The older `ClientWorkshopItemChangesRequest`
  has the same fields.
- "Item info for N item(s) (full list): revision 0, U update(s), P
  private, update time T": `PublishedFile.GetItemInfo` with the app, the
  last update time and every installed item with its `time_updated`
  (`full_reconcile` set for the full list). The response gives the
  current `manifest_id` of each item that changed and lists, under
  `private_items`, the items the account can no longer see.

For an item in `private_items` the official client still updated it in
the one case logged ("Item info … (unknown only) … 1 private" followed
by "changed cached item …, new manifest …"), so the new manifest came
from somewhere else, most likely the `GetDetails` it had sent for that
item 14 s earlier. *Unverified.*

ossteam would send `GetItemInfo` with every installed item at logon and
when the app starts, and nothing else. `GetItemChanges` adds nothing
that `GetItemInfo` and §2.3 do not give, and its `Ignored` result is not
understood yet (§10).

## 3. Downloading

### 3.1 Depot-based items

An item's content is a depot manifest in the app's `workshopdepot`, and
everything in `content.md` applies, with the item standing in for the
app:

1. The depot key: `ClientGetDepotDecryptionKey` with `depot_id` and
   `app_id` both set to the workshop depot's app (`content.md` §2).
   *Unverified:* whether item chunks are encrypted at all.
2. The manifest request code: `GetManifestRequestCode` with the app,
   the workshop depot and the item's manifest id, and no branch. The
   official client's request in `content_log.txt` has the form
   `/depot/730/manifest/<manifest>/5/<code>`, so it asks for a code too.
3. The manifest and the chunks, from the same content servers as the
   app's own depots (`content.md` §1, §4, §5).

One job downloads all the items of one app that need it, as the official
client does ("Starting Workshop download job", one per app, with its own
`Workshop update changed` states).

### 3.2 On disk

The official client puts items under the library folder that holds the
app:

```
steamapps/workshop/
  appworkshop_<app>.acf            the app's items and their state
  content/<app>/<item>/            an item's files
  downloads/<app>/                 staging
  temp/
```

ossteam does not mirror it: every path is its own (maintainer,
2026-11-03). Items live in the library folder that holds the app, inside
ossteam's own library layout (`content.md` §8), whose default folder is
in the data directory (`$XDG_DATA_HOME/ossteam/library`, SPEC §17):

```
<library>/
  common/<installdir>/                    the app
  workshop/<app>/<item>/                  an item's files
  .ossteam/staging/workshop/<app>/<item>/ files being written
```

A game finds its items through `ISteamUGC`: `GetItemInstallInfo` gives
an item's folder (§7). A game that looks for them by a path relative to
its install directory, as the official layout allows
(`../../workshop/content/<app>`), does not find them (*unverified:*
whether any game does this). There is no
`.acf`: the state lives in the database (§4).

Staging, renaming into place, the checks for symbolic links and the
disk-space check work as in `content.md` §8. An update of an item works
as in `content.md` §10, with the item's old manifest as the installed
one, so unchanged chunks are copied instead of downloaded.

### 3.3 Legacy items

A legacy item has one file and a `file_url` instead of a manifest.
DepotDownloader fetches the URL over HTTPS and writes the file under its
`filename`. ossteam would do the same into
`workshop/content/<app>/<item>/<filename>`, checking the name as a
single path component and the length against `file_size`. There is no
hash to check it against. *Unverified:* whether the official client puts
legacy files in the same directory; `ItemInstalled_t` carries
`m_hLegacyContent`, which suggests the game is told the file's UGC
handle instead.

### 3.4 When downloads run

| Case | Official client (from the logs) | ossteam |
|---|---|---|
| Item subscribed to | downloaded at once ("added subscribed item") | queued as a job for the app |
| `DownloadItem` from the running game | downloaded at once, committed while the game runs ("Workshop update job started while app is running, commit : yes") | the same: a high-priority job that commits at once |
| Update of a subscribed item while the game runs | *unverified*: the log's `commit : yes` suggests that the client can also defer the commit | deferred until the game exits, as `content.md` §10 does for apps |
| Unsubscribed item | removed after the game exits (`UnsubscribeItem` in the SDK header) | removed when no running game of the app is left, unless another account on the computer subscribes to it |
| App uninstalled | *unverified* | items removed with the app; they can be downloaded again |

A game started while its items are being updated is not held back.
*Unverified:* whether the official client waits for workshop updates
before it starts a game. If it does, ossteam would do the same (§10).

`content.auto_update` = `never` (`content.md` §13) applies to items as
well: they are marked as needing an update and not downloaded, except
when the game asks with `DownloadItem`.

## 4. What ossteam keeps

The official client's `appworkshop_<app>.acf` records, per app,
`SizeOnDisk`, `NeedsUpdate`, `NeedsDownload`, `TimeLastUpdated`,
`TimeLastAppRan` and two lists:

- `WorkshopItemsInstalled/<item>`: `size`, `timeupdated` and `manifest`
  of what is on disk;
- `WorkshopItemDetails/<item>`: the installed and the latest `manifest`
  and `timeupdated`, `timetouched` and, for a subscribed item,
  `subscribedby` (an account id). The cached item has no `subscribedby`.

ossteam's database would hold the same per library: for each item, its
app, the installed manifest, `time_updated` and size, the latest
manifest and `time_updated`, when a game last used it, and the set of
accounts on this computer that subscribe to it. An item without a
subscriber is cached. The list of subscriptions itself is the server's
and is asked for again (§2.2); the database keeps the last copy so that
an account offline still sees its items.

**Cached items** stay until the game calls
`MarkDownloadedItemAsUnused` (SDK 1.65), until the app is uninstalled,
or until they no longer fit the cache. `workshop.cache_limit`
(`ipc.md` §5.4) bounds the size of all cached items together, across
library folders; 0, the default, is no limit. When cached items exceed
it, the daemon deletes those a game used least recently until they fit,
never a subscribed item, and never an item of an app that is running.
*Unverified:* whether the official client evicts them on its own after a
time; it keeps `timetouched`, which suggests it does.

**Per-account choices** that SDK 1.6x added, `SetItemsDisabledLocally`
and `SetSubscriptionsLoadOrder`, are stored per account and app in the
database. *Unverified:* where the official client keeps them and whether
Steam syncs them.

## 5. IPC, CLI and GUI

**IPC** (schema change; exact fields come with the code):

| Method | Does |
|---|---|
| `ListWorkshopItems{app_id}` | The account's items of the app, subscribed and cached, with state (§7's `EItemState` bits), size, title and progress |
| `Subscribe{app_id, item}` / `Unsubscribe{app_id, item}` | `PublishedFile.Subscribe` / `Unsubscribe` (§6) |
| `UpdateWorkshop{app_id}` | Sends §2.4 and queues a job for what needs it |

Workshop jobs appear in the job list (`ipc.md` §5.6) with kind
`workshop` and the app's id. Pause, resume and cancel work as for app
jobs.

**CLI:** `ossteam workshop <APP>` lists the items. `ossteam workshop
subscribe|unsubscribe <APP> <ITEM>` and `ossteam workshop update <APP>`.

**GUI:** the App page gets a Workshop section with the items, their
state and size, and a link that opens the app's Workshop page in the
browser. Browsing and searching the Workshop happen on the website, as
they mostly do in the official client.

## 6. Subscribing

`PublishedFile.Subscribe` takes `publishedfileid`, `list_type`, `appid`,
`notify_client` and `include_dependencies`. *Unverified:* the value of
`list_type` for subscriptions. The SDK's `EUserUGCList` has Subscribed
at 6 (counting from Published = 0), but nothing ties the two together;
§10 settles it. ossteam would send `notify_client` = true and
`include_dependencies` = true, so that the item's dependencies are
subscribed to as well (*unverified* that this is what the SDK's
`SubscribeItem` does).

## 7. `ISteamUGC`

`ISteamUGC` is reachable through `ISteamClient::GetISteamUGC`. Its
calls go to the daemon over the game IPC (`steamworks-abi.md` §3) and
act only for the app the daemon knows the game by (GIPC-2): `SubscribeItem`
subscribes for that app, and `GetSubscribedItems` lists that app's
items. Where a method takes another app's id (a query's
`nConsumerAppID`, `CreateItem`), the daemon passes it on and Steam
decides, so the game gets what Steam would grant it and no more.

| Methods | ossteam |
|---|---|
| `GetNumSubscribedItems`, `GetSubscribedItems` | The app's subscribed items from §4. Items disabled locally are left out unless the game asks for them |
| `GetItemState` | `EItemState` bits from §4: Subscribed 1, LegacyItem 2, Installed 4, NeedsUpdate 8, Downloading 16, DownloadPending 32, DisabledLocally 64; 0 for an item ossteam does not track |
| `GetItemInstallInfo` | Size on disk, the item's directory (§3.2), and its `time_updated`. False when not installed |
| `GetItemDownloadInfo` | Bytes downloaded and total of the item's job. False when it has none |
| `DownloadItem` | §3.4. True when the daemon accepted the request; `DownloadItemResult_t` follows, and `ItemInstalled_t` when the files are in place |
| `SubscribeItem`, `UnsubscribeItem` | §6, as API calls with `RemoteStorageSubscribePublishedFileResult_t` / `…Unsubscribe…`; `RemoteStoragePublishedFileSubscribed_t` / `…Unsubscribed_t` when §2.3 reports it |
| `GetNumDownloadedItems`, `GetDownloadedItems`, `MarkDownloadedItemAsUnused` | SDK 1.65: cached and subscribed items on disk; §4 |
| `SuspendDownloads` | Pauses the app's workshop job while the game runs, until it resumes them or exits |
| `CreateQueryUserUGCRequest`, `CreateQueryAllUGCRequest`, `CreateQueryUGCDetailsRequest`, the `SetReturn…`/`Add…`/`Set…` filters, `SendQueryUGCRequest`, `GetQueryUGC…`, `ReleaseQueryUGCRequest`, `RequestUGCDetails` | The shim builds the query, and `SendQueryUGCRequest` sends it through the daemon as `PublishedFile.GetUserFiles`, `QueryFiles` or `GetDetails`. The shim keeps the results per handle until the game releases it. The mapping of each filter to a request field is listed with the code |
| `StartPlaytimeTracking`, `StopPlaytimeTracking`, `StopPlaytimeTrackingForAllItems` | `PublishedFile.Start…`/`Stop…PlaytimeTracking` for the game's app. The daemon sends `StopPlaytimeTrackingForAllAppItems` itself when the game exits, in case it did not |
| `SetUserItemVote`, `GetUserItemVote`, `AddItemToFavorites`, `RemoveItemFromFavorites`, `GetAppDependencies`, `GetWorkshopEULAStatus` | One request each through the daemon (`PublishedFile.Vote`, `GetUserVoteSummary`, `Subscribe` with the favourites list, `GetAppRelationships`, `Workshop.GetEULAStatus`) |
| `ShowWorkshopEULA` | Opens the Workshop agreement in the browser (no overlay exists). True when it was opened |
| `GetUserContentDescriptorPreferences` | The account's content preferences from Steam. *Unverified:* the request (§10). Fails until it is known |
| `SetItemsDisabledLocally`, `SetSubscriptionsLoadOrder` | §4 |
| Publishing: `CreateItem`, `StartItemUpdate`, the `SetItem…` setters, `SubmitItemUpdate`, `GetItemUpdateProgress`, `DeleteItem`, the dependency methods | §8. Fail under ABI-3 until it is built |
| `BInitWorkshopForGameServer` | Game servers are not in M3. Fails |

The callbacks are those of `isteamugc.h` (`k_iSteamUGCCallbacks` + 1 to
+20; +19 is unused), with layouts from the SDK version the game uses
(`steamworks-abi.md` §2.4).

### 7.1 `ISteamRemoteStorage`

`isteamremotestorage.md` leaves its UGC and Workshop methods to M3. The
older Workshop methods (`EnumerateUserSubscribedFiles`,
`GetPublishedFileDetails`, `SubscribePublishedFile`,
`UnsubscribePublishedFile`) map to the same requests as their
`ISteamUGC` counterparts. `UGCDownload`, `GetUGCDetails` and `UGCRead`
download a legacy item's file, or a leaderboard's attached file, by its
UGC handle: `ClientUFSGetUGCDetails` (EMsg 5226/5227) gives the URL. The
message's fields are no longer in the current dumps, so they are
*unverified*. The legacy publishing methods (`PublishWorkshopFile`,
`FileShare`, `UpdatePublishedFile…`) are §8.

## 8. Publishing

Publishing creates an item (`CreateItem` → `PublishedFile.Publish` or
`ClientUCMPublishFile` 7311) and uploads its content. The dumps name the
upload services but not the order: depot-based content most likely goes
through `ContentBuilder.StartDepotUpload` / `FinishDepotUpload` (the
SteamPipe builder's service), and previews and legacy files through
`Cloud.BeginUGCUpload` / `CommitUGCUpload`. Nothing on the development
computer shows an upload, and none can be observed without publishing
something publicly or privately from an account.

This page therefore proposes to leave publishing out of M3's first step
and to write it up separately once an upload has been observed (§11 Q2).
Until then every publishing method fails under ABI-3, so a game that
offers uploading says it failed instead of seeming to succeed.

## 9. Out of scope

- Browsing and searching the Workshop in ossteam's GUI. The website does
  this, and a game's own browser uses the query methods (§7).
- Game servers (`BInitWorkshopForGameServer`, `steamcmd
  +workshop_download_item`).
- Items of non-Steam shortcuts (`consumer_shortcutid`).
- Workshop items that are sold (`Microtransaction`, `for_sale_data`).

## 10. Investigation before code

1. **Spacewar (480).** Its app info names a workshop depot, and the test
   account owns it. Its Workshop is used by Steamworks developers for
   testing, so it is the likely test title. Whether it has items that
   anyone can subscribe to is still to be checked on the website.
2. **The official client subscribing** on the test account, with its
   `workshop_log.txt`, `content_log.txt` and `appworkshop_480.acf`
   compared before and after, and a CM capture if the user agrees: which
   request lists the subscriptions (§2.2), `list_type` (§6), what
   `GetItemChanges`' `Ignored` means (§2.4), and whether a game waits for
   item updates before it starts (§3.4).
3. **Unsubscribing** while the game runs and after, to see when files go
   (§3.4), and a **cached item** that the game asks for, to see whether it
   is ever evicted (§4).
4. **Uninstalling the app**, to see whether its items go with it.
5. **The content preferences request** (§7): whatever the official client
   sends when the Store's content settings are opened.

Requests that ossteam sends for 1 to 5 come from ossteam with the user
watching. Nothing here is sent before the user has approved it,
subscribing included, since a subscription is visible on the account's
profile.

## 11. Questions for review

1. **The layout** (§3.2): `<library>/workshop/content/<app>/<item>/`
   beside `common/`, so that relative paths that work with the official
   client work with ossteam? Or under `.ossteam/`, which keeps ossteam's
   own files out of the library's top level?

   *Answer (2026-11-03):* neither. Every path is ossteam's own, new and clean, in
   the platform-standard directories; nothing mirrors the official layout
   (§3.2).

2. **Publishing** (§8): leave it out of M3's first step and write it up
   once an upload has been observed, as proposed? It needs an item
   published from the test account, at least privately.

   *Answer (2026-11-03):* as proposed.

3. **Updates while a game runs** (§3.4): defer subscribed items' updates
   until the game exits and commit `DownloadItem` at once, as proposed?

   *Answer (2026-11-03):* as proposed.

4. **Cached items** (§4): keep them until the game marks them unused or
   the app is uninstalled, until the official client's eviction is known?

   *Answer (2026-11-03):* as proposed, and the cache's size is adjustable
   (`workshop.cache_limit`, §4).

5. **The CLI** (§5): `ossteam workshop <APP>` with `subscribe`,
   `unsubscribe` and `update`?

   *Answer (2026-11-03):* as proposed.

## 12. Verification against the official client

| Item | Status |
|---|---|
| Message fields (PublishedFile, PublishedFileClient, UCM 7378/7379/7381, Workshop.GetEULAStatus) | from the client's message dumps |
| `workshopdepot` = the app's own id | 6 cached apps |
| Item content is a manifest in the workshop depot | DepotDownloader; the official client's manifest URL for depot 730 |
| Manifest request code for item manifests | the official client's manifest URL |
| Depot key for the workshop depot, chunks encrypted | unverified |
| Layout under `steamapps/workshop/` and the `.acf` keys | the official client's files |
| Which request lists subscriptions (7378 or `GetUserFiles`) | unverified (§10) |
| `GetItemChanges` and `GetItemInfo` behind the log lines | inferred from the log; unverified |
| `list_type` for subscribing | unverified |
| `DownloadItem` commits while the game runs | the official client's log |
| Deferred updates, unsubscribe timing, eviction, uninstall | unverified |
| `ClientUFSGetUGCDetails` fields | not in the dumps; unverified |
| Publishing | unverified (§8) |
| `ISteamUGC` methods, callbacks and `EItemState` | SDK headers |
