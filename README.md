# BittSeeder
![Test](https://github.com/Power2All/bittseeder/actions/workflows/rust.yml/badge.svg)
[<img src="https://img.shields.io/badge/DockerHub-link-blue.svg">](<https://hub.docker.com/r/power2all/bittseeder>)
[<img src="https://img.shields.io/discord/1476196704163201059?label=Discord">](<https://discord.gg/zMyZJz4U2D>)

A fast, unified Rust seeder that serves torrent data over **BitTorrent** (BT wire protocol) and **WebRTC** (RTC data channels) simultaneously — or either one on its own.

BittSeeder handles both protocols from a single binary. Both share the same torrent file, piece data, tracker URL list, and upload counter. You choose the protocol globally or per-torrent via YAML or CLI.

---

## Table of contents

- [Requirements](#requirements)
- [How it works](#how-it-works)
  - [BitTorrent mode](#bittorrent-mode)
  - [WebRTC mode](#webrtc-mode)
  - [Both mode](#both-mode)
  - [Shutdown](#shutdown)
- [Building](#building)
- [Usage — single-torrent (CLI)](#usage--single-torrent-cli)
- [Usage — multi-torrent (YAML)](#usage--multi-torrent-yaml)
  - [YAML format](#yaml-format)
  - [Global config keys](#global-config-keys)
  - [Torrent entry keys](#torrent-entry-keys)
- [Web management UI](#web-management-ui)
  - [Authentication](#authentication)
  - [Password hashing](#password-hashing-argon2id)
  - [Endpoints](#endpoints)
- [Batch Add & torrent upload](#batch-add--torrent-upload)
- [Let's Encrypt auto-certificate](#lets-encrypt-auto-certificate)
- [Thread count configuration](#thread-count-configuration)
- [Protocol selection reference](#protocol-selection-reference)
- [Supported BEPs](#supported-beps)
- [Client identification](#client-identification)
- [Architecture overview](#architecture-overview)
- [License](#license)

---

## Requirements

- **Rust 1.85+** (edition 2024)
- A C compiler (needed by the `webrtc` / `aws-lc-sys` dependency)
- Linux, macOS, or Windows

---

## How it works

### BitTorrent mode

When `protocol` is `bt` or `both` (the default):

1. **Torrent creation** — BittSeeder hashes the piece data (SHA-1 v1, SHA-256 v2, or hybrid) and writes a `.torrent` file.
2. **Tracker announce** — announces `started` to every configured tracker URL using HTTP (`http://`, `https://`) or the UDP tracker protocol (`udp://`). All trackers that respond successfully are kept; re-announces are sent to all of them. The re-announce interval is the minimum interval reported by any tracker.
3. **TCP listener** — binds a TCP port (default `6881`). In multi-torrent mode a single shared listener handles all torrents by looking up the info-hash from the BT handshake.
4. **Peer connection** — for each inbound TCP connection the BittSeeder performs the BT handshake, sends a full bitfield, unchokes the peer, then fulfils `REQUEST` messages by reading blocks directly from disk (no full-file buffering).
5. **UPnP** — optionally maps the TCP port on the local gateway using `igd-next`.
6. **Re-announce** — a background task re-announces at the tracker-supplied interval.

### WebRTC mode

When `protocol` is `rtc` or `both`:

1. **Offer creation** — a `RTCPeerConnection` is created and an SDP offer is generated with ICE candidates gathered (up to 5 s timeout). The seeder always holds one unused offer ready to hand to the next incoming peer.
2. **Tracker announce** — announces to every HTTP(S) tracker URL with the SDP offer encoded as `rtcoffer=`. Each tracker stores the offer alongside the seeder's peer entry. Answers are collected from all trackers each signaling cycle.
3. **Answer polling** — each signaling cycle BittSeeder re-announces and retrieves any pending `rtc_answers` from the tracker. For each answer it calls `set_remote_description` on the corresponding `PeerConnection`, completing the WebRTC handshake, and immediately creates a fresh offer for the next peer.
4. **Data channel** — once the WebRTC data channel opens, BittSeeder listens for `MSG_PIECE_REQUEST` frames (1-byte type `0x01` + 4-byte big-endian piece index). It reads the full piece from disk and replies with either a single `MSG_PIECE_DATA` frame (≤ 65 531 bytes) or multiple `MSG_PIECE_CHUNK` frames for large pieces.
5. **Re-announce interval** — controlled by `rtc_interval_ms` (default `5000` ms), overridden by the tracker's `rtc interval` response field.

### Both mode

When `protocol` is `both` (the default):

- All steps above run concurrently inside a single `tokio` runtime.
- Both protocols share the same `Arc<TorrentInfo>`, `Arc<AtomicU64>` upload counter, rate limiter, and peer ID.
- The BT TCP listener and RTC signaling loop are independent tasks coordinated by a `tokio::sync::watch` shutdown channel.
- Stats output shows `bt_peers`, `rtc_peers`, and `uploaded` together.

### Shutdown

On `Ctrl-C` or a web UI stop signal:

1. The watch channel broadcasts `true` — all background tasks (BT listener, BT re-announce, RTC signaling) exit their loops.
2. The RTC task sends a `stopped` announcement to **every** RTC tracker before returning (up to 5 s per tracker).
3. After tasks finish, every BT tracker receives a `stopped` announcement (up to 5 s per tracker).
4. In multi-torrent mode the shared TCP listener is aborted and the registry entry is removed.

---

## Building

```bash
# debug build
cargo build

# optimised release build (LTO, single codegen unit, stripped)
cargo build --release
```

The binary is placed at `target/debug/BittSeeder` or `target/release/BittSeeder`.

---

## Usage — single-torrent (CLI)

```
BittSeeder [OPTIONS] [FILES]...
```

### Core flags

| Flag | Default | Description |
|---|---|---|
| `--protocol <PROTOCOL>` | `both` | `bt`, `rtc`, or `both` |
| `--tracker <URL>` | — | Tracker announce URL (repeatable). HTTP and UDP supported for BT; only HTTP for RTC |
| `--port <PORT>` | `6881` | BT TCP listen port |
| `--upnp` | false | Enable UPnP port mapping |
| `--ice <URL>` | Google STUN | ICE server URL (repeatable), e.g. `stun:stun.l.google.com:19302` |
| `--rtc-interval <MS>` | `5000` | WebRTC signaling poll interval in milliseconds |
| `--name <NAME>` | file name | Torrent display name |
| `--out <FILE>` | `<name>.torrent` | Path to write the `.torrent` file |
| `--torrent-version <VER>` | `v1` | `v1`, `v2`, or `hybrid` |
| `--torrent-file <FILE>` | — | Re-seed from an existing `.torrent` file |
| `--magnet <URI>` | — | Re-seed using a magnet URI |
| `--webseed <URL>` | — | Web-seed URL (repeatable) |
| `--upload-limit <KB/s>` | unlimited | Per-torrent upload rate cap |
| `--web-port <PORT>` | `8090` | Start the web management UI on this port |
| `--web-password <PASS>` | — | Protect the web UI with a password |
| `--log-level <LEVEL>` | `info` | `error`, `warn`, `info`, `debug`, `trace` |

### Proxy flags

`--proxy-type`, `--proxy-host`, `--proxy-port`, `--proxy-user`, `--proxy-pass`
Supported types: `http`, `http_auth`, `socks4`, `socks5`, `socks5_auth`.

### Examples

```bash
# Seed a file over both BT and WebRTC
BittSeeder --tracker http://tracker.example.com/announce movie.mkv

# BT only, custom port
BittSeeder --protocol bt --port 51413 --tracker udp://tracker.opentrackr.org:1337/announce film.mkv

# WebRTC only, custom ICE, fast polling
BittSeeder --protocol rtc \
  --tracker http://tracker.example.com/announce \
  --ice stun:stun.l.google.com:19302 \
  --rtc-interval 3000 \
  movie.mkv

# Re-seed an existing torrent over both protocols
BittSeeder --torrent-file existing.torrent --tracker http://tracker.example.com/announce /data/movie.mkv

# Multi-file torrent
BittSeeder --name "My Album" --tracker http://tracker.example.com/announce \
  track01.flac track02.flac track03.flac

# With web UI
BittSeeder --protocol both --port 6881 \
  --tracker http://tracker.example.com/announce \
  --web-port 8092 --web-password secret \
  movie.mkv
```

---

## Usage — multi-torrent (YAML)

When no files are given, BittSeeder automatically looks for `config.yaml` in the current directory. Pass `--config` to use a different file:

```bash
# Uses config.yaml in the current directory (default)
BittSeeder

# Explicit config file
BittSeeder --config torrents.yaml

# Override protocol and port from CLI
BittSeeder --config torrents.yaml --protocol bt --port 6881

# Override web UI port (default: 8090)
BittSeeder --config torrents.yaml --web-port 8092
```

If the YAML file does not exist, BittSeeder creates an empty one and waits. The config is hot-reloaded when the file changes on disk, a `SIGHUP` is received (Unix), or the web UI triggers a reload.

### YAML format

```yaml
config:
  listen_port: 6881
  protocol: both                  # bt | rtc | both (default: both)
  rtc_ice_servers:
    - stun:stun.l.google.com:19302
    - stun:stun1.l.google.com:19302
  rtc_interval_ms: 5000
  upnp: false
  web_port: 8092
  web_password: secret
  # Manual TLS (mutually exclusive with Let's Encrypt below)
  # web_cert: /path/to/cert.pem
  # web_key:  /path/to/key.pem
  # Automatic Let's Encrypt TLS (set domain + email to activate)
  # letsencrypt_domain: myserver.example.com
  # letsencrypt_email:  admin@example.com
  # letsencrypt_http_port: 80     # port BittSeeder binds for HTTP-01 challenge
  log_level: info
  show_stats: true
  proxy:
    proxy_type: socks5
    host: 127.0.0.1
    port: 1080
    # username: user
    # password: pass

torrents:
  - name: "My Movie"
    file:
      - /data/movie.mkv
    trackers:
      - http://tracker.example.com/announce
      - udp://tracker.opentrackr.org:1337/announce
    version: v1
    upload_limit: 10240           # KB/s; omit for unlimited
    enabled: true

  - name: "Music Album"
    file:
      - /data/album/track01.flac
      - /data/album/track02.flac
    trackers:
      - http://tracker.example.com/announce
    protocol: rtc                 # override global — RTC only for this torrent
    ice:
      - stun:custom.stun.example.com:3478
    rtc_interval: 3               # seconds (converted to ms internally)
    enabled: true

  - name: "Re-seed from .torrent"
    torrent_file: /data/existing.torrent
    file:
      - /data/existing_content/
    trackers: []                  # read from .torrent file automatically
    enabled: true
```

### Global config keys

| Key | Type | Default | Description |
|---|---|---|---|
| `listen_port` | `u16` | `6881` | BT TCP port shared by all torrents |
| `protocol` | `string` | `both` | Default protocol for all torrents |
| `rtc_ice_servers` | `[string]` | Google STUN x2 | Default ICE server list |
| `rtc_interval_ms` | `u64` | `5000` | Default RTC signaling interval (ms) |
| `upnp` | `bool` | `false` | Enable UPnP port mapping |
| `web_port` | `u16` | `8090` | Web management UI port |
| `web_password` | `string` | — | Web UI password (bearer token auth) |
| `web_cert` | `path` | — | TLS certificate for HTTPS web UI |
| `web_key` | `path` | — | TLS private key for HTTPS web UI |
| `log_level` | `string` | `info` | Log verbosity |
| `show_stats` | `bool` | `true` | Print periodic peer/upload stats to stdout |
| `proxy` | `object` | — | Outbound proxy for tracker announces |
| `web_threads` | `usize` | *(auto)* | Number of actix-web worker threads (omit to let the OS decide) |
| `seeder_threads` | `usize` | *(auto)* | Number of tokio worker threads used by the seeder runtime (omit to use all CPU cores) |
| `source_folder` | `path` | — | Directory scanned by the **Batch Add** feature |
| `letsencrypt_domain` | `string` | — | Domain name for automatic Let's Encrypt TLS certificate |
| `letsencrypt_email` | `string` | — | Contact email registered with the Let's Encrypt account |
| `letsencrypt_http_port` | `u16` | `80` | Port BittSeeder binds to serve the HTTP-01 ACME challenge |

### Torrent entry keys

| Key | Type | Default | Description |
|---|---|---|---|
| `name` | `string` | file name | Torrent display name |
| `file` | `[path]` | — | Files or directories to seed (required unless `torrent_file` is set) |
| `trackers` | `[url]` | `[]` | Tracker announce URLs |
| `torrent_file` | `path` | — | Existing `.torrent` to re-seed |
| `magnet` | `string` | — | Magnet URI (tracker URLs extracted automatically) |
| `out` | `path` | `<name>.torrent` | Where to write the generated `.torrent` |
| `version` | `string` | `v1` | Torrent hash version: `v1`, `v2`, `hybrid` |
| `webseed` | `[url]` | — | Web-seed URLs embedded in the torrent |
| `upload_limit` | `u64` | — | Upload rate cap in KB/s |
| `protocol` | `string` | *(global)* | Per-torrent protocol override: `bt`, `rtc`, `both` |
| `ice` | `[url]` | *(global)* | Per-torrent ICE server list |
| `rtc_interval` | `u64` | *(global)* | Per-torrent RTC signaling interval in **seconds** |
| `enabled` | `bool` | `true` | Set `false` to skip this torrent without removing it |

> **Protocol resolution order:** per-torrent `protocol` → CLI `--protocol` → YAML `config.protocol` → `both`
> **ICE resolution order:** per-torrent `ice` → YAML `config.rtc_ice_servers` → Google STUN x2

---

## Web management UI

The web UI starts automatically on port `8090` (override with `--web-port` or `config.web_port` in YAML). The UI is served at `http://host:<port>/`.

Features:
- Live **Peers** and **Upload Speed** charts (24 h / 48 h / 72 h window)
- Per-torrent uploaded bytes and active peer count, updated every second via WebSocket
- Add, edit, enable/disable, and delete torrents without restarting
- **Batch Add** — scan a configured source folder and register every top-level file/folder as a new torrent entry in one click
- **Upload `.torrent`** — upload an existing `.torrent` file directly from your browser instead of typing a server-side path
- **Upload Files / Folders** — upload any file or entire folder from your browser directly to the server. Files are transferred in chunks with per-chunk SHA-256 validation; a full-file SHA-256 hash check is performed on finalize. Upload progress and hash-verification progress are shown live
- Dark/light theme toggle
- Live **Console** log viewer (last 10 000 lines, streaming via WebSocket)
- Fully responsive — works on desktop, tablet, and mobile

### Authentication

When a `web_password` is configured:

1. On first visit (or after session expiry) a **login modal** is shown.
2. Enter the password; a `POST /api/login` request returns a **bearer token**.
3. The token is stored in `localStorage` as `seeder_token` and sent as `Authorization: Bearer <token>` on every subsequent API request.
4. Sessions expire after **1 hour** of inactivity. Each successful API call resets the timer.
5. The **Logout** button calls `POST /api/logout`, invalidates the server-side session, and returns to the login modal.

When no password is configured the UI is accessible without authentication.

### Password hashing (Argon2ID)

Passwords are stored and verified using **Argon2ID** — they are never stored in plain text. Use the built-in `hash-password` subcommand to generate a hash:

```bash
# Interactive (hidden input, confirmation prompt)
BittSeeder hash-password

# Non-interactive (pipe-friendly)
BittSeeder hash-password mysecretpassword
```

The command prints a PHC-format string such as:

```
$argon2id$v=19$m=19456,t=2,p=1$<salt>$<hash>
```

Store this string as the `web_password` value in your YAML config or pass it directly to `--web-password`:

```yaml
config:
  web_port: 8092
  web_password: "$argon2id$v=19$m=19456,t=2,p=1$<salt>$<hash>"
```

> **Note:** plain-text passwords are still accepted as a fallback for development convenience (any value that does not start with `$argon2` is compared literally). For production use always store a hashed value.

### Endpoints

| Method | Path | Description |
|---|---|---|
| `GET` | `/` | Web management UI (HTML) |
| `GET` | `/ws` | WebSocket — live stats + log stream |
| `POST` | `/api/login` | `{"password":"…"}` → `{"token":"…"}` |
| `POST` | `/api/logout` | Invalidates the current bearer token |
| `GET` | `/api/status` | Live stats: uploaded bytes and peer count per torrent |
| `GET` | `/api/config` | Read global config |
| `PUT` | `/api/config` | Update global config (triggers hot-reload) |
| `GET` | `/api/torrents` | List all torrent entries |
| `POST` | `/api/torrents` | Add a torrent entry |
| `PUT` | `/api/torrents/{idx}` | Replace torrent entry at index |
| `DELETE` | `/api/torrents/{idx}` | Remove torrent entry at index |
| `GET` | `/api/browse?path=…` | Server-side file browser |
| `POST` | `/api/mkdir` | Create a directory on the server: `{"path":"…"}` |
| `POST` | `/api/upload-torrent?name=<filename>` | Upload a `.torrent` file (raw bytes body, ≤ 32 MiB) |
| `POST` | `/api/batch-add` | Scan `source_folder` and bulk-add all untracked top-level entries |
| `POST` | `/api/file-upload/init` | Start a chunked file upload session |
| `POST` | `/api/file-upload/chunk` | Upload one chunk (per-chunk SHA-256 validated before writing) |
| `POST` | `/api/file-upload/finalize` | Finalise upload — full-file SHA-256 verified, file renamed to destination |
| `DELETE` | `/api/file-upload/{upload_id}` | Cancel a chunked upload and remove the partial file |
| `GET` | `/api/file-upload/{upload_id}/hash-progress` | Poll full-file hash verification progress (`bytes_done`, `total`, `percent`) |

---

## Batch Add & torrent upload

### Batch Add

The **Batch Add** button in the web UI calls `POST /api/batch-add`. It reads the `source_folder` value from the global config (set it in **Settings → Network → Batch Add**), then scans every top-level file and directory inside that folder. Any entry that is not already tracked (matched by absolute path) is automatically added as a new torrent entry with:

- `name` — the file/directory name
- `file` — the absolute path
- `trackers` — empty list (fill in afterwards via the edit dialog)
- `enabled` — `true`

Hidden entries (names starting with `.`) are silently skipped. After adding, the config is saved to disk and a hot-reload is triggered.

The response body is:

```json
{ "added": 3, "skipped": 1 }
```

### Torrent file upload

In the **Add Torrent** dialog there is an upload icon next to the `.torrent` browse button. Clicking it opens a native file picker restricted to `.torrent` files. The selected file is sent as a raw binary `POST` to `/api/upload-torrent?name=<filename>` (max 32 MiB). The server saves the file under `<yaml-dir>/uploaded_torrents/<filename>` and returns:

```json
{ "path": "/absolute/path/to/uploaded_torrents/filename.torrent", "name": "filename.torrent" }
```

The returned path is filled into the `.torrent` field of the Add Torrent form automatically, ready to submit.

### File / folder upload

The **Upload Files** button opens an upload modal where you can pick individual files or entire folders to send directly to the server.

**How it works:**

1. The client calls `POST /api/file-upload/init` with the destination path, total file size, number of chunks, chunk size, and the full-file SHA-256 hash computed in the browser.
2. The server pre-allocates the destination file (`<dest>.uploaded`) at the full size (BitTorrent-style allocation).
3. Each chunk is uploaded via `POST /api/file-upload/chunk`. The server validates the chunk's SHA-256 before writing it at the correct byte offset — no write-back verify.
4. After all chunks are sent the client calls `POST /api/file-upload/finalize`. The server streams the entire pre-allocated file through a SHA-256 hash and compares it with the client-supplied hash. On mismatch the partial file is deleted and an error is returned; on success the file is renamed to its final destination path.
5. While finalization runs the client polls `GET /api/file-upload/{upload_id}/hash-progress` every 400 ms and displays a live "Verifying X%" progress bar.

A **Include folder name** toggle (default on) controls whether the top-level folder name is included in the destination path when uploading a folder.

---

## Let's Encrypt auto-certificate

BittSeeder can obtain and renew a [Let's Encrypt](https://letsencrypt.org) TLS certificate automatically using the **ACME HTTP-01** challenge — no manual certificate management required.

### How to enable

Set `letsencrypt_domain` and `letsencrypt_email` in your YAML config (or through **Settings → Security → Let's Encrypt** in the web UI):

```yaml
config:
  web_port: 8443
  letsencrypt_domain: myserver.example.com
  letsencrypt_email:  admin@example.com
  letsencrypt_http_port: 80   # omit to default to 80
```

That is all. Leave `web_cert` and `web_key` unset — BittSeeder fills them in automatically once the certificate is issued.

### What happens

1. **At startup** BittSeeder checks whether `bittseeder.crt` (written next to `config.yaml`) is missing or older than 60 days.
2. If a certificate is needed, BittSeeder creates (or loads) an ACME account stored in `bittseeder-account.key` alongside the config.
3. It starts a temporary HTTP server on `letsencrypt_http_port` (default `80`) to serve the ACME HTTP-01 challenge at `/.well-known/acme-challenge/<token>`.
4. After Let's Encrypt validates the domain, BittSeeder finalises the order, downloads the certificate chain, and writes:
   - `bittseeder.crt` — PEM certificate chain
   - `bittseeder.key` — PEM private key
5. The global config is updated (`web_cert` / `web_key` → these paths) and written back to disk.
6. The web server hot-restarts to serve HTTPS with the new certificate — no manual restart needed.
7. Every **12 hours** BittSeeder repeats the check. If the certificate is still fresh the check is a no-op; if not, it renews automatically.

### Requirements

- The domain must resolve to the machine running BittSeeder.
- Port `80` (or the configured `letsencrypt_http_port`) must be reachable from the internet. If you run BittSeeder behind a reverse proxy, configure the proxy to forward port 80 to the challenge port, or use iptables to redirect it.
- `web_cert` and `web_key` should be left unset when using Let's Encrypt — they are managed automatically.

### Files written next to `config.yaml`

| File | Contents |
|---|---|
| `bittseeder.crt` | PEM-encoded certificate chain (renewed every 60–90 days) |
| `bittseeder.key` | PEM-encoded private key |
| `bittseeder-account.key` | ACME account credentials (JSON) — keep this safe |

### Certificate expiry in the web UI

The **Settings → Security → Let's Encrypt** panel shows a read-only **Certificate Expires** date derived from the certificate file's modification time plus 90 days (the standard Let's Encrypt validity period).

---

## Thread count configuration

BittSeeder runs two independent runtimes whose thread counts can be tuned separately — either in the YAML config or through the web UI **Settings → Performance** tab.

| Config key | Web UI field | What it controls |
|---|---|---|
| `web_threads` | Web threads | Number of actix-web worker threads serving the HTTP/WebSocket API |
| `seeder_threads` | Seeder threads | Number of tokio worker threads in the dedicated seeder runtime |

When a field is left blank (or the YAML key is absent) the runtime uses its default — for actix-web that is the number of logical CPUs; for tokio it is also all logical CPUs.

**On-the-fly changes:**

- **Seeder threads** — applied immediately on every hot-reload. The existing seeder runtime is shut down cleanly and a new one is started with the updated thread count. Active peers are disconnected and reconnect after the seeder restarts (usually within a couple of seconds).
- **Web threads** — applied by hot-restarting the actix-web server. The old server is stopped gracefully (`ServerHandle::stop(true)`) and a new one is spawned with the new worker count. The brief downtime is typically under a second.

---

## Protocol selection reference

| Scenario | `protocol` value | BT listener | RTC signaling |
|---|---|---|---|
| Classic BitTorrent only | `bt` | Yes | No |
| WebRTC only (browser-compatible) | `rtc` | No | Yes |
| Serve both clients simultaneously | `both` | Yes | Yes |

A torrent entry with `protocol: bt` in a `both`-mode YAML session still benefits from the shared BT listener — it just won't make RTC offers. Similarly, a `protocol: rtc` entry skips the BT registry entirely.

---

## Supported BEPs

BittSeeder implements the following [BitTorrent Enhancement Proposals](https://www.bittorrent.org/beps/bep_0000.html):

| BEP | Title | Notes |
|---|---|---|
| [BEP 3](https://www.bittorrent.org/beps/bep_0003.html) | The BitTorrent Protocol | Core wire protocol (handshake, BITFIELD, REQUEST, PIECE, CHOKE/UNCHOKE); HTTP tracker announce/stopped; v1 torrent metainfo format |
| [BEP 9](https://www.bittorrent.org/beps/bep_0009.html) | Extension for Peers to Send Metadata Files | Magnet URI parsing — extracts info hash and tracker URLs from `magnet:?xt=urn:btih:…&tr=…`; peer metadata exchange (ut_metadata) is not implemented |
| [BEP 12](https://www.bittorrent.org/beps/bep_0012.html) | Multitracker Metadata Extension | `announce-list` written to all generated `.torrent` files; all tracker tiers are announced in parallel |
| [BEP 15](https://www.bittorrent.org/beps/bep_0015.html) | UDP Tracker Protocol | Full connect/announce/stopped lifecycle over UDP (`udp://` tracker URLs) |
| [BEP 19](https://www.bittorrent.org/beps/bep_0019.html) | WebSeed (GetRight style) | `url-list` field written to generated `.torrent` files when `--webseed` / `webseed` entries are configured |
| [BEP 23](https://www.bittorrent.org/beps/bep_0023.html) | Tracker Returns Compact Peer Lists | Always requests `compact=1`; parses 6-byte compact IPv4 peer entries (4-byte IP + 2-byte port) |
| [BEP 52](https://www.bittorrent.org/beps/bep_0052.html) | The BitTorrent Protocol v2 | Full v2 torrent creation (SHA-256 piece hashing, per-file Merkle trees, `file tree` info structure); hybrid v1+v2 torrents; v2 magnet links (`xt=urn:btmh:1220…`) |

---

## Client identification

BittSeeder uses the Azureus-style peer ID format:

```
-BS0100-<12 random digits>
```

| Part | Value | Meaning |
|---|---|---|
| `BS` | client code | **B**itt**S**eeder |
| `0100` | version digits | v0.1.0 |
| 12 digits | random | unique per session |

BitTorrent clients that maintain a known-client database (e.g. qBittorrent, Transmission) will display the raw code (`TS`) until BittSeeder is added to their fingerprint database. BittSeeder will **not** be misidentified as any other client.

---

## Architecture overview

```
BittSeeder binary
│
├── config/
│   ├── enums/seed_protocol.rs     SeedProtocol { Bt, Rtc, Both }
│   └── structs/
│       ├── global_config.rs       YAML config: section (all fields)
│       ├── seeder_config.rs       Per-torrent runtime config
│       └── torrent_entry.rs       YAML torrents: entry
│
├── torrent/                       .torrent build + parse (v1/v2/hybrid)
│
├── tracker/
│   ├── structs/bt_client.rs              BtTrackerClient { Http | Udp }
│   ├── structs/rtc_client.rs             RtcTrackerClient (HTTP-only + SDP offer)
│   └── structs/rtc_announce_response.rs  RtcAnnounceResponse
│
├── seeder/
│   ├── seeder.rs                  BT wire handlers + RTC data channel handlers
│   ├── structs/seeder.rs          Seeder { peer_count (BT) + peers (RTC) + … }
│   ├── structs/torrent_registry.rs  Shared BT listener registry
│   ├── structs/peer_conn.rs       WebRTC PeerConnection wrapper
│   └── impls/seeder.rs            run() — concurrent BT+RTC with watch-channel shutdown
│
└── web/
    ├── acme.rs                    ACME HTTP-01 flow — Let's Encrypt auto-certificate
    ├── api.rs                     REST API + WebSocket + bearer token auth
    ├── server.rs                  Actix-web server + optional TLS
    └── index.html                 UI: charts, live log console, torrent management
```

**Concurrency model inside `run()`:**

```
run()
 ├─ tokio::spawn  stats task (every 10 s)
 ├─ tokio::spawn  BT re-announce task   ──┐
 ├─ tokio::spawn  BT TCP accept loop    ──┤─ stopped via watch::channel(true)
 ├─ tokio::spawn  RTC signaling loop    ──┘
 └─ ctrl_c().await  (or external stop signal from web UI)
       └─ stop_tx.send(true)
             ├─ BT stopped announce (all trackers)
             └─ RTC stopped announce (all trackers, inside RTC task)
```

---

## License

[MIT](LICENSE) — © Power2All
