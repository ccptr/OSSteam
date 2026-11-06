# Steam Input

| | |
|---|---|
| Status | Draft; awaiting review. Nothing has been built, and §11 lists what must be observed first |
| Requirements | SPEC §1.1 (goals 3 and 4), §12.3, §20, §21 Later, ABI-3, LIC-9, WS-1, SYNC-1, DIRS-1, FAITH-1 to FAITH-4, GIPC-1, GIPC-2, VAC-4 |
| Code | none yet. `ISteamInput` and `ISteamController` are reachable through their getters and fail under ABI-3 (SPEC §12.3) |

Steam Input is the part of the official client that reads game
controllers itself, instead of leaving them to the game, and gives each
game what it wants from them. It does three jobs:

1. It finds and opens controllers: Valve's own (the Steam Controller of
   2015, the Steam Deck's built-in controls, the Steam Controller of
   2026) and other makers' (Xbox, PlayStation, Nintendo, generic
   gamepads), as far as the user and the game have *opted in* (§2.3).
2. It applies a *configuration* to each controller: what every button,
   stick, trackpad, trigger and gyro does. A configuration can turn the
   controller into keyboard keys, mouse movement and a *virtual gamepad*
   (§4), so that games that know nothing of Steam Input still get the
   layout the user chose.
3. It serves `ISteamInput` and its older name `ISteamController` (§5) to
   games that use the API: they name *actions* ("Jump", "Move") in an
   *action manifest*, and the configuration binds controls to them.

In ossteam all three are a library, `ossteam-input`, that other programs
can embed without the daemon or a Steam session (SPEC §1.1, LIC-9). §6
says where the library ends and the daemon and the shim begin.

**Verification.** This draft draws on the following sources:

- the Steamworks SDK headers that Proton ships (`isteaminput.h` and
  `isteamcontroller.h`, SDK 1.24 to 1.65), for the interfaces, their
  versions, structures and callbacks;
- the official client's install on the development computer:
  `controller_base/` (54 templates and the client's own configurations,
  glyph images under `controller_base/images/api/`), the action manifest
  and configurations that Counter-Strike: Global Offensive (legacy) ships
  in its depot, and the app info of the 201 apps in its PICS cache;
- the official client's `logs/controller.txt`, for the kinds of lines it
  writes. Only their structure is used, never their values;
- the message definitions that SteamDatabase extracts from the official
  client (`steammessages_accounthardware.steamclient.proto`,
  `webuimessages_steaminput.proto`, `steammessages_clientsettings.proto`,
  `steammessages_publishedfile.steamclient.proto`);
- SDL 3 (zlib), for the device ids of `controller_list.h`, the controller
  types of `controller_type.h` and the `SteamVirtualGamepadInfo` file
  that SDL reads in games (§4.2). Facts only: SDL's Valve device drivers
  carry code from Valve;
- the `steam-devices` udev rules that the distribution packages, for the
  device permissions the official client needs on Linux.

No configuration has been edited and no API game has been run against
the official client with the test account, so how it chooses, stores and
applies configurations is inferred from files and messages, and is
*unverified* until §12 is done.

## 1. Words

| Word | Meaning |
|---|---|
| controller | a physical device that Steam Input has opened |
| controller type | the family it belongs to, named as configurations name it: `controller_xboxone`, `controller_ps5`, `controller_neptune` (the Steam Deck) and so on (§2.1) |
| source | one part of a controller that a configuration binds: `button_diamond` (the four face buttons), `dpad`, `joystick`, `right_joystick`, `left_trigger`, `right_trigger`, `left_trackpad`, `right_trackpad`, `center_trackpad`, `switch` (shoulder, menu and back buttons), `gyro` |
| group | how one source behaves: a *mode* (`dpad`, `joystick_move`, `absolute_mouse`, `flickstick` …) with its inputs, bindings and settings (§3.1) |
| binding | what an input does: press a key, move the mouse, press a virtual gamepad button, fire a game action, run a client action (§3.1) |
| action set, layer | a named set of game actions, and a layer that changes some of them on top of the active set (`ISteamInput::ActivateActionSet`, `ActivateActionSetLayer`) |
| action manifest | a game's file naming its action sets, layers and actions, and its own configurations (§3.3); the "In-Game Actions" file |
| template | a ready-made configuration for a kind of game ("Gamepad", "Keyboard (WASD) and Mouse"), not tied to one game (§3.2) |
| virtual gamepad | a gamepad that Steam Input creates for the game to see, whose state comes from the configuration (§4.2) |
| lizard mode | the Steam Controller's built-in keyboard and mouse emulation, which the client turns off while it drives the controller (§2.2) |

## 2. Devices

### 2.1 Controller types

The configuration files name these types (`controller_type`, and the
template file names in `controller_base/templates/`):

| Type | Device |
|---|---|
| `controller_steamcontroller_gordon` | Steam Controller (2015) |
| `controller_neptune` | Steam Deck built-in controls |
| `controller_triton` | Steam Controller (2026); seen in the client's own files (`chord_triton.vdf`) |
| `controller_xbox360`, `controller_xboxone` | Xbox 360, Xbox One and Series |
| `controller_ps3`, `controller_ps4`, `controller_ps5` | DualShock 3 and 4, DualSense (and Edge) |
| `controller_switch_pro`, `controller_switch_joycon_left`, `controller_switch_joycon_right` | Nintendo Switch Pro, Joy-Cons |
| `controller_generic` | any other gamepad |
| `controller_android`, `controller_apple`, `controller_mobile_touch` | Steam Link on phones; out of scope (§10) |

SDL's `controller_type.h` holds the numbers the client uses for the
same families (`k_eControllerType_SteamController` = 2,
`…SteamControllerNeptune` = 4, `…SteamControllerTriton` = 10,
`…XBox360Controller` = 31, `…PS4Controller` = 34, `…PS5Controller` = 45
and so on), and its `controller_list.h` maps 619 USB vendor and product
ids to them. Valve's devices have vendor id `0x28de`. The protobuf
messages of §3.4 carry `controller_type` as such a number. The
`ESteamInputType` that games see (§5) is a different, smaller enum.

### 2.2 How the official client reads them

From `logs/controller.txt` on Linux:

- Valve's controllers are opened through `hidraw` with the client's own
  driver ("Steam controller device opened for index …", "Controller uses
  V2 HID protocol via Dongle" for the 2026 controller's USB receiver).
  The client takes the controller out of lizard mode when it opens it
  (`CExitLizardModeWorkItem`) so that its keyboard and mouse reports
  stop, and closes the device when a read fails.
- Other devices are read through SDL 3. The client ships its own build
  (`ubuntu12_32/libSDL3.so.0` and `ubuntu12_64/libSDL3.so.0`, from SDL's
  3.4 branch) and `steamclient.so` loads it at run time (it is not among
  the library's `NEEDED` entries, but its strings name `libSDL3.so.0`
  and the SDL gamepad and joystick functions it calls). The same strings
  hold SDL hint names that choose SDL's HIDAPI drivers
  (`SDL_JOYSTICK_HIDAPI_STEAM`, `SDL_JOYSTICK_HIDAPI_STEAMXBOX`,
  `SDL_JOYSTICK_HIDAPI_GIP`, `SDL_HIDAPI_LIBUSB` …), and the path format
  `sdl://%d` that the log shows for devices found that way. The only
  such device in the log on the development computer is a set of racing
  pedals. *Unverified*: the values the client gives those hints, and
  whether it leaves Valve's controllers out of SDL (as its own hidraw
  driver suggests).
- For each app that starts, the client logs an "Opted-in Controller Mask
  for AppId": a bit per controller type (`ffff` for all) (§2.3).

The devices need permissions that a user normally lacks. The
`steam-devices` rules give the user at the seat (`uaccess`) read and
write access to `hidraw` and `input` devices with vendor id `0x28de`,
to the `hidraw` devices of the Sony and Nintendo controllers listed by
id, and to `/dev/uinput` (§4). ossteam needs the same rules (§9).

On Windows and macOS: *unverified*. The client is expected to read
devices through the platform's HID API (Windows HID and XInput, macOS
IOKit and Game Controller framework), which §11 must confirm.

### 2.3 Which controllers it takes

Steam Input takes a controller for a game only when both sides opt in:

- **The user**, per controller family, in the client's settings. The
  client's settings definitions name them (`steammessages_clientsettings.proto`):
  `SteamController_PSSupport` (an `int`, default 1),
  `SteamController_XBoxSupport`, `SteamController_SwitchSupport` and
  `SteamController_GenericGamepadSupport` (each a `bool`), stored for
  the user on this computer. Valve's own controllers are always taken.
  *Unverified*: what each value of `SteamController_PSSupport` means.
- **The developer**, per app, through the Steam Input settings on the
  partner site. The game sees the result through
  `GetSessionInputConfigurationSettings` (`ESteamInputConfigurationEnableType`).
  The user can override it per game. *Unverified*: where the client
  reads the developer's mask; it is not among the app info keys in the
  cache (§3.4), and the log's opted-in mask is all §11 has.

A controller that Steam Input does not take is left to the game, which
reads it as it would without Steam.

### 2.4 Only one reader

When Steam Input drives a controller, the game must not also read the
physical device, or it sees every press twice. The official client
tells the game's SDL which devices to skip through the environment
(`launch-env.md` lists the variables, unset by ossteam today):

- `SDL_GAMECONTROLLER_IGNORE_DEVICES`: the vendor and product ids
  (`0xVVVV/0xPPPP`, comma-separated) of the controllers Steam Input has
  taken;
- `SDL_GAMECONTROLLER_ALLOW_STEAM_VIRTUAL_GAMEPAD`: lets SDL see the
  virtual gamepads (§4.2) that it would otherwise skip;
- `SteamVirtualGamepadInfo`: the path of a file describing each virtual
  gamepad (§4.2).

Games that read devices without SDL (`hidraw`, evdev or, on Windows,
XInput and DirectInput directly) are not told. *Unverified*: whether the
official client does anything for them on Linux; on Windows see §4.2.

## 3. Configurations

### 3.1 The format

A configuration is text KeyValues (`keyvalues.md`) with the root key
`controller_mappings`:

```
"controller_mappings"
{
    "version"           "3"
    "revision"          "…"            // the author's edit count
    "title"             "…"
    "description"       "…"
    "creator"           "…"            // account id; -1 in official configurations
    "controller_type"   "controller_ps5"
    "major_revision"    "…"            // from the action manifest (§5.3)
    "minor_revision"    "…"
    "localization"      { "<language>" { "title" "…" "description" "…" } }
    "actions"           { "<action set>" { … } }      // API games only
    "action_layers"     { "<layer>" { … } }           // API games only
    "group"
    {
        "id"     "0"
        "mode"   "four_buttons"
        "inputs"
        {
            "button_a"
            {
                "activators"
                {
                    "Full_Press"
                    {
                        "bindings" { "binding" "xinput_button A" }
                        "settings" { "haptic_intensity" "1" }
                    }
                }
            }
        }
        "settings" { … }
    }
    "group" { … }                       // the key repeats, once per group
    "preset"
    {
        "id"     "0"
        "name"   "Default"              // or an action set's name
        "group_source_bindings"
        {
            "0"  "button_diamond active"
            "7"  "switch active"
            "8"  "right_trackpad inactive"
        }
    }
    "settings" { "left_trackpad_mode" "0" "right_trackpad_mode" "0" }
}
```

- `group` and `preset` repeat as keys, so a reader must keep duplicate
  keys in order (`keyvalues.md` allows it).
- A **preset** is one action set (or, with no API, the single
  `Default`). Its `group_source_bindings` attach groups to sources by
  id: `<group id>` = `<source> active|inactive [modeshift]`. An
  `inactive` group is kept so that the editor can switch to it; a
  `modeshift` group replaces the active one while its mode-shift button
  is held.
- Group **modes** seen in the client's files: `dpad`, `trigger`,
  `joystick_move`, `four_buttons`, `absolute_mouse`, `switches`,
  `flickstick`, `joystick_mouse`, `single_button`, `scrollwheel`,
  `disabled`, `gyro_to_mouse`, `mouse_joystick`, `joystick_camera`.
  Each mode has its own inputs (`dpad_north` …, `button_a` …, `click`,
  `edge`) and settings (sensitivity, dead zones, curves, haptics).
  There are more modes in configurations people publish (`radial_menu`,
  `touch_menu`, `mouse_region` and others; *unverified* until §11).
- **Activators** say when a binding fires. The client's files use
  `Full_Press`, `Soft_Press` (a trigger's partial pull), `Long_Press`,
  `Double_Press` and `Start_Press`; published configurations may use
  more (*unverified*). Each has its own settings (`haptic_intensity`,
  repeat, timings).
- **Bindings** are a verb and its arguments, with an optional label
  after a comma (`key_press SPACE, Jump`):

  | Verb | Arguments |
  |---|---|
  | `xinput_button` | a virtual gamepad button or axis: `A`, `B`, `X`, `Y`, `dpad_up` … , `shoulder_left`, `trigger_left`, `joystick_left` … |
  | `key_press` | a key name: `W`, `SPACE`, `LEFT_SHIFT`, `UP_ARROW`, `VOLUME_UP`, `PLAY` … |
  | `mouse_button` | `LEFT`, `RIGHT`, `MIDDLE`, `BACK`, `FORWARD` |
  | `mouse_wheel` | `SCROLL_UP`, `SCROLL_DOWN` |
  | `game_action` | `<action set> <action>`: an API game's action (§5) |
  | `mode_shift` | `<source> <group id>` |
  | `controller_action` | a client command: `CHANGE_PRESET`, `SHOW_KEYBOARD`, `SCREENSHOT`, `quit_application`, `controller_poweroff`, `empty_binding` and others |

  Several `binding` keys under one `bindings` fire together.

The full list of modes, inputs, settings, key names and controller
actions comes from the client's own files and published configurations
(§11), and is the largest piece of work before code. ossteam keeps the
format exactly so that configurations move between it and the official
client without change.

### 3.2 Templates and the client's own configurations

The client ships templates in `controller_base/templates/`: one per
controller type and kind of game (`controller_ps5_gamepad_fps.vdf`,
`controller_xboxone_wasd.vdf` …) plus type-neutral ones
(`gamepad_fps.vdf`, `wasd.vdf` …), and its own configurations in
`controller_base/` for the desktop, Big Picture, the chord (Guide button
held) and so on. They are Valve's data, not ossteam's: ossteam cannot
ship them or copy them into the repository (SPEC §2.6). §13
asks what it uses instead.

The desktop, Big Picture and other client configurations are also
published as apps of type `Config` (413080 "Steam Controller Configs -
Desktop", 413090 "… - Big Picture", 413100 "… - Video Player", 424690
"… - Streaming Client", 736120 "… - Launchers", 1235260 "… - Secure
Desktop"), whose app info names official configurations as for a game
(§3.4).

### 3.3 Action manifests

A game that uses the API ships its actions in a KeyValues file with
the root key `In Game Actions`:

```
"In Game Actions"
{
    "configurations"
    {
        "controller_ps4"     { "0" { "path" "ps4_controller_config.vdf" } }
        "controller_neptune" { "0" { "path" "steam_deck_controller_config.vdf" } }
    }
    "actions"
    {
        "GameControls"
        {
            "title"         "#InGame_Controls"
            "legacy_set"    "0"
            "StickPadGyro"  { "Move" { "title" "#Move" "input_mode" "joystick_move" } }
            "AnalogTrigger" { … }
            "Button"        { "jump" "#Jump" }
        }
        "BuyMenuLayer"
        {
            "set_layer"         "1"
            "parent_set_name"   "GameControls"
            …
        }
    }
    "localization" { "english" { "InGame_Controls" "In-Game Controls" … } }
}
```

- Each key under `actions` is an action set, or a layer when
  `set_layer` is `1`. Under it, `StickPadGyro` holds analog actions with
  their `input_mode` (`joystick_move`, `absolute_mouse`), and `Button`
  digital actions as `<name>` = `<title>`. `AnalogTrigger`, for analog
  trigger actions, is in Valve's documentation of the format but not in
  the manifest on the development computer (*unverified*).
- Titles starting with `#` are looked up in `localization`, in the
  user's language, then English.
- `configurations` names the game's own configuration for each
  controller type, by a path relative to the manifest.

The client finds the manifest in one of three ways:

1. app info `config/steaminputmanifestpath`: a path relative to the
   install folder, with Windows separators
   (`csgo\cfg\controller_config\steam_input_manifest.vdf`). The client
   logs "Found App Manifest for appid … specified path … os path …";
2. the game calling `SetInputActionManifestFilePath` with an absolute
   path (§5);
3. a manifest published with the official configurations through the
   Workshop (§3.4). *Unverified*: how the client gets it in this case.

### 3.4 Which configuration a controller gets

For each game and controller, the client picks one configuration. The
candidates, as far as the sources show (*unverified*: the order):

1. **The user's choice**, kept by the server per account, app and
   controller type: `AccountHardware.GetDesiredControllerConfigForApp`
   returns, for an `appidorname`, either a `publishedfileid` or a
   `templatename`, optionally for one controller serial only, with an
   `autosave` flag; `SetDesiredControllerConfigForApp` stores it.
2. **The user's own configurations.** They are files under the
   "Steam Controller Configs" app (241100, type `Config`), whose Steam
   Cloud setup saves `{Steam3AccountID}/config/*.vdf` under its install
   folder (`steamapps/common/Steam Controller Configs/`). *Unverified*:
   the layout below `config/`.
3. **The developer's official configurations**: app info
   `config/steamcontrollerconfigdetails` maps published file ids to a
   `controller_type`, the branches they apply to (`enabled_branches`,
   comma-separated) and `use_action_block` (`true` when the
   configuration uses the API's actions). `config/steamcontrollerconfigfileids`
   names one id without details. These are Workshop items (`workshop.md`).
4. **Configurations in the depot**, through the action manifest (§3.3).
5. **A template**: app info `config/steamcontrollertemplateindex`, an
   integer (values 1, 2, 4, 6 and 13 in the cache). *Unverified*: which
   template each value names.
6. **Community configurations**: other players' published
   configurations, which the client lists and the user may choose.

App info also has `common/controller_support` (`partial` or `full`),
`config/nativesteamcontroller` (seen on Spacewar, 480), the touch
configurations for phones (`steamcontrollertouchtemplateindex`,
`steamcontrollertouchconfigdetails`, out of scope) and
`common/controllervr` (VR, out of scope).

Candidates 1, 3 and 6 come from the server and the Workshop, and 2 is
synced through Steam Cloud. SPEC §21 says configurations shared through
the Workshop or synced through Steam Cloud wait for those parts of M3.
Before then ossteam has 4, 5 and configurations the user makes locally;
§13 asks whether reading the user's choice (1), which is neither, may
come first.

When it uses a published configuration, the client reports playtime for
it (`PublishedFile.SetPlaytimeForControllerConfigs`), and it reports
which Valve controllers it saw (`AccountHardware.UpdateControllerUsageReport`).
Both are visible to Valve, not to other users; §13 asks whether ossteam
sends them.

## 4. What a configuration drives

### 4.1 Keyboard and mouse

`key_press`, `mouse_button`, `mouse_wheel` and the mouse modes produce
keyboard and mouse input that every program sees:

| Platform | How |
|---|---|
| Linux | a `uinput` keyboard and mouse. This works under X11 and Wayland alike, since it is a device to the kernel, and needs write access to `/dev/uinput` (§2.2) |
| Windows | `SendInput`. Input sent into a window of higher integrity (an elevated program) is dropped by Windows |
| macOS | `CGEventPost`, which needs the user to grant ossteam Accessibility access |

*Unverified*: what the official client uses on Windows and macOS, and
the names it gives its `uinput` devices on Linux (§11).

### 4.2 The virtual gamepad

`xinput_button` bindings drive a virtual gamepad shaped like an Xbox
360 controller: 15 buttons, two sticks, two triggers, and rumble that is
passed back to the controller.

On **Linux** the client creates a `uinput` gamepad per controller that
needs one, with vendor id `0x28de` and product id `0x11ff` ("Steam
Virtual Gamepad" in SDL's `usb_ids.h`). The virtual gamepads are shared
by every running program: the SDK says the gamepad slots "on
Linux/macOS … are shared for all running apps"
(`SteamInputGamepadSlotChange_t`). For games that use SDL, the client
writes a file and names it in `SteamVirtualGamepadInfo`, so that SDL can
show the real controller's name and type for each virtual one:

```
[slot 0]
name=…
VID=0x…
PID=0x…
type=…           # an SDL gamepad type name, such as ps5
handle=…         # the controller's InputHandle_t (§5.2)
```

SDL reads it again when its modification time changes, at most every
three seconds, and ignores it when the game runs under older Wine
(`wine64-preloader`). *Unverified*: the file's
path, and which `type` names the client writes.

On **Windows** there is no way for a program in user mode to add a
gamepad that every program sees: that needs a signed kernel driver.
The official client is reported to emulate XInput inside the game's
process, through its overlay library, which hooks the game's controller
APIs (*unverified*; §11). ossteam has no overlay, and one needs a VAC
impact ADR (VAC-4), so §13 asks what ossteam does on Windows.

On **macOS**: *unverified*. A virtual HID device (`IOHIDUserDevice`)
needs an entitlement from Apple, so the same question applies (§13).

### 4.3 Which configuration is in effect

Only one configuration drives a controller at a time:

- while a game that ossteam launched is running and Steam Input applies
  to it, the game's configuration;
- otherwise, the desktop configuration (keyboard and mouse), so that a
  Steam Controller works as a mouse;
- in Big Picture and Game Mode (SPEC §1.1, goal 4) while no game runs,
  the Big Picture configuration (*unverified*: what its name suggests,
  §3.2);
- and while the Guide button is held, the chord configuration.

The official client switches with the focused window: when the user
moves from a game to another window, the desktop configuration takes
over ("per focus change", says the SDK for `SteamInputConfigurationLoaded_t`).
Under Wayland a program cannot learn which window has focus, and under
X11 and Windows it can, so §13 asks how ossteam decides.

### 4.4 Haptics, lights and motion

The configuration's haptic settings pulse the trackpads and rumble
motors; the API's `TriggerVibration`, `TriggerSimpleHapticEvent`,
`SetLEDColor` and `SetDualSenseTriggerEffect` reach the controller
through the same drivers. Gyro data feeds `gyro_to_mouse` and the
API's `GetMotionData`. The calibration the user does in the client
(`CSteamInputService_GyroCalibration_Notification`, software
calibration) is the official client's own UI service; ossteam keeps its
own calibration data with the library.

## 5. `ISteamInput` and `ISteamController`

### 5.1 Versions

| Interface | Versions | SDK |
|---|---|---|
| `ISteamController` | `STEAMCONTROLLER_INTERFACE_VERSION` (1.24), `SteamController003` to `SteamController008` | 1.24 to 1.65 |
| `ISteamInput` | `SteamInput001`, `SteamInput002`, `SteamInput005`, `SteamInput006`, `SteamInput007` | 1.43 to 1.65 |

`ISteamController` is the older name of the same API: its functions
are a subset of `ISteamInput`'s with `Controller` in the names, and
ossteam serves both from one implementation. The vtables come from
Proton's generated tables like the other interfaces
(`steamworks-abi.md` §2.4), and the structure layouts are already in
`steamworks-abi.md` §2.2.

### 5.2 Handles

- `InputHandle_t` is a `u64` naming one controller for the game. At
  most `STEAM_INPUT_MAX_COUNT` (16) are connected at once.
  `STEAM_INPUT_HANDLE_ALL_CONTROLLERS` (`u64::MAX`) addresses all of
  them where the SDK allows it. *Unverified*: how the client chooses
  the values, and whether a controller keeps its handle when it
  reconnects.
- `InputActionSetHandle_t`, `InputDigitalActionHandle_t` and
  `InputAnalogActionHandle_t` are `u64` handles that the game looks up
  by name once (`GetActionSetHandle` …), and that are 0 for a name the
  manifest lacks. *Unverified*: whether they are indexes or hashes.

### 5.3 What each function does

- `Init(bExplicitlyCallRunFrame)` starts the API for the game;
  `Shutdown` ends it. With `false`, the state updates whenever the game
  runs callbacks (`SteamAPI_RunCallbacks`); with `true`, only when it
  calls `RunFrame`.
- `RunFrame` takes the newest state from the daemon. The SDK says that
  `GetConnectedControllers` returns nothing before the first one.
  `BWaitForData` blocks until new state arrives or the timeout ends,
  and `BNewDataAvailable` says whether it has since the game last read
  an action. The SDK describes the official client's side as "an IPC
  event from Steam … when there is new data to be fetched from the data
  drop" (§6.3).
- `SetInputActionManifestFilePath` gives the manifest's path (§3.3).
- Action sets and layers: `GetActionSetHandle`, `ActivateActionSet`,
  `GetCurrentActionSet`, `ActivateActionSetLayer`,
  `DeactivateActionSetLayer`, `DeactivateAllActionSetLayers`,
  `GetActiveActionSetLayers` (at most 16). The configuration's preset
  for the set comes into effect (§3.1).
- Actions: `GetDigitalActionHandle`, `GetDigitalActionData` (pressed,
  active), `GetAnalogActionHandle`, `GetAnalogActionData` (mode, x, y,
  active), `StopAnalogActionMomentum`, and `Get…ActionOrigins`, which
  list the controls bound to an action as `EInputActionOrigin` values
  (at most 8).
- Names and art: `GetStringForActionOrigin`, `GetStringForXboxOrigin`,
  `GetStringFor…ActionName` return localized names;
  `GetGlyphPNGForActionOrigin`, `GetGlyphSVGForActionOrigin`,
  `GetGlyphForActionOrigin_Legacy` and `GetGlyphForXboxOrigin` return
  the **path of an image file** in the client's install
  (`controller_base/images/api/{dark,light,knockout}/…`, in sizes `_sm`,
  `_md` and `_lg`). Under Proton the path is turned into a Windows
  path for the game. Those images are Valve's, so ossteam needs its own
  (§13).
- Devices: `GetConnectedControllers`, `GetInputTypeForHandle`
  (`ESteamInputType`), `GetControllerForGamepadIndex` and
  `GetGamepadIndexForController` (between handles and virtual gamepad
  slots), `GetActionOriginFromXboxOrigin`, `TranslateActionOrigin`,
  `GetDeviceBindingRevision` (the configuration's `major_revision` and
  `minor_revision`).
- Output: `TriggerVibration`, `TriggerVibrationExtended`,
  `TriggerSimpleHapticEvent`, `Legacy_TriggerHapticPulse`,
  `Legacy_TriggerRepeatedHapticPulse`, `SetLEDColor`,
  `SetDualSenseTriggerEffect` (§4.4). On a controller that cannot do
  it, a call does nothing.
- `GetMotionData`: the gyro's orientation, speed and acceleration.
- `ShowBindingPanel` opens the configuration editor for the
  controller. ossteam's GUI is the editor (§7); while it has none, the
  call returns `false`.
- `GetRemotePlaySessionID` returns 0: ossteam has no Remote Play.
- `GetSessionInputConfigurationSettings` returns the app's opted-in
  configuration types (§2.3).
- `EnableActionEventCallbacks(pCallback)` makes the API call the game's
  function directly, from `RunFrame` or `SteamAPI_RunCallbacks`, with a
  `SteamInputActionEvent_t` for each change of an action.

### 5.4 Callbacks

Through the usual callback queue (`steamworks-abi.md` §2.5), in
`RunFrame` or `SteamAPI_RunCallbacks`:

| Callback | Id | When |
|---|---|---|
| `SteamInputDeviceConnected_t` | 2801 | a controller connects, after `EnableDeviceCallbacks`; once for each already connected one when it is called |
| `SteamInputDeviceDisconnected_t` | 2802 | a controller disconnects, after `EnableDeviceCallbacks` |
| `SteamInputConfigurationLoaded_t` | 2803 | a configuration takes effect for a controller, "once per controller per focus change"; carries the app, handle, the configuration's creator, revisions, and whether it uses API actions and gamepad bindings |
| `SteamInputGamepadSlotChange_t` | 2804 | a controller's virtual gamepad slot changes |

`k_iSteamControllerCallbacks` is 2800.

### 5.5 Until the API is built

Until ossteam serves the API, both interfaces keep failing under ABI-3:
`Init` returns `false`. Spacewar (480) stops there under Proton
(`m1c-play` status note). Once the API is built, `Init` returns `true`
whenever the daemon can serve input for the game, also with no
controller connected, since that is a normal state (a controller may
connect later). §13 asks the maintainer to confirm.

## 6. The library, the daemon and the shim

### 6.1 `ossteam-input`

The library holds everything that does not need Steam:

- **devices**: finding, opening and reading controllers on each
  platform, with their permissions, hot plugging, lizard mode, haptics,
  lights and gyro calibration. Other makers' controllers are read
  through SDL 3 (zlib), as the official client reads them (§2.2; the
  maintainer, 2026-11-06). The library uses SDL's joystick, gamepad,
  sensor and HID APIs only, never its windows or events loop for the
  embedding program; SDL runs on the library's own device thread.
  Valve's Steam Controllers (2015 and 2026) and the Steam Deck's
  built-in controls are not: they have a driver of ossteam's own, `ossteam-steamcontroller` (§6.5; the maintainer,
  2026-11-06), and SDL is told to leave them alone, as the official
  client's HIDAPI hints suggest it does (§2.2). How SDL is built and shipped
  is settled with the library's code; the proposal is a pinned build
  from source, as `rusqlite` bundles SQLite, so that every platform
  gets the same version and the official client's install is never
  used;
- **configurations**: reading and writing `controller_mappings` (§3.1)
  and action manifests (§3.3) with `ossteam-vdf`, and checking them;
- **the mapping engine**: from a controller's state and a configuration
  to keyboard, mouse, gamepad and action output, frame by frame. It is
  pure: no clocks or devices of its own, time is passed in, so it can be
  tested with recorded input;
- **outputs**: `uinput`, `SendInput` and `CGEventPost` for keyboard and
  mouse, and the virtual gamepad where the platform allows (§4.2);
- **action state**: per controller, the digital and analog action
  data, the active set and layers, and motion, which the daemon copies
  into what the shim reads (§6.3).

Its API uses its own types only: no app ids, accounts, IPC messages or
other ossteam types (SPEC WS-1). An embedding program gives it the
configuration to use and gets input out; which configuration that is,
and where it comes from, is the embedder's business. It is synchronous
(SYNC-1): its device threads are its own and its API blocks or polls.

### 6.2 The daemon

`ossteamd` embeds the library and adds what needs Steam or the daemon:
which app is running and has focus (§4.3), the user's and developer's
opt-in (§2.3), choosing the configuration (§3.4), the environment
variables of §2.4 at launch (`launch-env.md`), the settings, and the IPC
and game IPC messages. It owns the devices for every game at once, so a
controller is opened once even when two games run.

### 6.3 The shim

The shim (`ossteam-steamclient`) implements `ISteamInput` and
`ISteamController` in the game's process. Action state changes every
few milliseconds, so it does not travel as game IPC messages: the daemon
writes each game's state into memory shared with that game, and wakes
it with an event, as the SDK says the official client does (a "data
drop" and an "IPC event"). Setup, manifest paths, action set changes and
haptics go over game IPC, as their rate is low. The shared memory's
layout belongs to the game IPC (`ossteam-gameipc`), not to the library:
the shim depends on nothing else (WS-2), and the library stays free of
ossteam's protocol (WS-1). The game can only read its own app's state
(GIPC-1).

*Unverified*: the latency the official client achieves between a
controller report and `RunFrame`; §11 measures it. SPEC §20 has no
budget for it yet (§13).

### 6.4 Which crates use it

`ossteamd` (all of it). `ossteam-gui` edits configurations through the
daemon's IPC, since WS-1 lets it depend only on `ossteam-ipc`,
`ossteam-types` and `ossteam-ui`; if the editor turns out to need the
library's configuration model directly, WS-1 changes first (§13).
`ossteam-steamclient` does not (WS-2): it reads what the daemon
writes in the layout of `ossteam-gameipc` (§6.3). `ossteam-cli` uses the
daemon's IPC, not the library.

### 6.5 `ossteam-steamcontroller`

A driver for Valve's Steam Controllers (the 2015 controller, the 2026
controller with its receiver, and the Steam Deck's built-in controls;
the maintainer, 2026-11-06) that any program can use: a game engine,
a remapper, a tool for firmware settings, with or without
`ossteam-input`. It is LGPL-2.1-or-later (LIC-9), pure Rust, and
depends on no workspace crate (WS-1). It has two layers:

- **protocol**: pure functions from report bytes to the controller's
  state (buttons, sticks, trigger, trackpads, gyro, battery) and from
  requests (lizard mode off and on, settings, haptic pulses, rumble,
  lights) to report bytes. No I/O, so it can be tested with recorded
  reports and used over any transport an embedder has;
- **transport**: opening the devices and moving reports, per platform
  (`hidraw` on Linux, the HID API on Windows, IOKit on macOS), with
  hot plugging, behind a feature so that an embedder with its own HID
  access can leave it out.

Each controller's protocol is written up from facts in a page of its
own (`docs/spec/input/steam-controller-2015.md`,
`…/steam-controller-2026.md`, `…/steam-deck.md`) before its code (§11
item 7). The Deck's controls are a USB device inside the Deck, with no
receiver or Bluetooth. SDL's driver keeps their lizard mode off by
sending a command again on a timer, which suggests the device turns it
back on by itself (*unverified*; §11, §13 Q8). The development computer
has a 2026 controller with its receiver; the 2015 controller needs one
to test with (§13), and the Deck's controls the Deck that is to be the
Linux reference (`docs/acceptance/reference-hw.md`).

On Linux, the kernel's `hid-steam` driver also takes these controllers,
and is reported to stop its own input when a program opens the
`hidraw` device (*unverified*; §11).

## 7. IPC, CLI and GUI

Over the client IPC (`ipc.md`), the daemon offers: the connected
controllers and their types; the user's opt-in settings (§2.3) and the
per-game override; the configuration in effect for each game and
controller, and choosing another; reading and saving a configuration;
and events when controllers connect or the configuration in effect
changes.

The CLI gets `ossteam input` with subcommands for the controllers,
settings and choosing a configuration. The GUI gets a controller page
in the settings and a configuration editor reachable from each game,
which `ShowBindingPanel` opens (§5.3). Editing configurations in the
GUI is the larger part of this work and gets its own section in
`gui.md` before it is built.

## 8. What ossteam keeps

Under the daemon's data directory (DIRS-1):

- the configurations the user saves, in the official format (§3.1),
  per account, app and controller type, laid out as the official
  client lays out app 241100 once §11 has seen it, so that M3 can sync
  them through Steam Cloud without a conversion;
- the configuration chosen for each app and controller type, until the
  server's copy (§3.4) can be used;
- gyro calibration and per-controller settings, in the library's own
  files.

## 9. Platforms and permissions

- **Linux**: ossteam needs read and write access to the controllers'
  `hidraw` devices and to `/dev/uinput`. The `steam-devices` rules
  give both to the user at the seat. When they are missing, the daemon
  says so (the controllers it cannot open, and why) instead of showing
  no controllers; ossteam may ship equivalent rules in its packages.
  Under Flatpak, devices need `--device=all`.
- **Windows**: HID devices need no special rights. Input sent to
  elevated windows is dropped (§4.1). The virtual gamepad waits for §13.
- **macOS**: sending keyboard and mouse input needs the user's
  Accessibility permission, and reading some HID devices needs Input
  Monitoring (*unverified* for gamepads). Both are asked for when Steam
  Input is first used, not at install.

## 10. Out of scope

Steam Link and Remote Play (the streaming client's configurations, the
phone touch configurations, `GetRemotePlaySessionID`), VR controllers
(`controllervr`), the screen reader and magnifier
controller actions (`sr_*`, `toggle_magnifier`), game recording
(`gr_*`), and pairing the 2026 controller's receiver
(`PairDongleTritonConnected` and the related messages), which the
receiver keeps by itself once paired. Controller registration
(`AccountHardware.RegisterSteamController`) is left out until a reason
for it appears. What the Steam and Quick Access buttons do in Big
Picture and Game Mode belongs to `docs/spec/game-mode.md` (SPEC §1.1,
goal 4), not here.

## 11. Investigation before code

1. With the test account and the official client: connect a controller,
   start a game that does not use the API, and record what the client
   creates (`uinput` devices and their names, the `SteamVirtualGamepadInfo`
   file and the environment of §2.4) and what it opens (`hidraw`, evdev,
   SDL).
2. Choose, edit and save a configuration for that game in the official
   client, then record the files under app 241100 and the
   `AccountHardware` and `PublishedFile` calls it makes (from its log
   lines). Structure only.
3. Run an API game (Spacewar, 480, and a game with a manifest in its
   depot) and record the handles it gets, the callbacks and their
   order, and the latency from a press to `RunFrame`.
4. Collect the full vocabulary of §3.1 (modes, inputs, settings,
   activators, key names, controller actions) from the client's files
   and from published configurations, and write it up as its own page,
   `docs/spec/input-config.md`.
5. Map `steamcontrollertemplateindex` values to templates.
6. On real Windows and a Mac (`hardware-mac-windows` status note): how
   the client reads devices, sends keyboard and mouse input, and
   emulates a gamepad, and whether the overlay is needed for it.
7. Whether SDL 3 gives everything a configuration needs for each
   other maker's family (touchpads, haptics, gyro); and the protocols of
   the 2015 and 2026 Steam Controllers and the Steam Deck's controls,
   written up from facts in a page each before `ossteam-steamcontroller`
   is written (§6.5), with how
   the kernel's `hid-steam` driver and the official client share the
   devices.

The controller on the development computer is a 2026 Steam Controller
with its USB receiver, which serves items 1 to 3 and 7 for that family.

## 12. Verification against the official client

Before the library is relied on, the same controller, game and
configuration give the same result under ossteam and the official
client: the same keys and mouse movement, the same virtual gamepad as
the game's SDL sees it, the same `ISteamInput` handles' behaviour,
action data, origins and callbacks, and a configuration saved by one is
read by the other unchanged.

## 13. Questions for review

1. **Device access.** *Answered 2026-11-06*: SDL 3 for other makers'
   controllers (§6.1), and a driver of ossteam's own for the Steam
   Controllers, reusable by other projects (`ossteam-steamcontroller`,
   §6.5). The Steam Deck's built-in controls go in it too (the
   maintainer, 2026-11-06). Still open: who lends a 2015 controller for
   testing?
2. **The virtual gamepad on Windows and macOS** (§4.2). Options: none
   (keyboard and mouse only, and API games), until an overlay ADR
   (VAC-4) allows hooking the game's controller APIs; a signed
   third-party virtual bus driver; or ossteam's own driver.
   Recommendation: none for now, stated plainly in the settings, and
   the question revisited with the overlay.
3. **Focus** (§4.3). Under Wayland ossteam cannot see which window has
   focus. Should the game's configuration apply for as long as the game
   runs, with the desktop configuration only when no game runs, or
   should ossteam use what each compositor offers (X11 and Windows give
   the focused window)? Recommendation: the first, everywhere, with
   focus following added where a platform allows it later.
4. **Templates and glyphs** (§3.2, §5.3). ossteam cannot ship Valve's.
   Should it write its own templates and glyph art, read them from an
   official client's install when one is present, or both?
   Recommendation: its own, minimal at first (a gamepad template per
   type, plain glyphs), since reading another install's files makes
   behaviour depend on it.
5. **The user's choice from the server** (§3.4). May the daemon read
   `GetDesiredControllerConfigForApp` before M3, so that a template the
   user chose in the official client is used? Writing it back
   (`SetDesired…`) changes what the official client does, so that would
   wait for M3 with the rest.
6. **Reports to Valve** (§3.4). Should ossteam send
   `UpdateControllerUsageReport` and `SetPlaytimeForControllerConfigs`?
   They are not visible to other users. Recommendation: no, until a
   reason appears.
7. **`Init` with no controllers** (§5.5): return `true`, as proposed?
8. **Budgets** (SPEC §20). Reading a connected controller wakes the
   daemon at the controller's report rate (hundreds of times a second),
   which the idle budget (≤ 1 wakeup a second) does not allow. Proposed:
   the idle budget holds while no controller is connected (hot plugging
   comes from device events, not polling), and a new budget is added
   for the time from a controller report to the game's `RunFrame` and
   to `uinput` output, set once §11 has measured the official client.
   The Deck's controls are always connected, so on a Deck the idle
   budget cannot rest on there being no controller; and keeping their
   lizard mode off adds a timer (§6.5). Should ossteam leave lizard mode
   on, and stop reading them, while nothing needs them (no game, no Big
   Picture or Game Mode, no desktop configuration of its own)?
9. **SPEC.** Should Steam Input get requirement ids (`INPUT-n`) and a
   milestone once this page is accepted, and should the SPEC add
   `controller_mappings` compatibility (§3.1) and the permissions of §9
   as rules? And should WS-1 let the GUI use `ossteam-input`'s
   configuration model for the editor (§6.4), or should it go through
   the daemon?
