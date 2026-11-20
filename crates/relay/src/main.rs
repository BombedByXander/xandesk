mod api;

use arcdesk_protocol::{valid_credentials, IceServer, Signal};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::{header, HeaderMap, Method, StatusCode},
    response::{Html, IntoResponse},
    routing::{get, patch, post},
    Router,
};
use futures_util::{SinkExt, StreamExt};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::Arc, time::Duration};
use subtle::ConstantTimeEq;
use tokio::sync::{mpsc, oneshot, Mutex, Semaphore};
use tower_http::cors::{AllowOrigin, CorsLayer};

type Tx = mpsc::Sender<Signal>;
pub(crate) struct Room {
    pub(crate) digest: [u8; 32],
    pub(crate) pairing_digest: Option<[u8; 32]>,
    pub(crate) host: Tx,
    pub(crate) viewer: Option<Tx>,
    pub(crate) approved: bool,
    pub(crate) viewer_tokens: HashMap<[u8; 32], tokio::time::Instant>,
    pub(crate) pair_claim: Option<oneshot::Sender<bool>>,
    pub(crate) pair_claim_approved: bool,
    pub(crate) device_name: String,
    pub(crate) ice_servers: Vec<IceServer>,
}
pub(crate) struct App {
    pub(crate) rooms: Mutex<HashMap<String, Room>>,
    pub(crate) ice: Vec<IceServer>,
    pub(crate) turn_key_id: String,
    pub(crate) turn_api_token: String,
    pub(crate) origins: Vec<String>,
    pub(crate) supabase_url: String,
    pub(crate) supabase_publishable_key: String,
    pub(crate) public_url: String,
    pub(crate) http: reqwest::Client,
    slots: Arc<Semaphore>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct TurnCredentials {
    ice_servers: Vec<IceServer>,
}

impl App {
    pub(crate) async fn issue_ice_servers(&self) -> anyhow::Result<Vec<IceServer>> {
        if self.turn_key_id.is_empty() && self.turn_api_token.is_empty() {
            return Ok(self.ice.clone());
        }
        anyhow::ensure!(
            !self.turn_key_id.is_empty() && !self.turn_api_token.is_empty(),
            "Both CLOUDFLARE_TURN_KEY_ID and CLOUDFLARE_TURN_API_TOKEN are required"
        );
        let response = self.http
            .post(format!(
                "https://rtc.live.cloudflare.com/v1/turn/keys/{}/credentials/generate-ice-servers",
                self.turn_key_id
            ))
            .bearer_auth(&self.turn_api_token)
            .json(&serde_json::json!({"ttl": 172800}))
            .send()
            .await?;
        anyhow::ensure!(
            response.status().is_success(),
            "Cloudflare TURN credential request returned HTTP {}",
            response.status()
        );
        let credentials: TurnCredentials = response.json().await?;
        anyhow::ensure!(
            credentials.ice_servers.iter().any(|server| server.urls.iter().any(|url| url.starts_with("turn:" ) || url.starts_with("turns:"))),
            "Cloudflare returned no TURN servers"
        );
        Ok(credentials.ice_servers)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("arcdesk_relay=info")
        .init();
    let ice = match std::env::var("ARCDESK_ICE_SERVERS") {
        Ok(json) => serde_json::from_str(&json)?,
        Err(_) => vec![IceServer {
            urls: vec!["stun:stun.l.google.com:19302".into()],
            ..Default::default()
        }],
    };
    let origins = std::env::var("ARCDESK_ORIGINS")
        .unwrap_or_else(|_| "http://localhost:8787,http://127.0.0.1:8787".into())
        .split(',')
        .map(|s| s.trim().to_owned())
        .collect();
    let supabase_url = std::env::var("XANDESK_SUPABASE_URL")
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_owned();
    let supabase_publishable_key =
        std::env::var("XANDESK_SUPABASE_PUBLISHABLE_KEY").unwrap_or_default();
    let public_url = std::env::var("XANDESK_RELAY_PUBLIC_URL")
        .or_else(|_| std::env::var("RENDER_EXTERNAL_URL"))
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_owned();
    let state = Arc::new(App {
        rooms: Mutex::new(HashMap::new()),
        ice,
        turn_key_id: std::env::var("CLOUDFLARE_TURN_KEY_ID").unwrap_or_default(),
        turn_api_token: std::env::var("CLOUDFLARE_TURN_API_TOKEN").unwrap_or_default(),
        origins,
        supabase_url,
        supabase_publishable_key,
        public_url,
        http: reqwest::Client::new(),
        slots: Arc::new(Semaphore::new(256)),
    });
    let cors_origins = state
        .origins
        .iter()
        .filter_map(|origin| origin.parse().ok())
        .collect::<Vec<_>>();
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(cors_origins))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);
    let app = Router::new()
        .route("/", get(|| async { Html(include_str!("../../../web/index.html")) }))
        .route("/client.js", get(|| async { ([("content-type", "text/javascript; charset=utf-8")], include_str!("../../../web/client.js")) }))
        .route("/app.js", get(|| async { ([("content-type", "text/javascript; charset=utf-8")], include_str!("../../../web/app.js")) }))
        .route("/viewer.html", get(|| async { Html(include_str!("../../../web/viewer.html")) }))
        .route("/style.css", get(|| async { ([("content-type", "text/css; charset=utf-8")], include_str!("../../../web/style.css")) }))
        .route("/health", get(|| async { "ok" }))
        .route("/ws", get(upgrade))
        .route("/api/v1/devices", get(api::list_devices))
        .route("/api/v1/devices/pairing/claim", post(api::claim_device))
        .route("/api/v1/devices/{id}", patch(api::rename_device).delete(api::remove_device))
        .route("/api/v1/devices/{id}/sessions", post(api::create_session))
        .layer(cors)
        .layer(axum::middleware::from_fn(|req, next: axum::middleware::Next| async move {
            let mut response = next.run(req).await;
            let h = response.headers_mut();
            h.insert("referrer-policy", "no-referrer".parse().unwrap());
            h.insert("x-content-type-options", "nosniff".parse().unwrap());
            h.insert("cache-control", "no-store".parse().unwrap());
            h.insert("content-security-policy", "default-src 'self'; connect-src 'self' ws: wss:; script-src 'self'; style-src 'self' 'unsafe-inline'; media-src 'self' blob:; frame-ancestors 'none'; base-uri 'none'".parse().unwrap());
            response
        }))
        .with_state(state);
    let address = std::env::var("ARCDESK_BIND").unwrap_or_else(|_| "127.0.0.1:8787".into());
    let listener = tokio::net::TcpListener::bind(&address).await?;
    tracing::info!(%address, "XanDesk signaling server ready");
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}

async fn upgrade(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> axum::response::Response {
    if let Some(origin) = headers.get("origin") {
        if !origin
            .to_str()
            .is_ok_and(|o| app.origins.iter().any(|allowed| allowed == o))
        {
            return StatusCode::FORBIDDEN.into_response();
        }
    }
    let Ok(permit) = app.slots.clone().try_acquire_owned() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    ws.max_message_size(128 * 1024)
        .max_frame_size(128 * 1024)
        .on_upgrade(move |socket| async move {
            let _permit = permit;
            connection(socket, app).await;
        })
}

async fn send(socket: &mut WebSocket, signal: &Signal) -> bool {
    socket
        .send(Message::Text(serde_json::to_string(signal).unwrap().into()))
        .await
        .is_ok()
}

async fn connection(mut socket: WebSocket, app: Arc<App>) {
    let first = tokio::time::timeout(Duration::from_secs(10), socket.recv()).await;
    let Ok(Some(Ok(Message::Text(text)))) = first else {
        return;
    };
    let Ok(signal) = serde_json::from_str::<Signal>(&text) else {
        return;
    };
    let (room_id, token, is_host, pairing_code, device_name) = match signal {
        Signal::Register {
            room,
            token,
            pairing_code,
            device_name,
        } => (room, token, true, Some(pairing_code), device_name),
        Signal::Join { room, token } => (room, token, false, None, String::new()),
        _ => return,
    };
    let pairing_code_valid = pairing_code
        .as_ref()
        .and_then(|code| normalize_pairing_code(code))
        .is_some();
    let has_pairing_code = pairing_code.as_ref().is_some_and(|code| !code.is_empty());
    if (is_host && has_pairing_code && !pairing_code_valid) || !valid_credentials(&room_id, &token)
    {
        return;
    }
    let host_ice = if is_host {
        match app.issue_ice_servers().await {
            Ok(servers) => Some(servers),
            Err(error) => {
                send(&mut socket, &Signal::Error {
                    message: format!("Could not configure WebRTC relay servers: {error:#}"),
                }).await;
                return;
            }
        }
    } else {
        None
    };
    let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    let (tx, mut rx) = mpsc::channel(16);
    let registered = {
        let mut rooms = app.rooms.lock().await;
        if is_host {
            if rooms.contains_key(&room_id) {
                false
            } else {
                let pairing_digest: Option<[u8; 32]> = pairing_code.as_ref().and_then(|code| {
                    normalize_pairing_code(code)
                        .map(|normalized| Sha256::digest(normalized.as_bytes()).into())
                });
                if pairing_digest.as_ref().is_some_and(|candidate| {
                    rooms.values().any(|room| {
                        room.pairing_digest
                            .as_ref()
                            .is_some_and(|known| bool::from(known.ct_eq(candidate)))
                    })
                }) {
                    return;
                }
                rooms.insert(
                    room_id.clone(),
                    Room {
                        digest,
                        pairing_digest,
                        host: tx.clone(),
                        viewer: None,
                        approved: false,
                        viewer_tokens: HashMap::new(),
                        pair_claim: None,
                        pair_claim_approved: false,
                        device_name,
                        ice_servers: host_ice.clone().unwrap_or_default(),
                    },
                );
                true
            }
        } else if let Some(room) = rooms.get_mut(&room_id) {
            let (authorized, account_authorized) = if token.is_empty() {
                (pairing_code
                    .as_ref()
                    .and_then(|code| normalize_pairing_code(code))
                    .and_then(|code| {
                        let presented: [u8; 32] = Sha256::digest(code.as_bytes()).into();
                        room.pairing_digest
                            .as_ref()
                            .map(|known| bool::from(known.ct_eq(&presented)))
                    })
                    .unwrap_or(false), false)
            } else if bool::from(room.digest.ct_eq(&digest)) {
                (true, false)
            } else {
                let now = tokio::time::Instant::now();
                room.viewer_tokens.retain(|_, expiry| *expiry > now);
                (room.viewer_tokens.remove(&digest).is_some(), true)
            };
            if authorized && room.viewer.is_none() && room.host.try_send(Signal::Request { account_authorized }).is_ok() {
                room.viewer = Some(tx.clone());
                room.approved = false;
                true
            } else {
                false
            }
        } else {
            false
        }
    };
    if !registered {
        send(
            &mut socket,
            &Signal::Error {
                message: "Host unavailable, busy, or pairing details invalid.".into(),
            },
        )
        .await;
        return;
    }
    if is_host {
        let _ = tx.try_send(Signal::Registered {
            ice_servers: host_ice.unwrap_or_default(),
        });
    }
    let (mut sink, mut source) = socket.split();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(20));
    let mut last_seen = tokio::time::Instant::now();
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if last_seen.elapsed() > Duration::from_secs(60) { break; }
                if sink.send(Message::Ping(vec![].into())).await.is_err() { break; }
            }
            outgoing = rx.recv() => {
                let Some(outgoing) = outgoing else { break };
                let terminal = matches!(outgoing, Signal::Reject | Signal::Left | Signal::Error { .. });
                if sink.send(Message::Text(serde_json::to_string(&outgoing).unwrap().into())).await.is_err() { break; }
                if terminal && !is_host { break; }
            }
            incoming = source.next() => {
                let Some(Ok(incoming)) = incoming else { break };
                last_seen = tokio::time::Instant::now();
                let Message::Text(text) = incoming else {
                    if matches!(incoming, Message::Close(_)) { break; }
                    continue;
                };
                let Ok(signal) = serde_json::from_str::<Signal>(&text) else { break };
                let mut rooms = app.rooms.lock().await;
                let Some(room) = rooms.get_mut(&room_id) else { break };
                let target_and_message = match (is_host, signal) {
                    (true, Signal::Approve) if room.viewer.is_some() && !room.approved => {
                        room.approved = true;
                        Some((room.viewer.as_ref().unwrap().clone(), Signal::Ready { ice_servers: room.ice_servers.clone() }))
                    }
                    (true, Signal::Reject) => room.viewer.as_ref().map(|v| (v.clone(), Signal::Reject)),
                    (true, Signal::PairApprove) if room.pair_claim.is_some() => {
                        room.pair_claim_approved = true;
                        if let Some(waiter) = room.pair_claim.take() { let _ = waiter.send(true); }
                        None
                    }
                    (true, Signal::PairReject) if room.pair_claim.is_some() => {
                        if let Some(waiter) = room.pair_claim.take() { let _ = waiter.send(false); }
                        None
                    }
                    (true, Signal::Answer { sdp }) if room.approved => room.viewer.as_ref().map(|v| (v.clone(), Signal::Answer { sdp })),
                    (false, Signal::Offer { sdp }) if room.approved => Some((room.host.clone(), Signal::Offer { sdp })),
                    // Host application heartbeats keep idle WebSocket sessions
                    // active on platforms that track inbound application data.
                    (_, Signal::Heartbeat) => None,
                    _ => None,
                };
                if let Some((target, signal)) = target_and_message {
                    if target.try_send(signal).is_err() { break; }
                }
            }
        }
    }
    let mut rooms = app.rooms.lock().await;
    if is_host {
        if let Some(room) = rooms.remove(&room_id) {
            if let Some(viewer) = room.viewer {
                let _ = viewer.try_send(Signal::Left);
            }
        }
    } else if let Some(room) = rooms.get_mut(&room_id) {
        room.viewer = None;
        room.approved = false;
        let _ = room.host.try_send(Signal::Left);
    }
}

pub(crate) fn normalize_pairing_code(value: &str) -> Option<String> {
    let code = value
        .chars()
        .filter(|c| *c != '-')
        .collect::<String>()
        .to_ascii_uppercase();
    (code.len() == 12
        && code
            .bytes()
            .all(|b| b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789".contains(&b)))
    .then_some(code)
}
