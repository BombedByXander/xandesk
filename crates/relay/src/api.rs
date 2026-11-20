use crate::{normalize_pairing_code, App};
use arcdesk_protocol::Signal;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Duration};
use subtle::ConstantTimeEq;
use tokio::sync::oneshot;

#[derive(Clone, Deserialize, Serialize)]
struct DeviceRow {
    id: String,
    owner_id: String,
    room_id: String,
    name: String,
    os: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    created_at: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceView {
    id: String,
    name: String,
    os: String,
    online: bool,
    last_seen: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairRequest {
    pairing_code: String,
    name: Option<String>,
}

#[derive(Deserialize)]
pub struct RenameRequest {
    name: String,
}

struct AuthUser {
    id: String,
    token: String,
}

fn error(status: StatusCode, message: impl Into<String>) -> impl IntoResponse {
    (status, Json(json!({"message": message.into()})))
}

async fn user_id(app: &App, headers: &HeaderMap) -> Result<AuthUser, (StatusCode, String)> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|v| !v.is_empty())
        .ok_or((StatusCode::UNAUTHORIZED, "Sign in to continue.".into()))?;
    if app.supabase_url.is_empty() || app.supabase_publishable_key.is_empty() {
        return Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "The XanDesk relay is missing its Supabase project settings.".into(),
        ));
    }
    let response = app
        .http
        .get(format!("{}/auth/v1/user", app.supabase_url))
        .header("apikey", &app.supabase_publishable_key)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| {
            (
                StatusCode::BAD_GATEWAY,
                "Could not verify your account with Supabase.".into(),
            )
        })?;
    if !response.status().is_success() {
        return Err((
            StatusCode::UNAUTHORIZED,
            "Your sign-in expired. Sign in again.".into(),
        ));
    }
    let user: Value = response.json().await.map_err(|_| {
        (
            StatusCode::BAD_GATEWAY,
            "Supabase returned an invalid account response.".into(),
        )
    })?;
    user.get("id")
        .and_then(Value::as_str)
        .map(|id| AuthUser {
            id: id.to_owned(),
            token: token.to_owned(),
        })
        .ok_or((
            StatusCode::UNAUTHORIZED,
            "Supabase did not identify this account.".into(),
        ))
}

fn db_ready(app: &App) -> Result<(), (StatusCode, String)> {
    if app.supabase_url.is_empty() || app.supabase_publishable_key.is_empty() {
        Err((
            StatusCode::SERVICE_UNAVAILABLE,
            "The Supabase project settings are missing on the XanDesk API.".into(),
        ))
    } else {
        Ok(())
    }
}

async fn db_list(app: &App, owner: &AuthUser) -> Result<Vec<DeviceRow>, (StatusCode, String)> {
    db_ready(app)?;
    let response = app
        .http
        .get(format!("{}/rest/v1/xandesk_devices", app.supabase_url))
        .query(&[
            ("select", "id,owner_id,room_id,name,os,created_at"),
            ("owner_id", &format!("eq.{}", owner.id)),
        ])
        .header("apikey", &app.supabase_publishable_key)
        .bearer_auth(&owner.token)
        .send()
        .await
        .map_err(|_| {
            (
                StatusCode::BAD_GATEWAY,
                "Could not reach the XanDesk device database.".into(),
            )
        })?;
    if !response.status().is_success() {
        return Err((StatusCode::BAD_GATEWAY,"The XanDesk device table is missing or unavailable. Apply the supplied Supabase migration.".into()));
    }
    response.json().await.map_err(|_| {
        (
            StatusCode::BAD_GATEWAY,
            "The XanDesk device database returned invalid data.".into(),
        )
    })
}

async fn db_owned(
    app: &App,
    owner: &AuthUser,
    id: &str,
) -> Result<Option<DeviceRow>, (StatusCode, String)> {
    db_ready(app)?;
    let response = app
        .http
        .get(format!("{}/rest/v1/xandesk_devices", app.supabase_url))
        .query(&[
            ("select", "id,owner_id,room_id,name,os,created_at"),
            ("owner_id", &format!("eq.{}", owner.id)),
            ("id", &format!("eq.{id}")),
        ])
        .header("apikey", &app.supabase_publishable_key)
        .bearer_auth(&owner.token)
        .send()
        .await
        .map_err(|_| {
            (
                StatusCode::BAD_GATEWAY,
                "Could not reach the XanDesk device database.".into(),
            )
        })?;
    if !response.status().is_success() {
        return Err((
            StatusCode::BAD_GATEWAY,
            "Could not read the XanDesk device database.".into(),
        ));
    }
    let rows: Vec<DeviceRow> = response.json().await.map_err(|_| {
        (
            StatusCode::BAD_GATEWAY,
            "The XanDesk device database returned invalid data.".into(),
        )
    })?;
    Ok(rows.into_iter().next())
}

pub async fn list_devices(State(app): State<Arc<App>>, headers: HeaderMap) -> impl IntoResponse {
    let owner = match user_id(&app, &headers).await {
        Ok(v) => v,
        Err((s, m)) => return error(s, m).into_response(),
    };
    let rows = match db_list(&app, &owner).await {
        Ok(v) => v,
        Err((s, m)) => return error(s, m).into_response(),
    };
    let rooms = app.rooms.lock().await;
    let devices = rows
        .into_iter()
        .map(|d| DeviceView {
            online: rooms.contains_key(&d.room_id),
            id: d.id,
            name: d.name,
            os: d.os,
            last_seen: None,
        })
        .collect::<Vec<_>>();
    Json(json!({"devices":devices})).into_response()
}

pub async fn claim_device(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Json(body): Json<PairRequest>,
) -> impl IntoResponse {
    let owner = match user_id(&app, &headers).await {
        Ok(v) => v,
        Err((s, m)) => return error(s, m).into_response(),
    };
    if let Err((s, m)) = db_ready(&app) {
        return error(s, m).into_response();
    }
    let Some(code) = normalize_pairing_code(&body.pairing_code) else {
        return error(
            StatusCode::BAD_REQUEST,
            "Enter the 12-character code shown by the Windows host.",
        )
        .into_response();
    };
    let digest: [u8; 32] = Sha256::digest(code.as_bytes()).into();
    let (room_id, host_name, rx) = {
        let mut rooms = app.rooms.lock().await;
        let candidate = rooms.iter().find_map(|(id, room)| {
            room.pairing_digest
                .as_ref()
                .filter(|known| bool::from(known.ct_eq(&digest)))
                .map(|_| id.clone())
        });
        let Some(id) = candidate else {
            return error(StatusCode::NOT_FOUND,"That pairing code is not active. On the Windows PC, choose Pair this PC and try the current code.").into_response();
        };
        let room = rooms.get_mut(&id).unwrap();
        if room.viewer.is_some() || room.pair_claim.is_some() || room.pair_claim_approved {
            return error(
                StatusCode::CONFLICT,
                "This PC is busy with another connection or pairing request.",
            )
            .into_response();
        }
        let (tx, rx) = oneshot::channel();
        if room.host.try_send(Signal::PairRequest).is_err() {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "The Windows host is not responding. Try again in a moment.",
            )
            .into_response();
        }
        room.pair_claim = Some(tx);
        (id, room.device_name.clone(), rx)
    };
    let approved = matches!(
        tokio::time::timeout(Duration::from_secs(95), rx).await,
        Ok(Ok(true))
    );
    if !approved {
        if let Some(room) = app.rooms.lock().await.get_mut(&room_id) {
            room.pair_claim = None;
            room.pair_claim_approved = false;
        }
        return error(
            StatusCode::FORBIDDEN,
            "Pairing was declined or timed out. Approve the pairing request on the Windows PC.",
        )
        .into_response();
    }
    let requested = body
        .name
        .unwrap_or_default()
        .trim()
        .chars()
        .take(64)
        .collect::<String>();
    let name = if requested.is_empty() {
        if host_name.trim().is_empty() {
            "Windows PC".into()
        } else {
            host_name.trim().chars().take(64).collect()
        }
    } else {
        requested
    };
    let existing_response = app
        .http
        .get(format!("{}/rest/v1/xandesk_devices", app.supabase_url))
        .query(&[
            ("select", "id,owner_id,room_id,name,os,created_at"),
            ("id", &format!("eq.{room_id}")),
        ])
        .header("apikey", &app.supabase_publishable_key)
        .bearer_auth(&owner.token)
        .send()
        .await;
    let existing_rows = match existing_response {
        Ok(r) if r.status().is_success() => r.json::<Vec<DeviceRow>>().await.ok(),
        _ => None,
    };
    let Some(existing_rows) = existing_rows else {
        if let Some(room) = app.rooms.lock().await.get_mut(&room_id) {
            room.pair_claim_approved = false;
        }
        return error(
            StatusCode::BAD_GATEWAY,
            "Could not verify existing computer ownership.",
        )
        .into_response();
    };
    if existing_rows.iter().any(|d| d.owner_id != owner.id) {
        if let Some(room) = app.rooms.lock().await.get_mut(&room_id) {
            room.pair_claim_approved = false;
        }
        return error(
            StatusCode::CONFLICT,
            "This PC is already linked to another account. Remove it there before pairing it here.",
        )
        .into_response();
    }
    let row = DeviceRow {
        id: room_id.clone(),
        owner_id: owner.id.clone(),
        room_id: room_id.clone(),
        name,
        os: "Windows PC".into(),
        created_at: None,
    };
    let saved = app
        .http
        .post(format!("{}/rest/v1/xandesk_devices", app.supabase_url))
        .query(&[("on_conflict", "id")])
        .header("apikey", &app.supabase_publishable_key)
        .bearer_auth(&owner.token)
        .header("content-type", "application/json")
        .header(
            "Prefer",
            "resolution=merge-duplicates,return=representation",
        )
        .json(&row)
        .send()
        .await;
    let Ok(response) = saved else {
        if let Some(room) = app.rooms.lock().await.get_mut(&room_id) {
            room.pair_claim_approved = false;
        }
        return error(StatusCode::BAD_GATEWAY,"PC approval worked, but the device could not be saved. Check the XanDesk device table and retry pairing.").into_response();
    };
    if !response.status().is_success() {
        if let Some(room) = app.rooms.lock().await.get_mut(&room_id) {
            room.pair_claim_approved = false;
        }
        return error(StatusCode::BAD_GATEWAY,"PC approval worked, but the device database rejected the save. Check the XanDesk device table.").into_response();
    }
    if let Some(room) = app.rooms.lock().await.get_mut(&room_id) {
        room.pair_claim_approved = false;
    }
    Json(json!({"device":{"id":row.id,"name":row.name,"os":row.os,"online":true}})).into_response()
}

pub async fn rename_device(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<RenameRequest>,
) -> impl IntoResponse {
    let owner = match user_id(&app, &headers).await {
        Ok(v) => v,
        Err((s, m)) => return error(s, m).into_response(),
    };
    let name = body.name.trim().chars().take(64).collect::<String>();
    if name.is_empty() {
        return error(StatusCode::BAD_REQUEST, "Enter a device name.").into_response();
    }
    let result = app
        .http
        .patch(format!("{}/rest/v1/xandesk_devices", app.supabase_url))
        .query(&[
            ("owner_id", &format!("eq.{}", owner.id)),
            ("id", &format!("eq.{id}")),
        ])
        .header("apikey", &app.supabase_publishable_key)
        .bearer_auth(&owner.token)
        .header("Prefer", "return=representation")
        .json(&json!({"name":name}))
        .send()
        .await;
    let Ok(response) = result else {
        return error(
            StatusCode::BAD_GATEWAY,
            "Could not reach the device database.",
        )
        .into_response();
    };
    let rows: Vec<DeviceRow> = match response.json().await {
        Ok(v) => v,
        Err(_) => {
            return error(
                StatusCode::BAD_GATEWAY,
                "Device database returned invalid data.",
            )
            .into_response()
        }
    };
    rows.into_iter()
        .next()
        .map(|d| Json(json!({"device":{"id":d.id,"name":d.name,"os":d.os}})).into_response())
        .unwrap_or_else(|| error(StatusCode::NOT_FOUND, "Computer not found.").into_response())
}

pub async fn remove_device(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let owner = match user_id(&app, &headers).await {
        Ok(v) => v,
        Err((s, m)) => return error(s, m).into_response(),
    };
    let device = match db_owned(&app, &owner, &id).await {
        Ok(Some(v)) => v,
        Ok(None) => return error(StatusCode::NOT_FOUND, "Computer not found.").into_response(),
        Err((s, m)) => return error(s, m).into_response(),
    };
    let result = app
        .http
        .delete(format!("{}/rest/v1/xandesk_devices", app.supabase_url))
        .query(&[
            ("owner_id", &format!("eq.{}", owner.id)),
            ("id", &format!("eq.{id}")),
        ])
        .header("apikey", &app.supabase_publishable_key)
        .bearer_auth(&owner.token)
        .header("Prefer", "return=representation")
        .send()
        .await;
    match result {
        Ok(r) if r.status().is_success() => {
            let mut rooms = app.rooms.lock().await;
            if let Some(room) = rooms.get_mut(&device.room_id) {
                if let Some(viewer) = room.viewer.take() {
                    let _ = viewer.try_send(Signal::Left);
                }
                room.approved = false;
                room.viewer_tokens.clear();
            }
            Json(json!({"ok":true})).into_response()
        }
        Ok(_) => error(StatusCode::BAD_GATEWAY, "Could not remove this computer.").into_response(),
        Err(_) => error(
            StatusCode::BAD_GATEWAY,
            "Could not reach the device database.",
        )
        .into_response(),
    }
}

pub async fn create_session(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let owner = match user_id(&app, &headers).await {
        Ok(v) => v,
        Err((s, m)) => return error(s, m).into_response(),
    };
    let Some(device) = (match db_owned(&app, &owner, &id).await {
        Ok(v) => v,
        Err((s, m)) => return error(s, m).into_response(),
    }) else {
        return error(StatusCode::NOT_FOUND, "Computer not found in your account.").into_response();
    };
    if app.public_url.is_empty() {
        return error(
            StatusCode::SERVICE_UNAVAILABLE,
            "XANDESK_RELAY_PUBLIC_URL is not set on the API server.",
        )
        .into_response();
    }
    let token = {
        let mut bytes = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut bytes);
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    };
    let digest: [u8; 32] = Sha256::digest(token.as_bytes()).into();
    let ice_servers = app.ice.clone();
    {
        let mut rooms = app.rooms.lock().await;
        let Some(room) = rooms.get_mut(&device.room_id) else {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "This computer is offline. Start XanDesk sharing on the Windows PC.",
            )
            .into_response();
        };
        if room.viewer.is_some() {
            return error(
                StatusCode::CONFLICT,
                "This computer already has an active viewer.",
            )
            .into_response();
        }
        room.viewer_tokens
            .retain(|_, expiry| *expiry > tokio::time::Instant::now());
        room.viewer_tokens.insert(
            digest,
            tokio::time::Instant::now() + Duration::from_secs(120),
        );
    }
    let session_id = {
        let mut b = [0u8; 16];
        rand::rngs::OsRng.fill_bytes(&mut b);
        b.iter().map(|v| format!("{v:02x}")).collect::<String>()
    };
    let ws_base = app
        .public_url
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);
    let signaling_url = format!("{}/ws", ws_base);
    Json(json!({"sessionId":session_id,"name":device.name,"room":device.room_id,"joinToken":token,"signalingUrl":signaling_url,"iceServers":ice_servers})).into_response()
}
