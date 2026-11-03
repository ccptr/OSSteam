---
name: m3-offline
description: M3 offline - offline.md drafted (1e770e4); a daemon started offline has an empty library today; gated on §10 and §9
metadata:
  type: project
---

offline.md is drafted (1e770e4). Gate: §10 and §9
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
