# Friends and chat

| | |
|---|---|
| Status | Draft for M3; awaiting review. Nothing has been built, and §10 lists what must be observed first |
| Requirements | SPEC M3, §9.4 (PLAY-2), §12.3, FAITH-1 to FAITH-4, SEC-4, SEC-6 |
| Code | none yet. M1's own persona is in `crates/ossteamd/src/session/worker.rs` (`ClientChangeStatus`) and `crates/ossteam-steamclient/src/friends.rs` (`steamworks/isteamfriends.md`) |

SPEC M3 asks for "the friends list and chat (ISteamFriends in full)".
This page describes what Steam tells a client about the account's
friends, how a client changes the list, how one-to-one chat works, and
what of this a game can see through `ISteamFriends`. It also says how
ossteam would do the same, and what it would leave out.

**Verification.** This draft draws on the following sources:

- the message definitions that SteamDatabase extracts from the official
  client (`steammessages_clientserver_friends.proto`,
  `steammessages_clientserver_2.proto`,
  `steammessages_friendmessages.steamclient.proto`,
  `steammessages_chat.steamclient.proto`,
  `steammessages_player.steamclient.proto`,
  `steammessages_community.steamclient.proto`, `webui/service_friendslist.proto`,
  `enums_clientserver.proto`);
- SteamKit2's `SteamFriends` handler and its enum tables, for the order
  of the logon exchanges and the enum values. Facts only, as with every
  community client;
- the Steamworks SDK headers that Proton ships (`lsteamclient/steamworks_sdk_165/isteamfriends.h`):
  the methods, callbacks, limits and the comments on them;
- the official Linux client's logs (`webhelper.txt`, `gameprocess_log.txt`,
  `text_filter_log.txt`) and `config/coplay_<steam id>.vdf` on the
  development computer. Only their structure is used here, never their
  values.

The official client's friends window is a web page (`friendsui` in the
logs) that talks to Steam through the client. None of its traffic is
in the logs, so the order of its calls is *unverified* throughout, and
§10 lists the captures that would settle it.

## 1. The friends list

### 1.1 Relationships

Steam keeps, for the account, a list of other accounts and groups
("clans") with a relationship to each. The numbers are
`EFriendRelationship`:

| Value | Name | Meaning |
|---|---|---|
| 0 | None | no relationship; in an update, the entry was removed |
| 1 | Blocked | the account blocked this user |
| 2 | RequestRecipient | this user asked to be friends; the account has not answered |
| 3 | Friend | friends |
| 4 | RequestInitiator | the account asked; the other user has not answered |
| 5 | Ignored | *unverified:* a request from this user that the account ignored |
| 6 | IgnoredFriend | *unverified:* a friend whose messages the account ignores |

For a group (a Steam id of account type 7, Clan), the same field holds
an `EClanRelationship`. Groups are §6.

### 1.2 Receiving the list

After logon, the CM sends `ClientFriendsList` (767) on its own, with
`bincremental` false: the whole list. Later changes come as messages
with `bincremental` true that carry only the entries that changed; an
entry with relationship 0 is gone. `max_friend_count`,
`active_friend_count` and `friends_limit_hit` describe the account's
limit. `FriendsList.GetFriendsList#1` returns the same message on
request; the friends window may use it instead (*unverified*).

The list carries Steam ids and relationships only. Names, states and
games come separately (§2).

### 1.3 Changing the list

| Action | Message | Reply |
|---|---|---|
| Ask to be friends, or accept a request | `Player.AddFriend#1`: `steamid` | `invite_sent`, `friend_relationship` (the new one), `result` (an EResult) |
| Remove a friend, cancel a request, or decline one | `Player.RemoveFriend#1`: `steamid` | `friend_relationship` |
| Block or unblock | `Player.IgnoreFriend#1`: `steamid`, `unignore` | `friend_relationship` |

SteamKit2 does the same with older messages: `ClientAddFriend` (791) with a
Steam id or an account name, answered by `ClientAddFriendResponse` (792), and
`ClientRemoveFriend` (714). Accepting a request is adding the user who
sent it, and declining it is removing them, in both forms. Either way the
CM also sends an incremental `ClientFriendsList` with the new
relationship, and ossteam takes the list from that message, not from the
reply. *Unverified:* which form the official client uses. ossteam would
use the `Player` methods, which carry an EResult. Adding by account name
or e-mail exists only in the older message and needs no support: the
official client adds through invite links and profiles.

### 1.4 Nicknames, categories and favourites

- **Nicknames.** The account can give a friend a name that only it sees.
  The CM sends `ClientPlayerNicknameList` (5587) after logon (whole list,
  then incremental changes, `removal` for a removed name).
  `Player.GetNicknameList#1` returns the list, and the CM sends the
  `PlayerClient.NotifyFriendNicknameChanged#1` notification when a
  nickname changes on another session. A nickname is set with
  `AMClientSetPlayerNickname` (5588), answered by 5589; an empty name
  removes it. *Unverified:* that the official client still uses 5588.
- **Categories.** The account's own groupings of friends:
  `FriendsList.GetCategories#1` returns `groupid`, `name` and the members'
  account ids. The CM also sends `ClientFriendsGroupsList` (5553) with the
  same facts in older field names. The SDK limits them to 100
  (`k_cFriendsGroupLimit`) with names up to 64 bytes. M3 reads them;
  changing them is out of scope (§9).
- **Favourites.** `FriendsList.GetFavorites#1` and `SetFavorites#1`: a
  list of friends, groups and group chats that the friends window shows
  first. Read only in M3, like categories.

## 2. Personas

### 2.1 What Steam sends

`ClientPersonaState` (766) carries the persona of one or more accounts.
Its `status_flags` (`EClientPersonaStateFlag`) say which fields of each
entry are filled in; a field outside them is not a change and must not
overwrite what ossteam knows:

| Bit | Name | Fields (from SteamKit2's handler and the field names) |
|---|---|---|
| `0x0001` | Status | `persona_state`, `persona_state_flags` |
| `0x0002` | PlayerName | `player_name` |
| `0x0004` | QueryPort | `query_port` |
| `0x0008` | SourceID | `steamid_source` |
| `0x0010` | Presence | `avatar_hash`, `persona_state`, `persona_state_flags` |
| `0x0040` | LastSeen | `last_logoff`, `last_logon`, `last_seen_online` |
| `0x0080` | UserClanRank | `clan_rank` |
| `0x0100` | GameExtraInfo | `game_name`, `gameid`, `game_played_app_id` |
| `0x0200` | GameDataBlob | `game_data_blob`, `game_server_ip`, `game_server_port` |
| `0x0400` | ClanData | `clan_data`, `clan_tag` |
| `0x1000` | RichPresence | `rich_presence` (§3) |
| `0x2000` | Broadcast | `broadcast_id` |
| `0x4000` | Watching | `watching_broadcast_*` |

*Unverified:* the exact split between Status and Presence, and which
bit covers `on_steam_deck`, `gaming_device_type` and `other_game_data`.
§10 checks it against the messages the test account receives.

The CM sends personas for friends that are online, unasked. For the
others, the client asks: SteamKit2 sends `ClientRequestFriendData` (815)
with every Steam id of a full friends list and the flags it wants, and
the CM answers with `ClientPersonaState`. `Chat.RequestFriendPersonaStates#1`
(no fields) exists too, and is probably the friends window's way of
asking for all of them (*unverified*). ossteam would request what it
needs: name, status, presence, game and rich presence for friends;
name and presence for anyone else a game or the user asks about (§7).

`persona_state` is an `EPersonaState`:

| Value | Name |
|---|---|
| 0 | Offline |
| 1 | Online |
| 2 | Busy |
| 3 | Away |
| 4 | Snooze |
| 5 | LookingToTrade |
| 6 | LookingToPlay |
| 7 | Invisible |

A friend who is invisible is sent as offline (*unverified*).
`persona_state_flags` (`EPersonaStateFlag`) adds facts about the session:
`0x1` has rich presence, `0x2` in a joinable game, `0x4` golden, `0x8`
Remote Play Together, `0x100` web client, `0x200` mobile, `0x400` Big
Picture, `0x800` VR, `0x1000` launched with a gamepad, `0x2000` launched
through a compatibility tool.

### 2.2 Avatars

`avatar_hash` is 20 bytes. Community clients turn it into an image URL
on `avatars.steamstatic.com`: the hash in lowercase hex followed by
`.jpg` (32 px), `_medium.jpg` (64 px) or `_full.jpg` (184 px).
*Unverified* against the official client, which keeps no avatar cache on
the development computer to compare with. An all-zero hash means the
default avatar. ossteam would fetch avatars through the same bounded
cache as the GUI's artwork (`gui.md`), and decode them for games as it
already decodes achievement icons (`steamworks/isteamuserstats.md` §1.3).

### 2.3 Steam level

`ClientFSGetFriendsSteamLevels` (7528) takes account ids and is
answered by 7529 with a `level` for each. ossteam asks for it only when
a game calls `GetFriendSteamLevel`, or the GUI shows a profile.

### 2.4 The account's own persona

M1 sends `ClientChangeStatus` (716) with `persona_state` and
`persona_set_by_user` (PLAY-2, `pics.md` §8). M3 would add:

- **More states.** The user may also choose Away or Busy. The official
  client's menu offers Online, Away, Invisible and Offline, and a
  separate "Do not disturb" (*unverified:* whether that reaches Steam,
  or only silences the client). ossteam offers the states that reach
  Steam, and no idle detection: `is_client_idle` stays false, because
  finding out whether the user is idle differs on every desktop and is
  not needed to be faithful. Away is the user's choice.
- **Renaming.** `ClientChangeStatus` with `player_name`, then
  `ClientPersonaChangeResponse` (5584) answers with an EResult and the
  name Steam kept. SteamKit2 sends the current state with the new name.
  *Unverified:* whether the reply needs `need_persona_response`, which
  SteamKit2 leaves unset. The SDK caps names at 128
  bytes of UTF-8 or 32 characters (`k_cchPersonaNameMax`,
  `k_cwchPersonaNameMax`).
- **Flags while a game runs.** The official client's
  `gameprocess_log.txt` prints "SSGL: persona state flags" several times
  per game session, which suggests it sends `persona_state_flags` when a
  game starts, joins a server, and exits. ossteam sends none yet. §10
  captures what the official client sends.

## 3. Rich presence

A game sets key/value pairs that friends see: what the player is doing,
and how to join them.

- **Upload.** `ClientRichPresenceUpload` (7501): `rich_presence_kv` is
  binary KeyValues (`keyvalues.md`) with a root holding the pairs,
  and `steamid_broadcast` lists accounts to send it to beyond friends
  (*unverified:* who the official client lists there; probably the
  players of the game's lobby or server). The upload belongs to the
  game's session. An empty set clears it, and ossteam clears it when the
  game exits. *Unverified:* the root's name.
- **Download.** Friends' rich presence arrives in `ClientPersonaState`
  (bit `0x1000`). For other accounts, `ClientRichPresenceRequest` (7502)
  is answered by `ClientRichPresenceInfo` (7503).
- **Limits** (SDK): 30 keys, keys up to 64 bytes, values up to 256
  bytes. `SetRichPresence` with a key past those limits fails.
- **Special keys** (SDK comments): `status` (shown in the friends
  list), `connect` (the string a friend's game gets when joining),
  `steam_display` (a localisation token, below), `steam_player_group`
  and `steam_player_group_size`.
- **Localisation.** `steam_display` names a token, such as
  `#Status_InMatch`, whose text and `%key%` substitutions come from the
  app's rich presence localisation: `Community.GetAppRichPresenceLocalization#1`
  with the app id and a language returns token lists per language.
  ossteam shows the substituted text, falling back to `status`, then to
  the game's name. *Unverified:* the substitution rules beyond `%key%`.

## 4. One-to-one chat

### 4.1 Two generations

Steam has two ways to send a friend a message:

- the older **`ClientFriendMsg`** (718) out and **`ClientFriendMsgIncoming`**
  (5427) in, with `ClientFriendMsgEchoToSender` (5578) copying a message
  to the sender's other sessions. SteamKit2 uses these;
- the **`FriendMessages`** service, which the friends window's names
  point to: `FriendMessages.SendMessage#1` out, and the
  `FriendMessagesClient.IncomingMessage#1` notification in, as a
  `ServiceMethod` (146) message from the CM.

`CMsgClientLogon` has a `chat_mode` field, and `CMsgClientUIMode` a
`chat_mode` too. Community clients set `chat_mode` 2 at logon to receive
messages as the newer notifications. *Unverified:* the official
client's value, and whether a session with mode 0 still receives the
older messages. ossteam sends no `chat_mode` today (`ossteam-auth`'s
logon). ossteam would use the newer service throughout, once §10 has
confirmed what the official client sends at logon.

### 4.2 Sending

`FriendMessages.SendMessage#1` with:

- `steamid`: the friend;
- `chat_entry_type`: an `EChatEntryType`, 1 for a message, 2 for
  "typing" (no text);
- `message`, and `contains_bbcode` (§4.4);
- `echo_to_sender` true, so the account's other sessions see the
  message (*unverified*: the official client's value);
- `client_message_id`: an id the sender chooses, presumably to match
  echoes (*unverified*).

The reply gives `server_timestamp` and `ordinal`, which together
identify the message (two messages in the same second differ in
ordinal), and `modified_message` and `message_without_bb_code`, the text
as Steam stored it. ossteam shows what Steam stored, not what was typed.
A failure is an EResult in the reply's header: a user that is not a
friend, a limited account, or rate limiting. The user sees it.

### 4.3 Receiving

`FriendMessagesClient.IncomingMessage#1`: `steamid_friend`,
`chat_entry_type`, `message`, `message_no_bbcode`,
`rtime32_server_timestamp`, `ordinal`, `from_limited_account`,
`low_priority`, and `local_echo`, true when the account itself sent the
message from another session. Typing notices come as entry type 2.

`FriendMessages.AckMessage#1` with the friend and a timestamp marks the
conversation read up to then. The CM echoes acks to the account's other
sessions as `FriendMessagesClient.NotifyAckMessageEcho#1`, so a message
read on one computer stops being unread on the others.

### 4.4 Text

Messages carry BBCode: `[emoticon]`, `[sticker]`, `[url]`, `[spoiler]`,
`[code]` and others (*unverified:* the full set, which the friends window
renders). ossteam would render a small known set and show the rest as
the `no_bbcode` text Steam sends along.

**Text filtering.** The official client hides profanity and slurs in
others' messages by default. What it hides is the account's community
preference: `Player.GetCommunityPreferences#1` returns
`text_filter_setting` (`ETextFilterSetting`: 0 not opted in, 1 on, 2 on
but allowing profanity, 3 off) and `text_filter_ignore_friends`.
`text_filter_log.txt` shows it downloads word lists from
`steamcommunity.com/textfilter/gettextfilterdictionary` (`type` =
`profanity` and `banned`, with a language). `Player.GetTextFilterWords#1`
returns the account's own banned and allowed words. *Unverified:* how
the words are matched. Faithful means filtering as the account's setting
says, so ossteam would follow it, and the same filter would answer
`ISteamUtils::FilterText` (`steamworks/isteamutils.md`, which fails it in
M1).

### 4.5 History and unread messages

Steam keeps conversation history on its servers:

- `FriendMessages.GetActiveMessageSessions#1` lists conversations with
  `last_message`, `last_view` and `unread_message_count`. It is how a
  client learns, after logon, that messages arrived while it was away.
- `FriendMessages.GetRecentMessages#1` with both Steam ids returns a
  page of messages (`accountid`, `timestamp`, `ordinal`, `message`,
  reactions), newest first (*unverified*), with `more_available`, and
  takes `time_last` and `ordinal_last` to fetch the next page.

The official client keeps no chat log on the development computer that
we know of. ossteam would keep none either: history is fetched from
Steam when a conversation opens, and lives in the daemon's memory only
while the session lasts (§8).

### 4.6 Left out

Reactions (`UpdateMessageReaction`, `MessageReaction` notifications) are
shown when they arrive but not added in M3. Reporting a message
(`ReportMessage`) is out of scope for M3, and session notices
(`SessionNotice`, "suspicious" conversations) are shown as text.

## 5. Group chat

The `ChatRoom` service (about 60 methods) carries the official client's
group chats: chat room groups with text and voice rooms, roles,
invites, bans and member lists, plus a `ChatRoomClient` notification
service. Text rooms work like §4 with a group and room id
(`SendChatMessage`, `GetMessageHistory`, `NotifyIncomingChatMessage`,
`AckChatMessage`). Voice rooms use WebRTC and are out of scope.

This page specifies no more than that. Group chat is a second, larger
piece than the friends list, and §11 asks whether M3 needs it.

## 6. Groups (clans)

Groups appear in the friends list (§1.1), and their names, avatars and
tags in `ClientPersonaState`. `ClientClanState` (822) carries a group's
member counts, events and announcements. *Unverified:* its fields and
when it is sent. M3 would show the account's groups by name only, and
answer the `ISteamFriends` clan methods that need only the list, names
and tags (§7). Officer lists, activity counts and clan chat rooms come
later in M3, with group chat (§5).

## 7. `ISteamFriends`

### 7.1 What the shim can answer

The daemon keeps the friends list, the personas and rich presence it
has received, and sends the shim what a game asks for. The table groups
the 78 methods of `SteamFriends018` (`vtables.txt`). Earlier versions
are subsets with the same meaning.

| Methods | With M3 |
|---|---|
| `GetPersonaName`, `GetPersonaState` | as in M1 |
| `GetFriendCount`, `GetFriendByIndex`, `HasFriend`, `GetFriendRelationship` | the friends list, filtered by `EFriendFlags` (`0x4` immediate = friends, `0x1` blocked, `0x2` and `0x80` requests, `0x8` group members, `0xFFFF` all) |
| `GetFriendPersonaState`, `GetFriendPersonaName`, `GetFriendGamePlayed` | the persona the daemon has. `GetFriendGamePlayed` fills `FriendGameInfo_t` from the game id, server address, query port and lobby id, and returns false when the friend is not in a game |
| `GetPlayerNickname` | the nickname (§1.4), or null |
| `GetFriendPersonaNameHistory` | the current name at index 0; earlier names are not known, and return an empty string (*unverified:* where the official client gets them) |
| `GetFriendSteamLevel` | the level, after asking Steam once (§2.3); 0 until it arrives |
| `GetFriendsGroupCount`, `GetFriendsGroupIDByIndex`, `GetFriendsGroupName`, `GetFriendsGroupMembersCount`, `GetFriendsGroupMembersList` | the categories (§1.4) |
| `GetSmallFriendAvatar`, `GetMediumFriendAvatar`, `GetLargeFriendAvatar` | an image handle for `ISteamUtils::GetImageRGBA`, or 0 when there is none. The SDK lets only the large one return -1 while it loads, followed by `AvatarImageLoaded_t` (334); the small and medium ones return 0 until `PersonaStateChange_t` reports the avatar, so the daemon fetches those two with the persona |
| `RequestUserInformation` | false when the name (and the avatar unless `bRequireNameOnly`) is known; otherwise true, and `PersonaStateChange_t` (304) follows |
| `SetRichPresence`, `ClearRichPresence` | the game's rich presence (§3) |
| `GetFriendRichPresence`, `GetFriendRichPresenceKeyCount`, `GetFriendRichPresenceKeyByIndex`, `RequestFriendRichPresence` | friends' rich presence; `FriendRichPresenceUpdate_t` (336) when it changes |
| `InviteUserToGame` | a message of `EChatEntryType` 3 (InviteGame) with the connect string (*unverified*); the friend's client then starts or tells the game, which gets `GameRichPresenceJoinRequested_t` (337) |
| `SetPlayedWith`, `GetCoplayFriendCount`, `GetCoplayFriend`, `GetFriendCoplayTime`, `GetFriendCoplayGame` | "recently played with". The official client keeps these in `config/coplay_<steam id>.vdf` (binary KV, `coplay/<n>` with `SteamID`, `gameid` and `Playtime`), so they are local. ossteam would keep them in its state database. *Unverified:* whether anything is sent to Steam |
| `GetClanCount`, `GetClanByIndex`, `GetClanName`, `GetClanTag`, `IsClanPublic`, `IsClanOfficialGameGroup` | the groups (§6) as far as the persona carries them; the last two return false until their source is known |
| `GetFriendCountFromSource`, `GetFriendFromSourceByIndex`, `IsUserInSource` | friends whose `steamid_source` is the given lobby or server |
| `SetListenForFriendsMessages`, `ReplyToFriendMessage`, `GetFriendMessage` | the game receives one-to-one messages (§4) as `GameConnectedFriendChatMsg_t` (343) while listening, and replies through §4.2 |
| `GetNumChatsWithUnreadPriorityMessages` | conversations with unread messages that are not `low_priority` |
| `SetInGameVoiceSpeaking` | does nothing: it only feeds the official client's UI |
| `ActivateGameOverlay*`, `RegisterProtocolInOverlayBrowser` | do nothing, as in M1: there is no overlay (SPEC "Later"), and `IsOverlayEnabled` is false |
| clan officer lists, activity counts, clan chat rooms, followers (`GetFollowerCount`, `IsFollowing`, `EnumerateFollowingList`: `ClientFSGetFollowerCount` 7515 and its neighbours), equipped profile items (`Player.GetProfileItemsEquipped#1`) | fail under ABI-3 until the later part of M3 (§5, §6) |

### 7.2 Callbacks

`PersonaStateChange_t` (304) carries a Steam id and `EPersonaChange`
bits (`0x1` name, `0x2` status, `0x4` came online, `0x8` went offline,
`0x10` game, `0x20` server, `0x40` avatar, `0x200` relationship,
`0x1000` nickname, `0x2000` level, `0x4000` rich presence, and others).
The SDK says a game sees one for every friend at startup. The daemon
works the bits out by comparing the persona before and after each
update, and sends the shim one change per user. The shim posts these
like its other callbacks (`steamworks-abi.md` §2.5).

### 7.3 Game IPC

Protocol 5 would add:

- requests for the friends list, a persona, rich presence of a user,
  an avatar (RGBA, as achievement icons), the Steam level, the
  categories and the coplay list, and for setting rich presence,
  inviting, setting played-with and sending a message;
- a `Friends` update from the daemon with what changed (users and
  `EPersonaChange` bits, rich presence, incoming messages while the game
  listens).

The daemon sends a game only what concerns the account and the users it
asked about. A game that never calls `ISteamFriends` gets no updates.

## 8. Privacy and the daemon

Friends' names, states, games and messages are other people's data.
SPEC §18 has no rule for it yet; this page proposes one. The daemon:

- keeps the friends list, personas, rich presence and history in memory
  only, and drops them at logout or account switch;
- stores only the account's own choices: its persona state (M1), and
  the coplay list (§7.1), which the official client also stores;
- never logs message text, names or rich presence values: logs name
  Steam ids and message kinds only;
- sends clients the friends list only after they subscribe to a new
  `FRIENDS` topic, as for every other topic (`ipc.md` §4).

## 9. Out of scope

- Voice chat, in group rooms and one-to-one.
- Changing categories and favourites, managing groups, and invite links.
- Reporting messages and users.
- Desktop notifications for messages. *Unverified* whether the GUI
  should have them in M3; they would use each platform's notification
  service, outside the protocol.
- Idle detection (§2.4).
- Trading, the Steam Community website, and profiles beyond name,
  avatar, level and status. `ClientFriendProfileInfo` (5535) exists
  for real name, location and summary; the GUI does not show them.

## 10. Investigation before code

All of these need the test account logged in, and a second account to
be its friend (§11).

1. **The logon exchange.** With the official client on the test
   account: the `chat_mode` it sends, whether `ClientFriendsList`,
   `ClientPlayerNicknameList` and `ClientFriendsGroupsList` arrive
   unasked, and how it asks for personas (`ClientRequestFriendData` or
   `Chat.RequestFriendPersonaStates`) and with which flags.
2. **Persona fields.** Which `status_flags` bits arrive with which
   fields (§2.1), and how an invisible friend appears.
3. **A message each way** between the two accounts, with the official
   client on one side: which messages carry it, the echo to a second
   session, the ack, and what typing looks like.
4. **A game session** with the official client: the persona state flags
   it sends (§2.4), and Spacewar's (480) rich presence upload (§3).
5. **`InviteUserToGame`** from a game on one account to the other.
6. **Avatar URLs** (§2.2), against the images the official friends
   window loads (`webhelper.txt` names the URLs it fetches, if its
   logging is turned up).

The fake CM gains a friends service for the tests: a second account
whose persona, messages and rich presence the test controls.

## 11. Questions for review

*Answer (2026-11-03):* all five as recommended: a second test
account for §10, group chat after M3, no chat history on disk, text
filtering as the account's community setting says, and all three CLI
commands.

1. **A second account.** §10 needs the test account to be friends with
   another account. Your own account, a second test account you create,
   or none (then chat is only tested against the fake CM)? Recommended:
   a second test account, so that nothing touches your own friends list.
2. **Group chat** (§5): in M3, after the friends list and one-to-one
   chat, or later? Recommended: later. M3's "chat" would then mean
   one-to-one chat, and the clan chat methods of `ISteamFriends` fail
   until group chat is done.
3. **Chat history on disk** (§4.5): none, as recommended, so that every
   computer shows what Steam has? The alternative is a local cache for
   offline reading, which the official client does not seem to have.
4. **Text filtering** (§4.4): follow the account's community setting,
   as recommended, including downloading Steam's word lists? Leaving it
   out would show messages that the official client hides from this
   user.
5. **The CLI.** `ossteam friends` (list), `ossteam chat <friend>` (an
   interactive conversation on the terminal), `ossteam friend
   add|remove|block <friend>`? Recommended: all three, with `chat` the
   only interactive command besides `login`.

## 12. Verification against the official client

| Item | Status |
|---|---|
| Message and service fields | from the client's message dumps |
| EMsg numbers | `enums_clientserver.proto` |
| `EFriendRelationship`, `EPersonaState`, `EPersonaStateFlag`, `EClientPersonaStateFlag`, `EChatEntryType`, `EFriendFlags` values | SteamKit2's enum tables; not yet seen in a capture |
| `ISteamFriends` methods, callbacks, `EPersonaChange`, limits | SDK headers |
| Which `status_flags` bit carries which field | SteamKit2, partly; unverified |
| Logon order: friends list unasked, personas requested | SteamKit2; unverified for the official client |
| `chat_mode` and which chat messages the official client uses | unverified |
| Adding, removing and blocking through `Player` methods | unverified |
| Avatar URLs | community clients; unverified |
| Rich presence upload format and localisation substitutions | unverified |
| Coplay kept locally | from the official client's `coplay_*.vdf` |
| Text filter word lists and setting | from `text_filter_log.txt` and the message dumps; matching unverified |
| Persona state flags during a game | from `gameprocess_log.txt` that they are sent; values unknown |
