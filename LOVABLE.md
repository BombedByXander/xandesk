# Connecting a Lovable frontend to ArcDesk

Copy this into Lovable when you are ready to build the frontend:

> Build an original remote-desktop frontend called ArcDesk. It connects to my existing native Rust Windows host through my Rust signaling server. Do not use Electron, mock video, fabricated FPS counters, pricing tiers, session timers, or payment screens. Use a responsive dark interface with pale mint accents. Provide a connection form, local-approval waiting state, real remote video, live decoded FPS, bitrate, round-trip time and resolution, fullscreen, disconnect, keyboard/mouse controls and game mouse pointer lock. Read the supplied `web/client.js` and implement the same WebSocket/WebRTC protocol. Keep pairing secrets in memory, never analytics or localStorage. On mobile use pointer events and an explicit keyboard/text input. Show genuine connection errors. System audio, controller emulation, file transfer, and account/device pairing are not available in the current host; do not create fake controls for them.

Attach `web/client.js`, `web/index.html`, and this file. Replace the server default with the HTTPS origin of your deployed Rust relay. Add the exact Lovable frontend origin to `ARCDESK_ORIGINS` on the relay. The Rust server must run on a separate host with WebSocket support; a static frontend alone cannot run it.

## Protocol version 0.1

Connect to `wss://YOUR-RELAY/ws`. JSON messages use snake-case `type` values. The native host registers a random 32-hex PC code and 64-hex secret. A browser submits:

```json
{"type":"join","room":"32-hex-pc-code","token":"64-hex-private-key"}
```

The host sees an approval request, unless its owner explicitly enabled away access before sharing. Until the host approves, no SDP is forwarded. On approval the viewer receives:

```json
{"type":"ready","ice_servers":[{"urls":["stun:stun.l.google.com:19302"],"username":"","credential":""}]}
```

Create an `RTCPeerConnection` with those ICE servers, add a receive-only video transceiver, and create one reliable ordered data channel named `control`. Create an offer, set it locally, wait for ICE gathering to complete, then send `{type:"offer",sdp:pc.localDescription.sdp}`. The response is `{type:"answer",sdp:"..."}`; apply it as the remote description. This version uses complete SDP, not trickled ICE. Do not send partial SDP and assume later ICE candidates will be accepted.

Handle `reject`, `left`, `error` and WebSocket/WebRTC disconnects by closing the peer and clearing all inputs. Do not retry an old approved session silently. Every new viewer connection is authorized again by the host, either through a local prompt or the owner's previously selected away-access setting.

Data-channel packets:

```js
{type:'move', x:0.5, y:0.5}                 // normalized within actual video, excluding letterboxing
{type:'relative', x:5, y:-2}                 // pointer-lock deltas
{type:'button', button:0, down:true}          // 0 left, 1 middle, 2 right
{type:'key', code:'KeyW', down:true}          // KeyboardEvent.code, physical key
{type:'wheel', delta:120}                    // positive scrolls up
{type:'release'}                            // all keys/buttons up; send on blur and disconnect
{type:'heartbeat'}                          // once per second; stale inputs release after 3 sec
{type:'clipboard', text:'Hello'}             // only if host opted in; <=64 KB UTF-8
```

Use `getStats()` inbound-rtp `framesDecoded` differences divided by actual timestamp differences to compute FPS. Use `bytesReceived` differences for Mbps and the nominated candidate pair's `currentRoundTripTime` for RTT. RTT is **not** glass-to-glass latency. A browser cannot read GTA's internal FPS from the video stream.

Before adding persistent user accounts, implement authenticated device enrollment and viewer authorization on the server, owner-driven revocation, and short-lived session grants. Replacing the pairing form with a cosmetic login screen does not implement authentication.
