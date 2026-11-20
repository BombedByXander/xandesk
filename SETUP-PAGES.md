# Deploy XanDesk on Cloudflare Pages

## Frontend

In Cloudflare Pages, connect this repository and set the project root to `arcdesk/web`. Use no build command; set the output directory to `.`. The site is static HTML/CSS/JavaScript and includes a Pages `wrangler.toml` plus security `_headers`. Alternatively, upload the contents of `arcdesk/web` as a Direct Upload. Set `XANDESK_API` in the Pages project's environment variables to your HTTPS API origin and deploy again.

The browser app expects a backend implementing the contracts below. Until that service is deployed, account pages show a real API-configuration error and do not fabricate devices or sessions. The standalone private pairing viewer still connects to the existing Rust host/relay protocol.

## Required account and device backend

Implement these routes over HTTPS. Return JSON errors as `{"message":"..."}`. Use short-lived bearer tokens; do not put tokens or passwords in URLs or browser persistent storage. Restrict CORS to the exact Pages origin. Add CSRF defenses if authentication switches to cookies.

| Method and path | Request / response |
|---|---|
| `POST /api/v1/auth/register` | `{name,email,password}` → `{accessToken,user}` |
| `POST /api/v1/auth/login` | `{email,password}` → `{accessToken,user}` |
| `GET /api/v1/devices` | Bearer token → `{devices:[{id,name,os,online,lastSeen}]}` |
| `POST /api/v1/devices/pairing` | Bearer token → `{pairingCode,expiresAt}`; one-time code, expire promptly |
| `PATCH /api/v1/devices/{id}` | `{name}` → updated device |
| `DELETE /api/v1/devices/{id}` | revoke enrollment and terminate active sessions |
| `POST /api/v1/devices/{id}/sessions` | `{}` → `{sessionId,signalingUrl,iceServers}` with short-lived session authorization |

For authenticated sessions, `signalingUrl` accepts a bearer-authenticated offer containing `{type:"offer",sessionId,sdp}` and returns `{sdp}`. It must authorize the host and viewer against the same account/device/session, route signaling only after authorization, and provide STUN plus expiring TURN credentials. ICE policy `all` prefers direct routes and automatically allows TURN candidates as fallback. The host must use the same authenticated enrollment and signaling protocol. This API and host integration do not exist in the current Rust pairing backend; the private pairing link is the functional preview path today.

## Remote networking

The account/session service must be publicly reachable from the host and browser. Operate signaling over WSS. Configure STUN and a TURN service reachable over UDP and TCP/TLS. Use time-limited TURN credentials, rate limiting, account/device authorization, monitoring, and bandwidth limits. The browser and host need no port forwarding, home IP address or VPN. No service can guarantee connectivity where a network blocks every allowed path. The current host's account enrollment and cross-network TURN path need server integration and separate-network validation before claiming worldwide production connectivity.
