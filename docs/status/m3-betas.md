---
name: m3-betas
description: M3 betas - betas.md drafted (5228106); §9 answered 2026-11-03 but Q1 (key storage), Q4 (older forms) and Q5 (lcsrequired) left open; §8 needs a published beta code
metadata:
  type: project
---

betas.md is drafted (5228106). Gate: §9 and §8, which needs a published
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
- §9 answered on 2026-11-03: switching branches in M3, and the CLI
  `ossteam beta unlock|forget` plus `ossteam branch`. Q1 (keeping the key
  in the keyring), Q4 (the older forms) and Q5 (`lcsrequired`) are left
  open.
