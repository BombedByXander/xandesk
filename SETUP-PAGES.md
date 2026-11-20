# Deploy XanDesk on Cloudflare Pages

## Frontend

In Cloudflare Pages, connect this repository and set the project root to `arcdesk/web`. Use no build command; set the output directory to `.`. The site is static HTML/CSS/JavaScript and includes a Pages `wrangler.toml`, a Pages Function at `functions/api/config.js`, and security `_headers`. The supplied Supabase project URL and publishable key are configured in `wrangler.toml` and exposed by that Function for browser auth. Set `XANDESK_API` to your HTTPS account/device API when it is deployed; that separate API is required for pairing, device management, and session launch. The Windows host reads `XANDESK_SITE_URL` (defaults to `https://xandesk.pages.dev`) when opening the owner dashboard from its tray menu.

In Supabase Authentication settings, enable the Email provider. Set the Site URL to your published Pages origin (for example `https://xandesk.pages.dev`) and add that origin's `/**` path to the redirect URL allow-list. Add your custom domain too if you use one. Email confirmation can stay enabled; new accounts then need to confirm their email before signing in. The app uses Supabase Auth directly for account creation, password sign-in, token refresh, and sign-out. Only the project's public publishable key is sent to the browser; never put a Supabase secret/service-role key in Pages frontend configuration.

To deploy from the command line instead, install Wrangler with `npm install -g wrangler`, authenticate with `npx wrangler login`, then run `npx wrangler pages deploy . --project-name xandesk` from `arcdesk/web`. Wrangler deploys the Pages Function with the static files. The Cloudflare dashboard's drag-and-drop Direct Upload does not deploy the `functions/` directory.

The browser app authenticates accounts with Supabase Auth. It separately expects a XanDesk API implementing the device/session contracts below. Until that service is deployed, the app can create accounts and sign in, but device pages show a real API-configuration error and do not fabricate devices or sessions. The PC owner signs in, submits the host's 12-character pairing code using **Add PC**, then approves enrollment on the host. After enrollment, an authorized peer signs in and chooses that PC from the dashboard; peers do not use the pairing code to view the stream.

Cloudflare Pages only serves the frontend and its small configuration Function. It does not run this Rust WebSocket relay or the account API. Deploy `arcdesk-relay` on a public server with a stable hostname and HTTPS/WSS; set the Windows app's relay address to that hostname and `ARCDESK_ORIGINS` to include the exact Pages site origin. The account API must verify a code against the live host, send an enrollment request through the relay, wait for the host's Allow response, and store an account-bound device authorization. Session creation must then authorize the viewer against that account-bound device. This account/host enrollment and session backend is not implemented in the current Rust preview. For a same-PC streaming test, run `arcdesk-relay.exe`, set the host address to `http://localhost:8787`, and open the local relay site on that same PC.

## Required account and device backend

Implement these device routes over HTTPS. Return JSON errors as `{"message":"..."}`. Validate the Supabase access token server-side against this Supabase project before each action; do not put tokens or passwords in URLs. Restrict CORS to the exact Pages origin. Add CSRF defenses if authentication switches to cookies.

| Method and path | Request / response |
|---|---|
| `GET /api/v1/devices` | Supabase access token in `Authorization: Bearer <token>` → `{devices:[{id,name,os,online,lastSeen}]}` |
| `POST /api/v1/devices/pairing/claim` | Supabase bearer token + `{pairingCode}` → `{device:{id,name,os,online}}`; validate the live host code and require the host's Allow approval before claiming it to this account |
| `PATCH /api/v1/devices/{id}` | `{name}` → updated device |
| `DELETE /api/v1/devices/{id}` | revoke enrollment and terminate active sessions |
| `POST /api/v1/devices/{id}/sessions` | `{}` → `{sessionId,signalingUrl,iceServers}` with short-lived session authorization |

For authenticated sessions, `signalingUrl` accepts a bearer-authenticated offer containing `{type:"offer",sessionId,sdp}` and returns `{sdp}`. It must authorize the host and viewer against the same account/device/session, route signaling only after authorization, and provide STUN plus expiring TURN credentials. ICE policy `all` prefers direct routes and automatically allows TURN candidates as fallback. The host must use the same authenticated enrollment and signaling protocol. This account API and host integration do not exist in the current Rust pairing backend; its functional preview flow uses the temporary pairing code.

## Remote networking

The account/session service must be publicly reachable from the host and browser. Operate signaling over WSS. Configure STUN and a TURN service reachable over UDP and TCP/TLS. Use time-limited TURN credentials, rate limiting, account/device authorization, monitoring, and bandwidth limits. The browser and host need no port forwarding, home IP address or VPN. No service can guarantee connectivity where a network blocks every allowed path. The current host's account enrollment and cross-network TURN path need server integration and separate-network validation before claiming worldwide production connectivity.
