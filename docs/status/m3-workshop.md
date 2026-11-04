---
name: m3-workshop
description: M3 workshop - workshop.md drafted (0aa5bfc); §11 answered 2026-11-03 (own clean paths, no official layout; adjustable cache size); needs §10 (subscribing is visible)
metadata:
  type: project
---

workshop.md is drafted (0aa5bfc). Gate: §11 and §10; subscribing shows on
the profile, so ask first ([spec-review-gates](spec-review-gates.md)).

- `depots/workshopdepot` is the app's own id (6 cached apps, including
  Spacewar 480).
- Item content is the manifest `hcontent_file` in that depot (normal
  key, request code and CDN path); legacy items use `file_url`.
- The official client keeps
  `steamapps/workshop/{appworkshop_<app>.acf,content/<app>/<item>}` and
  `logs/workshop_log.txt` (daily subscription list, GetItemInfo full
  list, DownloadItem commits while the game runs).
- §11 answered on 2026-11-03: every path is ossteam's own, new and
  clean, nothing mirrors the official layout; items are in
  `<library>/workshop/<app>/<item>/`. Cached items are bounded by
  `workshop.cache_limit` (0 = no limit). Publishing is left out until an
  upload has been observed; the rest as proposed.
