# XanDesk — native Rust remote desktop preview

A separate project from the arcade website in the parent directory. The Windows host uses a native `egui` window, DXGI screen capture, OpenH264 encoding, and Rust WebRTC. There is no Electron or embedded browser in the host.

**This is an early functional implementation, not a finished DextraDesk equivalent.** No subscriptions, billing, usage timers, watermarks, or application-enforced FPS ceiling are implemented. Actual output is bounded by display updates, CPU encoding speed, network capacity, decoder speed, and the browser display. Lossless quality or a game's internal FPS cannot be promised.

## Implemented

- Native Windows host with Start/Stop, local approval or opt-in away access, bitrate selection, capture/encode FPS, and a per-run 12-character pairing code.
- Primary-monitor desktop capture and H.264 over encrypted WebRTC.
- Browser keyboard, mouse, scroll, relative/game mouse, touch pointer, full-screen viewing, and optional browser-to-PC text clipboard.
- Measured browser decode FPS, incoming video bitrate, WebRTC round-trip time, and received resolution. These are measurements, not advertised targets. Static desktops may report very low FPS.
- Rust WebSocket signaling server. Single viewer per PC. Random 256-bit pairing keys, hashed server-side, constant-time comparison, origin allowlist, message-size limits, approval gate, connection limits, authentication timeout, and liveness checks.
- Key/button release on blur, disconnect, host stop, or missing input heartbeats.
- Configurable STUN and TURN. The signaling server does not receive video, audio, or input. A TURN server, when configured, forwards encrypted WebRTC packets.

## Important parity gaps

This preview does **not** yet implement system audio, virtual game controllers, file transfer, reverse clipboard synchronization, multi-monitor switching, GPU hardware encoding, automatic bandwidth/resolution adaptation, persistent account/device management, MFA, a Windows service, Windows lock/sign-in/UAC secure-desktop control, auto-start, code signing, a signed installer, or automatic updates. It requires an unlocked Windows desktop. By default each connection needs approval at the PC; select **Allow my private pairing link to connect while I’m away** before starting sharing to allow anyone with that PC's active pairing code to connect without another local prompt. Elevated apps and some games may reject synthetic input. Desktop capture currently does not composite a separately rendered hardware cursor.

These are real engineering tasks, not switches that can be enabled in Cloudflare Pages. DextraDesk's public feature list was reviewed at <https://dextradesk.com/>. XanDesk has its own name and UI and does not reuse DextraDesk branding or code.

## Build on Windows

Install current stable Rust and the Visual Studio C++ build tools plus a Windows SDK. The checked-in `Cargo.lock` fixes the dependency versions. A portable GNU toolchain is also supported by `scripts/build.ps1`; on this development machine it uses `E:\XanDeskBuild` to keep build caches off the full C: drive.

```powershell
cd arcdesk
.\scripts\build.ps1
```

The resulting `dist` folder contains `XanDesk.exe`, `arcdesk-relay.exe`, and any GNU runtime DLLs. Keep those DLLs alongside the executables. This is a portable unsigned preview, not a signed installer. For a standard MSVC build, `cargo build --release --workspace --locked` also works. No Python, Node, FFmpeg, or Electron runtime is required to run the compiled apps.

## Local test

1. Run `arcdesk-relay.exe`. It listens only on `127.0.0.1:8787` by default.
2. Run `XanDesk.exe`, enter `http://localhost:8787` as the relay, and click **Start sharing**.
3. Enter the 12-character pairing code shown in the host window into the website in Chrome or Edge on the same PC.
4. Approve the connection in the native host, or opt into away access before starting sharing. The browser displays the desktop. Opening it on the captured monitor creates the expected screen-within-screen effect.
5. Click **Stop sharing & disconnect** in the host to end access and invalidate the pairing code. Closing the host also ends sharing.

`localhost` on another device refers to that other device. Internet access requires the deployment below. Do not simply copy a localhost invitation to a Chromebook and expect it to connect.

## Cloudflare Pages and backend deployment

Deploy the Rust signaling server on infrastructure supporting persistent WebSockets, behind an HTTPS reverse proxy. This is separate from the XanDesk Pages frontend. HTTPS/WSS is required for remote connections; only loopback development permits HTTP. Terminate TLS on the server/proxy you control and keep pairing codes out of request logs. The code is shown only while the host is sharing and is invalidated when sharing stops.

Environment variables for `arcdesk-relay`:

| Variable | Example / purpose |
| --- | --- |
| `ARCDESK_BIND` | `127.0.0.1:8787` behind a proxy, or `0.0.0.0:8787` inside a private container network |
| `ARCDESK_ORIGINS` | Comma-separated exact frontend origins, e.g. `https://desk.example.com,https://your-project.pages.dev` |
| `ARCDESK_ICE_SERVERS` | JSON array of WebRTC ICE server objects, shown below |

```json
[
  {"urls":["stun:stun.l.google.com:19302"]},
  {"urls":["turn:turn.example.com:3478?transport=udp","turns:turn.example.com:5349?transport=tcp"],"username":"replace-me","credential":"replace-me"}
]
```

The default only includes public STUN. **It will not connect through every NAT/firewall.** Provide your own TURN service for restrictive networks and test it from a genuinely separate network. The current config uses static TURN credentials; before opening this as a public service, add authenticated short-lived TURN credentials, per-IP abuse protection at the reverse proxy, operational monitoring, and account authentication. Pairing codes stay active while sharing is active and should be treated as secrets. Hosting and relay bandwidth have infrastructure costs even when your product charges users nothing.

Example Caddy configuration:

```caddy
desk.example.com {
    reverse_proxy 127.0.0.1:8787
}
```

Set the host's connection server to `https://desk.example.com`. Pairing links will then point to the viewer served by that server. The host initiates outbound signaling and WebRTC connectivity; it does not expose an unauthenticated HTTP input endpoint on the PC.

## Cloudflare Pages integration

The XanDesk Pages frontend is in `web/`. Connect the repository with project root `arcdesk/web` and no build command. Configure the Pages variable `XANDESK_RELAY` with the public HTTPS origin of the Rust relay. The Windows host and website must use that same public relay for pairing codes to work across networks. Read [SETUP-PAGES.md](SETUP-PAGES.md) for deployment details and the separate account/device API contract. Persistent account auth, device enrollment, and session authorization endpoints are not implemented by the current Rust relay.

## Verification

```powershell
cargo test --workspace --locked
node --check web/client.js
node scripts/test-relay.mjs .\dist\arcdesk-relay.exe
```

The relay integration test needs Node 22+ and uses its built-in WebSocket client. It checks authentication failures, room ownership, local approval gating, concurrent-viewer rejection, SDP routing, reject/disconnect cleanup, and response headers. It does not prove real-world gaming latency, audio, controller compatibility, or cross-network TURN traversal. End-to-end capture and browser decoding require running the native host and approving a session.

## Architecture

```text
Windows host (Rust / egui) -- WSS -- Rust signaling -- WSS -- browser / Cloudflare Pages
       |                                                     |
       +--------- encrypted WebRTC video + input ------------+
                     direct, or via your TURN server
```

Captured frames are only encoded when the transport has room. The capture thread waits briefly only when DXGI has no new frame; there is no 60/120 FPS pacing timer. H.264 keyframes are refreshed periodically and requested on packet-loss feedback. Encoded reference frames are never intentionally dropped from the sending queue. The first software encoder prioritizes implementation portability and must be benchmarked before using this for high-refresh gaming.


