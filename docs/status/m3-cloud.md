---
name: m3-cloud
description: M3 cloud - cloud.md drafted (ade70b9); gated on §9 answers and §8 live observations
metadata:
  type: project
---

cloud.md is drafted (ade70b9). Gate: §9 answers and the §8 observations
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
- §9 asks 4 questions: SteamCloudDocuments `~/.SteamCloud` vs DIRS-1,
  writing to the test account's cloud, CLI conflict UX, M3 order.
