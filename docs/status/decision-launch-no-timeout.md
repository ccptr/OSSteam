---
name: decision-launch-no-timeout
description: Unconfirmed decision - Client::launch waits with no timeout because tool downloads can be long
metadata:
  type: project
---

`Client::launch` waits with no timeout, because tool downloads can be
long (ipc.md §7, launch.md §8).
