---
name: m3-offline
description: M3 offline - offline.md drafted (37f78f3); §10 answered 2026-11-03 as proposed but Q3 (offline logon ticket) open; a daemon started offline has an empty library today
metadata:
  type: project
---

offline.md is drafted (37f78f3). Gate: §10 and §9
([spec-review-gates](spec-review-gates.md)).

- Gap today: licenses are held in memory only, so a daemon started
  offline has an empty library.
- The official client uses loginusers.vdf
  WantsOfflineMode/SkipOfflineModeWarning and a localconfig.vdf
  `Offline` block (Ticket 11 B, Signature 256 B, EncryptedTicket 317 B,
  EncryptedSignature 256 B) from `Offline.GetOfflineLogonTicket`, which
  ossteam proposes not to use.
- steamui_login.txt shows a failed logon leading to "Start offline";
  `Player.RecordDisconnectedPlaytime` records offline playtime; appcache
  and stats are kept.
- Proposes a stored license list (changes pics.md §6), an offline stats
  merge (changes isteamuserstats.md) and a SetOfflineMode IPC.
- §10 answered on 2026-11-03, as proposed: serve the stored library and
  keep trying, the license list in the database, timed trials refused
  offline, offline stats merged at the next logon. Q3 (the offline logon
  ticket) is open.
