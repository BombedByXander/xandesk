use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IceServer {
    pub urls: Vec<String>,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub credential: String,
}

// Credentials must never be logged. Tokens are generated with OS randomness by the host.
#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Signal {
    Register {
        room: String,
        token: String,
        #[serde(default)]
        device_name: String,
        #[serde(default)]
        pairing_code: String,
    },
    Join {
        room: String,
        token: String,
    },
    PairRequest,
    PairApprove,
    PairReject,
    Registered {
        ice_servers: Vec<IceServer>,
    },
    Request {
        #[serde(default)]
        account_authorized: bool,
    },
    Approve,
    Reject,
    Ready {
        ice_servers: Vec<IceServer>,
    },
    Offer {
        sdp: String,
    },
    Answer {
        sdp: String,
    },
    Heartbeat,
    Left,
    Error {
        message: String,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Input {
    Move { x: f64, y: f64 },
    Relative { x: i32, y: i32 },
    Button { button: u8, down: bool },
    Key { code: String, down: bool },
    Wheel { delta: i32 },
    Release,
    Heartbeat,
    Clipboard { text: String },
}

pub fn valid_credentials(room: &str, token: &str) -> bool {
    room.len() == 32
        && token.len() == 64
        && room
            .bytes()
            .chain(token.bytes())
            .all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credentials_must_be_full_length_hex() {
        assert!(valid_credentials(&"a".repeat(32), &"b".repeat(64)));
        assert!(!valid_credentials("abc", &"b".repeat(64)));
        assert!(!valid_credentials(&"z".repeat(32), &"b".repeat(64)));
    }
}
