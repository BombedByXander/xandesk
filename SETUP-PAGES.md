# XanDesk website and API deployment

The site is hosted on Cloudflare Pages. The account/device API and WebRTC signaling run together in the Rust `arcdesk-relay` service, because Pages Functions cannot keep the host's persistent WebSocket connection. The same public relay URL is used for `XANDESK_API` and the Windows host's relay address.

## One-time Supabase setup

1. Open the Supabase project SQL Editor.
2. Run the SQL in `supabase/migrations/20261119000000_xandesk_devices.sql`.
3. In Authentication settings, enable the Email provider. Set the Site URL to the published Pages origin and allow that origin with `/**` for email confirmation redirects. Keep email confirmation enabled if desired.

The migration creates an owner-protected `xandesk_devices` table. The API verifies the user's Supabase access token and uses that same token with Row Level Security for device records. It does not require a Supabase service-role key.

## Deploy the relay and API

This repo includes a `render.yaml` Blueprint and `Dockerfile` for the Rust relay/API.

1. In Render, create a Blueprint from `BombedByXander/xandesk` and select the repository's root `render.yaml`.
2. Wait for the `xandesk-relay` service to deploy. Its public HTTPS service URL is also the WebSocket relay URL.
3. In Render, make sure `ARCDESK_ORIGINS` includes the exact XanDesk Pages origin. Add your custom site origin too if you use one.
4. The service reads its own public URL from Render, so no relay URL secret needs to be entered there.

The Render service uses your Supabase project URL and publishable key, which are public client settings. It does not expose or need a service-role key.

## Point Cloudflare Pages at the API

For the Pages Git project, use:

- **Root directory:** `web`
- **Build command:** leave blank
- **Build output directory:** `.`

In Pages project settings, add the environment variable `XANDESK_API` with the relay service's public HTTPS URL, for example `https://xandesk-relay.onrender.com` (use the exact URL Render assigned). Save and redeploy. The Pages config Function reads this setting for device API calls and WebRTC signaling.

The Windows host already uses the deployed XanDesk relay URL. The website and host both initiate outbound internet connections; no router port forwarding or home IP is needed.

For local Wrangler deployment, run `npx wrangler pages deploy . --project-name xandesk` from the `web` directory. This deploys the Pages Function along with the static site; Cloudflare drag-and-drop upload does not include Functions.

## API provided

The Rust relay implements:

| Route | Behavior |
|---|---|
| `GET /api/v1/devices` | Lists only the signed-in user's paired computers and their live online status |
| `POST /api/v1/devices/pairing/claim` | Validates the current host pairing code and waits for approval in the Windows app before saving ownership |
| `PATCH /api/v1/devices/{id}` | Renames a computer owned by the signed-in user |
| `DELETE /api/v1/devices/{id}` | Removes that account's enrollment and terminates its active viewer |
| `POST /api/v1/devices/{id}/sessions` | Issues a short-lived, one-use WebSocket join token for an online computer the user owns |

The host identity is stored locally so a computer keeps its account association between host restarts. Pairing codes are still temporary and must be approved from the host. Supabase validates user sessions; database Row Level Security protects each device record. The relay keeps active connections in memory, so run one always-on relay instance rather than scaling it horizontally.

## Network connectivity

The default ICE configuration includes public STUN, which can establish direct peer-to-peer connections on many networks. STUN alone cannot traverse every NAT or school/public firewall, so add TURN for reliable access across restrictive networks.

For managed Cloudflare TURN:

1. In Cloudflare, create a Realtime TURN key.
2. Create a scoped API token with **Account → Cloudflare Realtime → Edit** permission.
3. In Render, set `CLOUDFLARE_TURN_KEY_ID` to the key ID and `CLOUDFLARE_TURN_API_TOKEN` to the token. Keep the token private and never put it in Pages or the Windows app.
4. Redeploy the relay. The server requests short-lived TURN credentials and passes them to the host and browser; TURN is tried only when a direct route fails.

If those variables are empty, XanDesk falls back to public STUN only. The host status and session error message identify that direct-only mode. Cloudflare Realtime TURN includes a 1,000 GB monthly free tier, then charges for relayed outbound traffic; direct peer-to-peer traffic does not use TURN bandwidth. Connectivity still depends on the network allowing at least one configured ICE route.

Cloudflare Pages serves the site; Render hosts the long-lived Rust service; Supabase provides accounts and the device table. The relay/API source is in `crates/relay/src/`, and the Windows approval and stable host identity are in `crates/host/src/`.
