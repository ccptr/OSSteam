# Reference hardware

SPEC §20 measures every budget "on reference hardware defined in
`docs/acceptance/reference-hw.md`, with hardware GPU rendering". This page
defines that hardware and how a measurement on it is taken.

**Status: partly answered (2026-11-03).** The Steam Deck is the Linux
reference, but it is not set up yet; until then the development machine
(§4) stands in for it. No Windows or macOS reference machine is
available yet. Q5 of §5 is open, so no §20 row in the acceptance pages
can be ticked yet.

## 1. What a reference machine is for

A budget met on a fast machine says little about the machines people use,
so a reference machine is a floor, not the development machine. It should
be:

- **Modest and common:** about the slowest hardware on which a Steam user
  still expects a client to feel instant, so that meeting a budget there
  means meeting it nearly everywhere.
- **Fixed:** the same model can be bought or borrowed again, so a later
  run is comparable with an earlier one.
- **One per platform** that has §20 rows: Linux x86_64, Windows x86_64 and
  macOS (aarch64). Linux aarch64 and macOS x86_64 have no reference
  machine; their numbers are recorded when available but are not results.

## 2. Proposed machines

| Platform | Proposal | Why |
|---|---|---|
| Linux x86_64 | Steam Deck (LCD), SteamOS desktop mode or Arch on its internal SSD; wired gigabit Ethernet through a USB-C dock | Fixed hardware that many Steam users on Linux own: 4 Zen 2 cores, 16 GB shared memory, an RDNA 2 integrated GPU with Vulkan, a 1280 × 800 panel at 60 Hz. Its iGPU and memory bandwidth are well below a desktop's |
| Windows x86_64 | A desktop or laptop of the Deck's class or lower: 4 cores / 8 threads, 8 GB RAM, an integrated GPU with DirectX 11 feature level 11.1 or later, a SATA SSD, a 1920 × 1080 display at 60 Hz, wired gigabit Ethernet | Windows 11's own minimum is lower, but a 60 Hz 1080p iGPU machine with 8 GB is the floor of what plays Steam games. A specific model is to be named once one is available (§5 Q2) |
| macOS aarch64 | MacBook Air (M1, 2020), 8 GB, its built-in display, gigabit Ethernet through a USB-C adapter | The oldest and smallest Apple silicon Mac; macOS 14 and later still run on it |

Each reference machine is recorded in §4 with its exact model, OS
version, display and the date it was set up. A change of any of those
starts a new entry rather than editing the old one.

## 3. Conditions of a measurement

- **Builds:** release builds (`--release`) of the commit under test, built
  on or for that machine. Record the commit.
- **GPU:** hardware rendering (Vulkan on Linux, DirectX on Windows, Metal
  on macOS). A run that fell back to a software renderer (for example
  llvmpipe) is void.
- **Display:** the machine's own display at its native mode and refresh
  rate. Record the refresh rate and scale factor; m1e.md also asks for a
  run at scale factor 2. On a display faster than 60 Hz the frame budget
  is that display's interval, not 16 ms (m1e.md).
- **Power:** on mains power with the platform's default power profile; on
  the Deck, its default TDP.
- **Load:** a fresh login with nothing else running besides the desktop
  and a terminal; no official Steam client running, no game, no build.
- **Network** (download throughput only): wired gigabit Ethernet with no
  VPN or proxy in the path, `content.*` settings at their defaults. Before
  the run, record what the link reaches to a nearby speed-test server; if
  that is below 1 Gbit/s, the run measures the line, not the client, and
  is not a result. Record the CDN host the daemon chose.
- **Storage** (download throughput): the install goes to the machine's
  internal drive, in a library folder on a file system with room for the
  title twice over.
- **Repetition:** each row says how many runs it takes (m1e.md: five
  cold starts, record the slowest). Where a row does not say, take five
  runs and record the worst.

How each budget is measured stays with its acceptance page: m1b.md
(download throughput), m1c.md (shim callback pump), m1e.md (GUI start-up
and scrolling). m1a.md's row for the daemon's idle RSS, idle CPU and the
CLI round trip says no more than the budgets, so they are measured on
the reference machine as follows, once it is chosen:

- **Daemon idle RSS:** with an account logged in and a library loaded, no
  job, no game and no leaf client connected for 5 minutes; the resident
  set from `ps -o rss` (Linux, macOS) or the working set from Task
  Manager's *Details* (Windows).
- **Daemon idle CPU:** over the same 5 minutes, CPU time from `ps -o time`
  before and after, and wakeups from `powertop` (Linux), *Energy* in
  Activity Monitor (macOS) or Windows Performance Recorder (Windows).
- **CLI round trip:** 100 runs of `ossteam status` with the daemon
  running, p95 of the wall time.

## 4. Machines on record

**Linux x86_64, interim, from 2026-11-03:** the development machine
below, until the Steam Deck is set up (§5 Q1). Its runs are recorded as
interim results: they show regressions, but its CPU, GPU and display are
far above the Deck's, so a budget met on it is run again on the Deck
before it counts.

The development machine, for comparison only. Its numbers in the
acceptance pages are labelled as such and are not results: its GPU and
display are far above any reference machine's, and its traffic goes
through a VPN.

| | |
|---|---|
| CPU | AMD Ryzen 5 3600X, 6 cores / 12 threads |
| Memory | 16 GB |
| GPU | AMD Radeon RX 7900 XT class (Navi 31), Vulkan |
| Display | 2560 × 1440 at 165 Hz, scale factor 1.25 (1 in the 2026-10-18 runs) |
| Storage | NVMe SSDs (WD_BLACK SN770, Samsung 990 EVO Plus) |
| Network | gigabit Ethernet to an internet line of up to 970 Mbit/s (about 800 in practice, 2026-11-03), all traffic through Mullvad VPN |
| OS | Arch Linux, kernel 7.2.9, Hyprland |

## 5. Questions for review

1. Is the Steam Deck acceptable as the Linux reference, and is one
   available? If not, which Linux machine, and should it be the Windows
   machine booted into Linux, so the two OSes are compared on the same
   hardware?

   *Answer (2026-11-03):* the Steam Deck is acceptable, and the
   maintainer has one, but it needs setting up for this. Until then the
   development machine is the interim reference (§4).
2. Which Windows machine is available? If none of the class in §2, is a
   faster one acceptable as an interim reference, recorded as such?

   *Answer (2026-11-03):* Windows on the development machine, a dual
   boot, closer to release.
3. Is a MacBook Air (M1, 8 GB) available, or which Mac instead? macOS
   runs also need one for the M1e acceptance rows, so it is likely the
   same machine.

   *Answer (2026-11-03):* no Mac is available yet.
4. Download throughput: is there a wired gigabit line available without
   the VPN (`novpn`, or another network)? Through the VPN the line, not
   the client, sets the rate.

   *Answer (2026-11-03):* wired Ethernet to a line of up to 970 Mbit/s,
   about 800 in practice. Open: that is below the 1 Gbit/s §3 asks for,
   and whether a run may bypass the VPN.
5. Should a missed budget on a reference machine block M1 even when it
   is met everywhere else, as SPEC §21 M1e's exit says ("All §20 budgets
   are met"), or only be recorded until release?
