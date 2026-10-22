# GrandV Back end launcher

The desktop host for Grand-V. It serves the game from your `GrandV-assets` folder to browsers on this PC, on your LAN or on a public website, and it runs the multiplayer relay. Release packages name the program `GrandV back-end host` (`.exe` on Windows, `.app` on macOS). Download them from [Releases](https://github.com/waste-mgmt-consultant/Grand-V-but-clean/releases). The game assets are separate.

`assets/app.png` is the GrandV logo. The build embeds it as the window and taskbar icon and as the Windows executable's icon. macOS packaging makes an `.icns` from the same image. Linux packages include the PNG and a desktop-entry template; to install, put the executable and the icon on the usual search paths. The icon build trims the logo's transparent padding and centers it in a square with a small border. The supplied PNG stays unchanged.

## Players

Use the build for your OS and CPU. You need no Rust, Python or C++ tools and no separate relay program. Windows builds include the Microsoft C runtime. macOS and Linux use the system libraries and graphics drivers. The launcher window needs OpenGL 2.0 or newer; where it is missing (for example in some virtual machines), the window does not open, but `--headless` works.

Put the launcher next to your `GrandV-assets` folder, or select the folder with **Browse…**. On macOS the launcher looks next to the `.app`. Select an **Audience**: **This PC**, **LAN / school network** or **Public website**. Optionally turn on multiplayer, then click **Start & open game**. **Start server** starts hosting without opening a browser. The addresses to open appear under the buttons: localhost, `127.0.0.1`, and each network IPv4 address when the audience allows network access.

**Stop server**, or closing the window, stops this launcher's server, LAN discovery and relay sessions. It does not stop another program's server. **Join a server** works without a local game folder.

- **Game fixes** caps the game's streaming memory, fixes Snapmatic photos and can add a cash hotkey. With **This PC**, each photo saved to the phone gallery is also saved as a JPG file in `GrandV-assets/photos/`. See [Game fixes](../../README.md#game-fixes).
- **Check game files** finds damaged game archives. See [Check game files](../../README.md#check-game-files).

LAN hosting creates a private CA and server certificate when needed. Share only `lan-root-ca.cer`; each client installs trust by hand. The launcher never changes system trust. **Create / renew LAN certificate** renews the server certificate and keeps the root. Roots made by the older Python launcher are kept with their original fingerprint and private key. A certificate and key that you supply are checked and used as they are, without automatic renewal.

Public hosting uses an automatic Cloudflare Quick Tunnel or an HTTPS origin that you set up yourself.

### Listen address

With a reverse proxy and direct HTTPS off, the server listens only on this computer (`127.0.0.1`). If the proxy runs on another machine, or the launcher runs in a container, choose **Advanced server settings → Listen on → All networks (0.0.0.0)**. You can also start the launcher with `--bind 0.0.0.0`, or set `"bind": "0.0.0.0"` in `launcher-settings.json`. All networks serves plain HTTP on every network interface, so allow only the proxy through the firewall.

**This computer only (127.0.0.1)** always uses loopback. **Automatic (from audience)** follows the audience. Cloudflare tunnel mode always listens on this computer only. Router, firewall and reverse proxy setup stay outside the launcher.

## Hosting for friends with free HTTPS

1. Install [cloudflared](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/downloads/). Windows: use **Copy install command (PowerShell / CMD)**, open PowerShell or Command Prompt, paste `winget install --id Cloudflare.cloudflared --exact` and press Enter. Restart the launcher after installation so it picks up PATH changes. Alternatively place `cloudflared.exe` beside the launcher (`cloudflared` on macOS/Linux).
2. Choose **Public website**, enable **Cloudflare free tunnel (temporary HTTPS link)**, select the game folder and optionally enable multiplayer with a generated code.
3. Press **Start server** or **Start & open game**. GrandV binds its backend to loopback HTTP, starts its owned cloudflared process, captures the HTTPS URL and configures game links and the `/relay` WebSocket origin. Direct HTTPS is disabled automatically. No account, domain, port forwarding or client certificate import is required.
4. Use **Share game link** for the link/QR. Send the multiplayer code separately. Friends open the link in Chrome/Edge; they need neither local game files nor cloudflared.
5. Keep the host awake and launcher open. **Stop server** or closing the launcher stops its tunnel/backend. An unexpected tunnel exit stops hosting; start again for a new URL. Other cloudflared processes/services are preserved.

The UI automates startup and URL setup after one-time dependency installation. It does not install software or a background service. The choice is remembered in `launcher-settings.json`; every Start creates a fresh URL. Headless mode does not manage tunnels; use the manual procedure below.

The URL is public, including game files and existing save APIs. The multiplayer code protects only the relay. For access control or a stable hostname, configure a named tunnel and Cloudflare Access separately, then use the manual **Public HTTPS origin** field.

[Quick Tunnel limits](https://developers.cloudflare.com/tunnel/get-started/quick-tunnels/): no uptime guarantee, 200 in-flight requests, no SSE, new hostname each start. Browser saves/cache are scoped to each origin, so each URL has separate browser storage. Host disk backups remain, but do not assume automatic browser-profile recovery. Asset loads consume host upload bandwidth. All proxied visitors appear as loopback, so backend per-IP admission/bandwidth limits apply collectively; forwarded IP headers are not trusted.

For failures, inspect **Console output**. URL allocation times out after 45 seconds and stops the attempted tunnel/backend. Existing `.cloudflared/config.yaml` / `config.yml` can interfere; inspect them without deleting unrelated setups. Public DNS/connection readiness can lag URL creation by a few seconds. See [Cloudflare's official guide](https://developers.cloudflare.com/tunnel/get-started/quick-tunnels/).

Manual alternative: run `cloudflared tunnel --url http://127.0.0.1:8000`, paste its printed `https://....trycloudflare.com` origin into **Public website**, leave direct HTTPS off and start GrandV on port 8000. Share the `/launcher.html` link, keep both programs running, and stop cloudflared separately. Substitute your actual game port.

## Speed-limit guidance

Advanced server settings also provide **Maximum concurrent connections / streams**. Direct HTTP/HTTPS and manually configured proxy hosting have no fixed application ceiling: choose a positive limit, or **0 = unlimited** (the default, including existing settings without this field). This counts open connections, including idle connections and relay sessions, rather than players. Automatic Cloudflare tunnel mode always uses **200**, disables this control, and retains your direct-hosting value for switching back. Cloudflare's own 200 in-flight request limit remains separate. Stop and restart hosting to apply changes.

Advanced server settings show a tuning starting point of **10 MiB/s (about 84 Mbit/s) per direct client** when capacity allows, or **0/unlimited on a fast LAN** to avoid artificial asset throttling. This is a practical estimate, not a measured game minimum or a stall-free guarantee.

For Internet hosting, budget about 80% of measured host **upload**: per-player MiB/s = upload Mbit/s × 0.8 / 8.388608 / active players. A 100 Mbit/s upload supports a budget of about 9.5 MiB/s total, or 4.8 MiB/s each for two equally active players. Through Cloudflare, the field is labeled as a **shared tunnel limit** because all visitors appear as loopback and share the cap. Direct clients sharing a source IP, and other reverse proxies, can also share a cap. Limits are not reservations of bandwidth or fairness guarantees. Raise the cap if capacity allows and asset loads are late; otherwise reduce simultaneous players. Initial uncached loads, client performance and network latency also affect results.

## Console and server details

**Console output** opens a separate native, movable/resizable desktop window with live launcher status, HTTP request results, browser `/log` messages, TLS failures, relay events, game fix results and game file check results. It includes Follow output, Copy output and Clear controls. Closing the console leaves hosting running; closing the launcher closes both windows and its owned server. Output history is bounded to 1500 lines / 512 KiB, and timestamps are labeled UTC. Request query strings, launcher tokens and relay access codes are not added by the server's diagnostic logging.

The launcher reads and writes the same files and protocols as earlier Grand-V launchers:

- Settings: existing `launcher-settings.json`, preserving unknown fields.
- Access code: existing `multiplayer-access-code.txt`, outside the served directory.
- Disk saves: existing browser profile directories under `userdata`, base64 restore data, millisecond mtimes, legacy tombstone lists and timestamp maps.
- Game HTTP APIs: `GET`/`HEAD`, single byte ranges, `/data/batch?gz=1`, `X-Run-Lengths`, `/log`, userdata PUT/DELETE/restore, and authenticated loopback `/__local/port` changes.
- Isolation headers: COOP, COEP and CORP on responses; correct WASM/JavaScript MIME types.
- Per-source-IP client admission and streaming bandwidth limits, the connection limit above, and up to 64 relay sessions.
- Relay protocol 1: `check`, `join`, `welcome`, `joined`, `pose`, `left`; case-sensitive access codes, build/map agreement, two players per room, server-assigned IDs, bounded/coalesced queues and message limits.
- UDP discovery: `GTA5LAN1` on port 8014 with request nonce, private peers, fingerprint and canonical browser link.
- Public certificate download/info, including plaintext download on the LAN HTTPS listener; other plaintext requests redirect or reject.
- QR sharing, manual Join, offline troubleshooting.
- **Updates and info** opens a movable small popup with **Telegram** (`https://t.me/reGRANDTA`), **Discord** (`https://discord.gg/Td8Jdjw3Wf`) and **GitHub** (`https://github.com/waste-mgmt-consultant/Grand-V-but-clean`) buttons. GitHub links to this fork, where its launcher releases are published. Each opens its link in the default browser.

The relay runs inside the host process at `/relay`. The old `relay_port` setting stays in the settings file for compatibility but is not opened or used. Share links never contain the local launcher capability or access code.

Deliberate hardening: directory listings are disabled, reserved save metadata names cannot be written through the save API, malformed tombstones fail closed, paths cannot escape through existing symlinks/junctions, and request parsing/body collection has time/size limits. Directory indexes still work.

## Command line

In release packages the program is `GrandV back-end host`; on macOS it is inside the app at `GrandV back-end host.app/Contents/MacOS/`. A source build is `target/release/gta-browser-launcher`. The examples use the source build name.

Host without a window:

```text
gta-browser-launcher --headless --folder PATH_TO_GRANDV_ASSETS --no-open
```

It loads the folder's existing settings. `--port` and `--bind` override the remembered port and listen address. Ctrl+C stops the owned host. Headless mode does not start a Cloudflare tunnel. A normal launch with the window accepts `--folder`, `--port`, `--bind` and `--no-open` too.

Check the game files without the window:

```text
gta-browser-launcher --check-assets --folder PATH_TO_GRANDV_ASSETS
```

It prints the result and, when it finds damage, the damaged archives and the repair steps. It exits with code 0 only when it finds no damage.

## Build and test

Run in this directory with Rust stable and the native platform build prerequisites installed:

```text
cargo test --locked --all-targets -- --test-threads=1
cargo build --locked --release
target/release/gta-browser-launcher --ui-smoke --no-open --ui-screenshot ui-smoke.png
```

On Windows the executable has `.exe`. The GUI smoke run closes itself and creates no browser tab. Tests use temporary game/save fixtures; they do not read or modify player saves.

- macOS: one compatibility test needs a second loopback address. Add it with `sudo ifconfig lo0 alias 127.0.0.2 up`; it lasts until restart.
- Linux: the build needs the X11 and OpenGL development packages. `../../.github/workflows/rust-build.yml` lists them for Ubuntu.

Automated HTTP/TLS fixtures bind only to loopback and disable LAN advertising, including fixtures configured with LAN settings. The UDP protocol test also binds only to loopback. Do not grant firewall access to hashed `compatibility-*.exe` or `legacy_ca-*.exe` test binaries. Only deliberate LAN/public hosting by the finished launcher needs external network access.

`../../.github/workflows/rust-build.yml` runs the tests and builds Windows x64, macOS Apple silicon and Intel, and Linux x64 packages on pushes to `main` that change the launcher. A manual run with a release tag also publishes a GitHub release with the packages and `SHA256SUMS.txt`. Keep `troubleshooting.json` beside `rust`; it is embedded at compile time. Do not upload game assets, saves, generated certificates, access codes, private keys or `target`/`dist` directories.

The separately ignored `legacy_ca` test requires `GTA_LEGACY_CA_FIXTURE` pointing to a temporary game folder whose CA was generated by the original Python `lan_tls.py`; run it with `cargo test --test legacy_ca -- --ignored` to verify RSA CA migration independently.

The packages are not signed by Apple or Microsoft, and the macOS app is not notarized. See the main README for how to open them.
