---
name: m3-betas
description: M3 betas - betas.md drafted (360123c); beta password flow, key storage proposal; gated on §9 and §8
metadata:
  type: project
---

betas.md is drafted (360123c). Gate: §9 and §8, which needs a published
beta code ([spec-review-gates](spec-review-gates.md)).

- Today's PICS has no `pwdrequired` or `encryptedmanifests` (0 of 201
  cached apps); 32 apps carry `depots/privatebranches = 1` and list no
  private branch names.
- Flow: ClientCheckAppBetaPassword 5450/5451 (code → beta name plus hex
  key) → ClientPICSPrivateBetaRequest 8907/8908 (key bytes plus app
  access token → a text-KV depot section with a NUL). DepotDownloader
  sends no branch_password_hash.
- Proposes keeping the key in the keyring (`ossteam-beta`), never the
  code; SetBranch/UnlockBetas/ForgetBetas IPC; ISteamApps
  GetNumBetas/GetBetaInfo/SetActiveBeta.
- Q5 proposes refusing `lcsrequired` branches (seen on 220070 `local`)
  as an M1 fix.
