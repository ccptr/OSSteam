---
name: m3-friends
description: M3 friends - friends.md drafted (00f4d0d); §11 answered 2026-11-03 as recommended; §10 observations need a second test account the maintainer has yet to make
metadata:
  type: project
---

friends.md is drafted (00f4d0d). Gate: §11 and §10, which need the
account logged in plus a second account
([spec-review-gates](spec-review-gates.md)).

- Friends list 767 (unrequested after logon);
  Player.AddFriend/RemoveFriend/IgnoreFriend; nicknames 5587/5588;
  categories via FriendsList.GetCategories.
- ClientPersonaState 766 status_flags table (the split is partly
  unverified); ClientRequestFriendData 815.
- Rich presence 7501–7503 plus
  Community.GetAppRichPresenceLocalization.
- Chat via the FriendMessages service (community clients send logon
  `chat_mode` 2; ossteam sends none today); text filtering follows
  Player.GetCommunityPreferences.
- The official client stores coplay locally
  (`config/coplay_<steamid>.vdf`).
- Proposes in-memory-only friends data and no chat log on disk. §11
  answered on 2026-11-03, all as recommended: a second test account for
  §10 (not made yet; ask the maintainer), group chat after M3, no chat
  history on disk, text filtering per the community setting, and the
  three CLI commands.
