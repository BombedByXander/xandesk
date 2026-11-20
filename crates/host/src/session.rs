use crate::{capture, input::Controller, Command, Shared};
use anyhow::Context;
use arcdesk_protocol::{IceServer, Input, Signal};
use futures_util::{SinkExt, StreamExt};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use webrtc::{
    api::{
        interceptor_registry::register_default_interceptors,
        media_engine::{MediaEngine, MIME_TYPE_H264},
        APIBuilder,
    },
    ice_transport::ice_server::RTCIceServer,
    interceptor::registry::Registry,
    peer_connection::{
        configuration::RTCConfiguration, peer_connection_state::RTCPeerConnectionState,
        sdp::session_description::RTCSessionDescription, RTCPeerConnection,
    },
    rtp::{
        codecs::h264::H264Payloader,
        packetizer::{new_packetizer, Packetizer},
        sequence::new_random_sequencer,
    },
    rtp_transceiver::rtp_codec::RTCRtpCodecCapability,
    track::track_local::{
        track_local_static_rtp::TrackLocalStaticRTP, TrackLocal, TrackLocalWriter,
    },
};

pub fn server_url(text: &str) -> anyhow::Result<url::Url> {
    let mut url = url::Url::parse(text).context("Enter a valid connection server URL")?;
    anyhow::ensure!(
        url.username().is_empty() && url.password().is_none(),
        "Do not include credentials in the server URL"
    );
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    match url.scheme() {
        "https" => {
            url.set_scheme("wss").unwrap();
        }
        "http" if local => {
            url.set_scheme("ws").unwrap();
        }
        _ => anyhow::bail!("Use HTTPS for remote servers (HTTP is only allowed on localhost)"),
    }
    url.set_path("/ws");
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

pub async fn run(
    url: url::Url,
    room: String,
    token: String,
    pairing_code: String,
    bitrate: u32,
    clipboard: bool,
    away_access: bool,
    status: Shared,
    mut commands: mpsc::UnboundedReceiver<Command>,
) -> anyhow::Result<()> {
    let (socket, _) = tokio::time::timeout(Duration::from_secs(15), connect_async(url.as_str()))
        .await
        .context("Server connection timed out")??;
    let (mut sink, mut stream) = socket.split();
    sink.send(Message::Text(
        serde_json::to_string(&Signal::Register {
            room,
            token,
            device_name: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows PC".into()),
            pairing_code,
        })?
        .into(),
    ))
    .await?;
    let mut ice = Vec::new();
    let mut active: Option<Session> = None;
    let mut approved = false;
    let (answers, mut answer_rx) = mpsc::unbounded_channel::<anyhow::Result<String>>();
    let (errors, mut error_rx) = mpsc::unbounded_channel::<String>();
    let mut negotiation: Option<JoinHandle<()>> = None;
    let mut approval_deadline: Option<Instant> = None;
    let mut session_deadline: Option<Instant> = None;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let mut last_server = Instant::now();
    let result: anyhow::Result<()> = async {
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    anyhow::ensure!(last_server.elapsed() < Duration::from_secs(65),"Connection server stopped responding");
                    if approval_deadline.is_some_and(|t| t.elapsed() > Duration::from_secs(90)) {
                        let pair={let mut s=status.lock().unwrap();let pair=s.pair_pending;s.pending=false;s.pair_pending=false;pair}; approval_deadline = None;
                        let signal=if pair { Signal::PairReject } else { Signal::Reject };
                        sink.send(Message::Text(serde_json::to_string(&signal)?.into())).await?;
                    }
                    if session_deadline.is_some_and(|t| t.elapsed() > Duration::from_secs(40)) {
                        if !active.as_ref().is_some_and(|s| s.connected.load(Ordering::Acquire)) { anyhow::bail!("WebRTC connection timed out. Check TURN configuration."); }
                        session_deadline = None;
                    }
                }
                command = commands.recv() => match command {
                    Some(Command::Stop) | None => break,
                    Some(Command::Approve) if status.lock().unwrap().pending => {
                        approved = true; approval_deadline = None; session_deadline = Some(Instant::now());
                        { let mut s=status.lock().unwrap(); s.pending=false; s.message="Approved. Waiting for encrypted connection…".into(); }
                        sink.send(Message::Text(serde_json::to_string(&Signal::Approve)?.into())).await?;
                    }
                    Some(Command::Reject) if status.lock().unwrap().pending => {
                        approval_deadline=None; status.lock().unwrap().pending=false;
                        sink.send(Message::Text(serde_json::to_string(&Signal::Reject)?.into())).await?;
                    }
                    Some(Command::PairApprove) if status.lock().unwrap().pair_pending => {
                        approval_deadline=None; { let mut s=status.lock().unwrap();s.pair_pending=false;s.message="PC pairing approved.".into(); }
                        sink.send(Message::Text(serde_json::to_string(&Signal::PairApprove)?.into())).await?;
                    }
                    Some(Command::PairReject) if status.lock().unwrap().pair_pending => {
                        approval_deadline=None; status.lock().unwrap().pair_pending=false;
                        sink.send(Message::Text(serde_json::to_string(&Signal::PairReject)?.into())).await?;
                    }
                    _=>{}
                },
                answer = answer_rx.recv() => if let Some(answer) = answer {
                    let sdp=answer?; sink.send(Message::Text(serde_json::to_string(&Signal::Answer {sdp})?.into())).await?;
                },
                error = error_rx.recv() => if let Some(error) = error { anyhow::bail!(error); },
                _ = async { if let Some(deadline)=approval_deadline { tokio::time::sleep_until(tokio::time::Instant::from_std(deadline + Duration::from_secs(90))).await; } else { std::future::pending::<()>().await; } } => {
                    let pair={let mut s=status.lock().unwrap();let pair=s.pair_pending;s.pending=false;s.pair_pending=false;pair};approval_deadline=None;
                    let signal=if pair { Signal::PairReject } else { Signal::Reject };
                    sink.send(Message::Text(serde_json::to_string(&signal)?.into())).await?;
                },
                _ = async { if let Some(deadline)=session_deadline { tokio::time::sleep_until(tokio::time::Instant::from_std(deadline + Duration::from_secs(120))).await; } else { std::future::pending::<()>().await; } } => {
                    if !active.as_ref().is_some_and(|s|s.connected.load(Ordering::Acquire)){anyhow::bail!("ICE could not establish a connection through the configured STUN/TURN servers. Check TURN and network restrictions.");}
                    session_deadline=None;
                },
                incoming = stream.next() => {
                    let Some(incoming) = incoming else { anyhow::bail!("Connection server disconnected") };
                    last_server=Instant::now();
                    let incoming=incoming?;
                    let text=match incoming {
                        Message::Ping(data)=>{sink.send(Message::Pong(data)).await?; continue;}
                        Message::Close(_)=>anyhow::bail!("Connection server closed the session"),
                        Message::Text(text)=>text,
                        _=>continue,
                    };
                    match serde_json::from_str::<Signal>(&text)? {
                        Signal::Registered {ice_servers}=>{ice=ice_servers;let mut s=status.lock().unwrap();s.registered=true;s.message="Host online. Choose Pair this PC in the system tray to add it to your account.".into();}
                        Signal::Request=>{
                            if away_access {
                                approved=true; session_deadline=Some(Instant::now());
                                status.lock().unwrap().message="Private pairing link accepted. Establishing connection…".into();
                                sink.send(Message::Text(serde_json::to_string(&Signal::Approve)?.into())).await?;
                            } else {
                                let mut s=status.lock().unwrap(); s.pending=true;s.message="Connection approval needed.".into();approval_deadline=Some(Instant::now());
                            }
                        }
                        Signal::PairRequest=>{
                            let mut s=status.lock().unwrap();s.pair_pending=true;s.message="Account pairing approval needed.".into();approval_deadline=Some(Instant::now());
                        }
                        Signal::Offer {sdp} if approved && active.is_none()=>{
                            let session=Session::new(ice.clone(),bitrate,clipboard,true,status.clone(),errors.clone()).await?;
                            let peer=session.peer.clone(); let tx=answers.clone();
                            negotiation=Some(tokio::spawn(async move { let _=tx.send(answer(peer,sdp).await); }));
                            active=Some(session);
                        }
                        Signal::Left=>{
                            if let Some(task)=negotiation.take(){task.abort();}
                            if let Some(session)=active.take(){session.close().await;}
                            approved=false;approval_deadline=None;session_deadline=None;
                            let mut s=status.lock().unwrap();s.pending=false;s.pair_pending=false;s.message="Viewer disconnected. Ready for a new request.".into();s.capture_fps=0.0;s.encoded_fps=0.0;
                        }
                        Signal::Error {message}=>anyhow::bail!(message),
                        _=>{}
                    }
                }
            }
        }
        Ok(())
    }.await;
    if let Some(task) = negotiation {
        task.abort();
    }
    if let Some(session) = active {
        session.close().await;
    }
    let _ = sink.close().await;
    result
}

async fn answer(peer: Arc<RTCPeerConnection>, sdp: String) -> anyhow::Result<String> {
    peer.set_remote_description(RTCSessionDescription::offer(sdp)?)
        .await?;
    let description = peer.create_answer(None).await?;
    let mut gathering = peer.gathering_complete_promise().await;
    peer.set_local_description(description).await?;
    tokio::time::timeout(Duration::from_secs(20), gathering.recv())
        .await
        .context("ICE gathering timed out")?;
    Ok(peer.local_description().await.context("No local SDP")?.sdp)
}

struct Session {
    peer: Arc<RTCPeerConnection>,
    stopped: Arc<AtomicBool>,
    connected: Arc<AtomicBool>,
    controller: Arc<Mutex<Controller>>,
    tasks: Vec<JoinHandle<()>>,
}
impl Session {
    async fn new(
        ice: Vec<IceServer>,
        bitrate: u32,
        clipboard: bool,
        allow_input: bool,
        status: Shared,
        errors: mpsc::UnboundedSender<String>,
    ) -> anyhow::Result<Self> {
        let mut media = MediaEngine::default();
        media.register_default_codecs()?;
        let registry = register_default_interceptors(Registry::new(), &mut media)?;
        let api = APIBuilder::new()
            .with_media_engine(media)
            .with_interceptor_registry(registry)
            .build();
        let peer = Arc::new(
            api.new_peer_connection(RTCConfiguration {
                ice_servers: ice
                    .into_iter()
                    .map(|i| RTCIceServer {
                        urls: i.urls,
                        username: i.username,
                        credential: i.credential,
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            })
            .await?,
        );
        let stopped = Arc::new(AtomicBool::new(false));
        let connected = Arc::new(AtomicBool::new(false));
        let keyframe = Arc::new(AtomicBool::new(true));
        let controller = Arc::new(Mutex::new(Controller::new(clipboard)));
        let mut session = Self {
            peer: peer.clone(),
            stopped: stopped.clone(),
            connected: connected.clone(),
            controller: controller.clone(),
            tasks: vec![],
        };
        let watchdog = controller.clone();
        session.tasks.push(tokio::spawn(async move {
            let mut timer = tokio::time::interval(Duration::from_secs(1));
            loop {
                timer.tick().await;
                watchdog.lock().unwrap().release_if_stale();
            }
        }));
        let end = stopped.clone();
        let online = connected.clone();
        let report = status.clone();
        let error_tx = errors.clone();
        let controls = controller.clone();
        peer.on_peer_connection_state_change(Box::new(move |state| {
            let end = end.clone();
            let online = online.clone();
            let report = report.clone();
            let errors = error_tx.clone();
            let controls = controls.clone();
            Box::pin(async move {
                if state == RTCPeerConnectionState::Connected {
                    online.store(true, Ordering::Release);
                    report.lock().unwrap().message =
                        "CONNECTED — your primary monitor is being shared and controlled.".into();
                }
                if matches!(
                    state,
                    RTCPeerConnectionState::Disconnected
                        | RTCPeerConnectionState::Failed
                        | RTCPeerConnectionState::Closed
                ) {
                    online.store(false, Ordering::Release);
                    if !end.swap(true, Ordering::AcqRel) {
                        controls.lock().unwrap().release();
                        let _ = errors.send(
                            "Viewer disconnected or WebRTC connection failed. Sharing has stopped."
                                .into(),
                        );
                    }
                }
            })
        }));
        let end = stopped.clone();
        let online = connected.clone();
        let controls = controller.clone();
        peer.on_data_channel(Box::new(move |channel| {
            let end = end.clone();
            let online = online.clone();
            let controls = controls.clone();
            Box::pin(async move {
                if channel.label() != "control" {
                    let _ = channel.close().await;
                    return;
                }
                channel.on_message(Box::new(move |message| {
                    let end = end.clone();
                    let online = online.clone();
                    let controls = controls.clone();
                    Box::pin(async move {
                        if message.data.len() > 65536 || !message.is_string {
                            return;
                        }
                        if let Ok(event) = serde_json::from_slice::<Input>(&message.data) {
                            let mut controller = controls.lock().unwrap();
                            if allow_input
                                && !end.load(Ordering::Acquire)
                                && online.load(Ordering::Acquire)
                            {
                                controller.apply(event);
                            }
                        }
                    })
                }));
            })
        }));
        let track = Arc::new(TrackLocalStaticRTP::new(
            RTCRtpCodecCapability {
                mime_type: MIME_TYPE_H264.into(),
                clock_rate: 90000,
                sdp_fmtp_line:
                    "level-asymmetry-allowed=1;packetization-mode=1;profile-level-id=42e01f".into(),
                ..Default::default()
            },
            "desktop".into(),
            "arcdesk".into(),
        ));
        let sender = peer
            .add_track(track.clone() as Arc<dyn TrackLocal + Send + Sync>)
            .await?;
        let force = keyframe.clone();
        session.tasks.push(tokio::spawn(async move { while let Ok((packets,_))=sender.read_rtcp().await { for packet in packets { if packet.as_any().is::<webrtc::rtcp::payload_feedbacks::picture_loss_indication::PictureLossIndication>() {force.store(true,Ordering::Release);} } } }));
        let (tx, mut rx) = mpsc::channel::<capture::Frame>(1);
        let end = stopped.clone();
        let error_tx = errors.clone();
        session.tasks.push(tokio::spawn(async move {
            let mut origin: Option<Instant> = None;
            let base: u32 = rand::random();
            let mut packetizer = new_packetizer(
                1200,
                96,
                0,
                Box::<H264Payloader>::default(),
                Box::new(new_random_sequencer()),
                90000,
            );
            'frames: while let Some(frame) = rx.recv().await {
                if end.load(Ordering::Acquire) {
                    break;
                }
                let started = *origin.get_or_insert(frame.captured);
                // Explicit capture timestamps avoid a one-frame timing lag after idle periods.
                let timestamp = base.wrapping_add(
                    (frame.captured.duration_since(started).as_secs_f64() * 90000.0) as u32,
                );
                let packets = match packetizer.packetize(&frame.data.into(), 0) {
                    Ok(packets) => packets,
                    Err(error) => {
                        let _ = error_tx.send(format!("Video packetization failed: {error}"));
                        break;
                    }
                };
                for mut packet in packets {
                    packet.header.timestamp = timestamp;
                    if let Err(error) = track.write_rtp(&packet).await {
                        let _ = error_tx.send(format!("Video transport failed: {error}"));
                        break 'frames;
                    }
                }
            }
        }));
        std::thread::Builder::new()
            .name("arcdesk-capture".into())
            .spawn(move || {
                while !connected.load(Ordering::Acquire) && !stopped.load(Ordering::Acquire) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                if !stopped.load(Ordering::Acquire) {
                    if let Err(e) = capture::run(tx, stopped.clone(), keyframe, bitrate, status) {
                        if !stopped.load(Ordering::Acquire) {
                            let _ = errors.send(format!("Screen capture: {e:#}"));
                        }
                    }
                }
            })?;
        Ok(session)
    }
    async fn close(self) {
        self.stop();
        let _ = self.peer.close().await;
    }
    fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        self.connected.store(false, Ordering::Release);
        self.controller.lock().unwrap().release();
        for task in &self.tasks {
            task.abort();
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
        let peer = self.peer.clone();
        tokio::spawn(async move {
            let _ = peer.close().await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn remote_connections_require_tls() {
        assert!(server_url("http://192.168.1.5:8787").is_err());
        assert!(server_url("file:///C:/secret").is_err());
        assert_eq!(
            server_url("https://desk.example/a?token=no")
                .unwrap()
                .as_str(),
            "wss://desk.example/ws"
        );
        assert!(server_url("http://localhost:8787").is_ok());
        assert!(server_url("https://user:password@example.com").is_err());
    }

    // Read-only, loopback-only test harness. Not compiled into production executables.
    // Start this ignored test, then run scripts/test-browser.mjs with Playwright installed.
    #[tokio::test]
    #[ignore = "requires an unlocked Windows desktop and the Playwright browser test"]
    async fn browser_capture_smoke() -> anyhow::Result<()> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:18788").await?;
        println!("Read-only browser smoke test listening on 127.0.0.1:18788");
        let (stream, _) =
            tokio::time::timeout(Duration::from_secs(90), listener.accept()).await??;
        let mut ws = tokio_tungstenite::accept_async(stream).await?;
        let offer = tokio::time::timeout(Duration::from_secs(30), ws.next())
            .await?
            .context("Browser closed")??;
        let offer: serde_json::Value = serde_json::from_str(offer.to_text()?)?;
        let status = Arc::new(Mutex::new(crate::Status::default()));
        let (errors, mut error_rx) = mpsc::unbounded_channel();
        let session = Session::new(vec![], 5, false, false, status, errors).await?;
        let sdp = answer(
            session.peer.clone(),
            offer["sdp"].as_str().context("Missing SDP")?.into(),
        )
        .await?;
        ws.send(Message::Text(
            serde_json::json!({"type":"answer","sdp":sdp})
                .to_string()
                .into(),
        ))
        .await?;
        let report = tokio::select! {
            message=tokio::time::timeout(Duration::from_secs(60),ws.next())=>message?.context("Browser closed before report")??,
            error=error_rx.recv()=>anyhow::bail!("Host capture failed: {:?}",error),
        };
        let report: serde_json::Value = serde_json::from_str(report.to_text()?)?;
        session.close().await;
        anyhow::ensure!(
            report["framesDecoded"].as_u64().unwrap_or(0) > 0,
            "Browser did not decode real frames: {report}"
        );
        anyhow::ensure!(
            report["width"].as_u64().unwrap_or(0) > 0,
            "Browser received no resolution"
        );
        println!("Browser decoded real desktop H.264: {report}");
        Ok(())
    }
}
