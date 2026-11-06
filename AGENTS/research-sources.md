---
name: research-sources
description: Outside sources used for reverse engineering and which may be used for facts only
metadata:
  type: reference
---

Research sources, none kept in the repository; clone them again as
needed, outside it:

- SteamDatabase Protobufs.
- Steamless (CC BY-NC-ND): facts only.
- Proton: facts only (vtables, struct layouts). The generated struct
  layouts are in `lsteamclient/steamclient_structs_generated.h`.
- SteamKit2, node-steam-user, DepotDownloader.
- SteamCloudFileManager's ROOT_PATH_MAPPING.md (community root-number
  table): facts only.
- SDL 3 (zlib): device ids, controller types and the
  `SteamVirtualGamepadInfo` file, for Steam Input. Facts only for its
  Valve device drivers (`SDL_hidapi_steam*.c`, `hidapi/steam/`), which
  carry code from Valve.
- The `steam-devices` udev rules (as distributions package them): facts
  only, for the device permissions Steam Input needs.
