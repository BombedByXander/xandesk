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

The Windows app's **XanDesk relay server** field must use that same URL. It converts HTTPS to WSS automatically. The website and host both initiate outbound internet connections; no router port forwarding or home IP is needed.

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

The default ICE configuration includes public STUN, which can establish direct peer-to-peer connections on many networks. For TURN fallback on restrictive firewalls and NATs, set `ARCDESK_ICE_SERVERS` on the relay service to a JSON list containing your TURN provider's server URLs and credentials. TURN credentials must be treated as server secrets and rotated; do not commit them into this repository. Connectivity cannot be guaranteed if a network blocks both direct WebRTC and the configured TURN routes.

Cloudflare Pages serves the site; Render hosts the long-lived Rust service; Supabase provides accounts and the device table. The relay/API source is in `crates/relay/src/`, and the Windows approval and stable host identity are in `crates/host/src/`.
