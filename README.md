# Grand-V, but clean

No memecoin shit. Just the game.

A fork of [SebasKitten/Grand-V](https://github.com/SebasKitten/Grand-V) without the coin and donation parts:

- The launcher has no donation window and no wallet list.
- The app name has no `$` ticker.
- The launcher removes the Donate, Tweet and Trade `$GrandV` buttons and the donation dialog from the web UI when it serves the pages. The game asset files stay unchanged.

The launcher also fixes known game crashes and can check the game files for damage. See [Game fixes](#game-fixes) and [Check game files](#check-game-files).

**[Download the launcher from Releases](https://github.com/waste-mgmt-consultant/Grand-V-but-clean/releases)** for Windows, macOS or Linux. GitHub Actions builds it from this repo. You can also [build it yourself](LauncherApp/rust/README.md#build-and-test). The game assets are separate.

- macOS: the app is not notarized. After you unzip it, run `xattr -dr com.apple.quarantine "GrandV back-end host.app"`, or allow it in System Settings → Privacy & Security.
- Windows: SmartScreen can warn about the unsigned exe. Click More info → Run anyway.

## Quick start

1. Put the launcher next to your `GrandV-assets` folder, or click **Browse…** in the launcher and select that folder.
2. Under **Audience**, choose who can play: **This PC**, **LAN / school network** or **Public website**.
3. Click **Start & open game**.

The [launcher guide](LauncherApp/rust/README.md) covers LAN and public hosting, multiplayer and command-line options.

## About Grand-V

<p align="center"><img src="grand-logo-v2.png" alt="Grand-V logo" width="220"></p>

<img width="1920" height="1080" alt="Screenshot" src="https://github.com/user-attachments/assets/718e0547-abb1-4240-87b5-fa6bf06bf912" />

Grand-V is a browser-based port of GTA V built on a reverse-engineered RAGE engine, converted to WebAssembly memory64 with shared-memory workers and WebGPU. The project builds on [rea](https://github.com/morluto/rea).

## Features

- Web launcher with Story and Sandbox modes
- Separate saves per mode
- Graphics settings and controller / touch controls
- Experimental multiplayer support
- Frame rate limits: 24, 30, and 60 FPS and experimental 120 FPS.

<p align="center"><img src="launcher-web.png" alt="Web launcher" width="480"> <img src="launcher-native.png" alt="Desktop launcher" width="400"></p>

## Game fixes

The launcher fixes known game bugs and can add a cash hotkey while it serves the game. The asset files stay unchanged: the launcher keeps fixed copies in `GrandV-assets/patched/` and rebuilds them when an asset or a setting changes. If a game file does not match the known version, the launcher serves it unchanged and says so under the server status and in **Console output**. Change the fixes under **Game fixes** before you start the server.

- **Streaming memory limit** (on by default): caps the memory the game uses for streamed world data at 6 × 256 MB (1.5 GB). Without a cap this memory keeps growing until the game stops with an "Invalid fixup" error. A lower limit uses less memory but can cause short pauses. In `launcher-settings.json`, set `"memory_limit": false` to turn it off or `"memory_limit_heaps"` (1–31) to change it.

- **Cash hotkey** (off by default): in Story mode, each press of the key right of 0 adds $999,999 to the current character. In `launcher-settings.json`, this is `"cash_hotkey"`.

If you used the old `cash_hotkey.py` script, delete the `GrandV-assets/overrides/` folder. The launcher serves the files in that folder first, so the old hotkey stays on until you delete it.

## Check game files

A torrent client creates each file at full size before it downloads the data. So the launcher's size check at start cannot see a download that did not finish or did not reach the disk. The game then stops with "zstd error Data corruption detected", or crashes again and again in the same area.

Click **Check game files**, then **Start check**. The check reads every game archive and makes sure that it opens. It changes nothing. It takes about a minute on a fast disk, and you can keep playing or hosting meanwhile. If it finds damage:

1. In your torrent client, force a recheck of GrandV-assets (qBittorrent: right-click, **Force recheck**) and let it download to 100%.
2. On the game page, open **More options, saves and troubleshooting**, then **Clear game cache**. The browser keeps its own copy of the damaged data until you do this. Saves and settings are kept.
3. Run the check again.

The check covers the compressed parts of the game archives, where damaged data crashes the game. It does not cover archives without them (mostly audio) or files outside the archives. A torrent recheck covers every file.

To run the check without the window, see [Command line](LauncherApp/rust/README.md#command-line).

## System Requirements

### Desktop (Primary Target)

| Component       | Minimum                          | Recommended                      |
|-----------------|----------------------------------|----------------------------------|
| CPU         | Modern 4-core processor          | —                                |
| RAM         | 8 GB (low-memory mode)           | 16 GB or more                    |
| Graphics    | WebGPU-compatible integrated or dedicated GPU with current drivers and browser hardware acceleration enabled | — |
| Browser     | Up-to-date Chrome, Edge, or compatible Chromium browser with shared WebAssembly memory64 support | — |
| OS          | Windows, macOS, or Linux         | Windows (most tested)            |

Compatibility on macOS and Linux depends on GPU, drivers, and browser support.

### Mobile

Android and iOS support is experimental. Browser feature availability, memory constraints, and mobile GPU compatibility remain limiting factors.

## Hosting & Setup

The project is designed to be as simple as possible to run.

- Local self-hosting — No certificate required.
- LAN or website hosting — The launcher manages server startup, graphics options, and LAN configuration.
- LAN HTTPS clients must install and trust the certificate generated by the launcher. This is not required when hosting as a standard HTTPS website.

Client devices only need to open the host’s URL in a compatible browser. No separate game installation is required on each client.

Behind a reverse proxy on another machine, or in a container, set the listen address. See [Listen address](LauncherApp/rust/README.md#listen-address).

## Current Development Focus

Active work is ongoing in the following areas:

- Stability
- Rendering
- Audio
- Streaming
- Compatibility

## Reporting issues

Report game bugs [upstream](https://github.com/SebasKitten/Grand-V/issues) with your device model, OS/browser versions, the type of bug, and steps to reproduce it.

Report problems with this fork [here](https://github.com/waste-mgmt-consultant/Grand-V-but-clean/issues): the launcher downloads, the coin removal, the game fixes and the game file check.

Not sure where a crash belongs? First run **Check game files** and repair any damage. Then try again with the **Game fixes** turned off. If the crash goes away, report it here.

## Disclaimer & Copyright

This project is provided for educational purposes only.  

We do not supply, distribute, or host any game data, mirror files, or proprietary assets belonging to Rockstar Games or Take-Two Interactive. Users are solely responsible for obtaining any required game files through legitimate means.

Grand Theft Auto V and all related trademarks, assets, and intellectual property are the property of their respective owners. This project is not affiliated with, endorsed by, or connected to Rockstar Games or Take-Two Interactive in any way.

## Community

Grand-V is created and maintained by SebasKitten. This fork is maintained separately.

Join the project channels to follow progress:

- Telegram: [https://t.me/reGRANDTA](https://t.me/reGRANDTA)
- Discord: [https://discord.gg/hrbcPjjZfj](https://discord.gg/hrbcPjjZfj)
- X: [@Sebas_Kitten](https://x.com/Sebas_Kitten)
