use arcdesk_protocol::{valid_credentials, IceServer, Signal};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use futures_util::{SinkExt, StreamExt};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::Arc, time::Duration};
use subtle::ConstantTimeEq;
use tokio::sync::{mpsc, Mutex, Semaphore};

type Tx = mpsc::Sender<Signal>;
struct Room {
    digest: [u8; 32],
    pairing_digest: Option<[u8; 32]>,
    host: Tx,
    viewer: Option<Tx>,
    approved: bool,
}
struct App {
    rooms: Mutex<HashMap<String, Room>>,
    ice: Vec<IceServer>,
    origins: Vec<String>,
    slots: Arc<Semaphore>,
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
    let state = Arc::new(App {
        rooms: Mutex::new(HashMap::new()),
        ice,
        origins,
        slots: Arc::new(Semaphore::new(256)),
    });
    let app = Router::new()
        .route("/", get(|| async { Html(include_str!("../../../web/index.html")) }))
        .route("/client.js", get(|| async { ([("content-type", "text/javascript; charset=utf-8")], include_str!("../../../web/client.js")) }))
        .route("/app.js", get(|| async { ([("content-type", "text/javascript; charset=utf-8")], include_str!("../../../web/app.js")) }))
        .route("/viewer.html", get(|| async { Html(include_str!("../../../web/viewer.html")) }))
        .route("/style.css", get(|| async { ([("content-type", "text/css; charset=utf-8")], include_str!("../../../web/style.css")) }))
        .route("/health", get(|| async { "ok" }))
        .route("/ws", get(upgrade))
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
    let (room_id, token, is_host, pairing_code) = match signal {
        Signal::Register { room, token, pairing_code } => (room, token, true, Some(pairing_code)),
        Signal::Join { room, token } => (room, token, false, None),
        Signal::Pair { pairing_code } => {
            let Some(code) = normalize_pairing_code(&pairing_code) else { return };
            let digest: [u8; 32] = Sha256::digest(code.as_bytes()).into();
            let rooms = app.rooms.lock().await;
            let room_id = rooms.iter().find_map(|(id, room)| {
                room.pairing_digest
                    .as_ref()
                    .filter(|known| bool::from(known.ct_eq(&digest)))
                    .map(|_| id.clone())
            });
            drop(rooms);
            let Some(room_id) = room_id else {
                send(&mut socket, &Signal::Error { message: "Pairing code is invalid or the PC is offline.".into() }).await;
                return;
            };
            (room_id, String::new(), false, Some(code))
        }
        _ => return,
    };
    let pairing_code_valid = pairing_code.as_ref().and_then(|code| normalize_pairing_code(code)).is_some();
    let has_pairing_code = pairing_code.as_ref().is_some_and(|code| !code.is_empty());
    if (is_host && has_pairing_code && !pairing_code_valid) ||
        (!valid_credentials(&room_id, &token) && !(pairing_code_valid && !is_host && token.is_empty())) {
        return;
    }
    let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    let (tx, mut rx) = mpsc::channel(16);
    let registered = {
        let mut rooms = app.rooms.lock().await;
        if is_host {
            if rooms.contains_key(&room_id) {
                false
            } else {
                let pairing_digest: Option<[u8; 32]> = pairing_code.as_ref().and_then(|code| {
                    normalize_pairing_code(code).map(|normalized| Sha256::digest(normalized.as_bytes()).into())
                });
                if pairing_digest.as_ref().is_some_and(|candidate| rooms.values().any(|room| {
                    room.pairing_digest.as_ref().is_some_and(|known| bool::from(known.ct_eq(candidate)))
                })) {
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
                    },
                );
                true
            }
        } else if let Some(room) = rooms.get_mut(&room_id) {
            let authorized = if token.is_empty() {
                pairing_code.as_ref().and_then(|code| normalize_pairing_code(code)).and_then(|code| {
                    let presented: [u8; 32] = Sha256::digest(code.as_bytes()).into();
                    room.pairing_digest.as_ref().map(|known| bool::from(known.ct_eq(&presented)))
                }).unwrap_or(false)
            } else {
                bool::from(room.digest.ct_eq(&digest))
            };
            if authorized
                && room.viewer.is_none()
                && room.host.try_send(Signal::Request).is_ok()
            {
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
            ice_servers: app.ice.clone(),
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
                        Some((room.viewer.as_ref().unwrap().clone(), Signal::Ready { ice_servers: app.ice.clone() }))
                    }
                    (true, Signal::Reject) => room.viewer.as_ref().map(|v| (v.clone(), Signal::Reject)),
                    (true, Signal::Answer { sdp }) if room.approved => room.viewer.as_ref().map(|v| (v.clone(), Signal::Answer { sdp })),
                    (false, Signal::Offer { sdp }) if room.approved => Some((room.host.clone(), Signal::Offer { sdp })),
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

fn normalize_pairing_code(value: &str) -> Option<String> {
    let code = value.chars().filter(|c| *c != '-').collect::<String>().to_ascii_uppercase();
    (code.len() == 12 && code.bytes().all(|b| b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789".contains(&b)))
        .then_some(code)
}
