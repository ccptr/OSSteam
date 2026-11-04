---
name: m3-cloud
description: M3 cloud - cloud.md drafted (fd7c74d); §9 answered 2026-11-03 with Q1 (~/.SteamCloud) left until a game uses that root; needs the §8 live observations
metadata:
  type: project
---

cloud.md is drafted (fd7c74d). Gate: §9 answers and the §8 observations
([spec-review-gates](spec-review-gates.md)).

- Sources: SteamDatabase protobufs (Cloud.* service), Valve's Steam
  Cloud docs, the official client's `logs/cloud_log.txt` and
  `userdata/<id>/<app>/remotecache.vdf`, the community root-number table
  (SteamCloudFileManager ROOT_PATH_MAPPING.md, facts only), `ufs` keys
  in `~/.cache/ossteam/pics/app/*.vdf` (binary KV).
- Spacewar 480 has quota 4096 / 10 files: the API test title.
- Open: compressed (file_size < raw) and encrypted downloads, Auto-Cloud
  names on Steam (path prefixes), Proton roots and platform change, the
  conflict rule, client_id.
- §9 answered on 2026-11-03: writing to the test account's cloud is
  acceptable; `ossteam launch` asks on the terminal when it is
  interactive, else fails and a command resolves; M3 order is cloud,
  friends and chat, offline, Workshop, betas. Q1 (`~/.SteamCloud`):
  left until a game is seen that uses it; until then that root matches
  nothing on Linux.
