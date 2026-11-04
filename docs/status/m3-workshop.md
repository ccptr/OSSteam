---
name: m3-workshop
description: M3 workshop - workshop.md drafted (0aa5bfc); item content and layout; gated on §11 and §10 (subscribing is visible)
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
- Proposes `<library>/workshop/content/<app>/<item>` beside `common/`;
  publishing is left out (no upload observed). §11 asks 5 questions.
