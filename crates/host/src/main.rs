#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod capture;
mod input;
mod session;

use eframe::egui;
use rand::RngCore;
use std::sync::mpsc::{self as std_mpsc, Receiver as TrayReceiver};
use std::sync::{Arc, Mutex};
use std::{fs, path::PathBuf};
use tokio::sync::mpsc;
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    Icon, TrayIcon, TrayIconBuilder,
};

#[derive(Default)]
pub struct Status {
    pub message: String,
    pub pending: bool,
    pub pair_pending: bool,
    pub running: bool,
    pub registered: bool,
    pub capture_fps: f64,
    pub encoded_fps: f64,
    pub dimensions: (usize, usize),
}
pub enum Command {
    Approve,
    Reject,
    PairApprove,
    PairReject,
    Stop,
}
type Shared = Arc<Mutex<Status>>;

const DEFAULT_RELAY: &str = "https://xandesk.onrender.com";

#[derive(serde::Deserialize, serde::Serialize)]
struct HostIdentity {
    room: String,
}

fn host_identity() -> anyhow::Result<HostIdentity> {
    let root = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("Windows profile directory is unavailable"))?;
    let dir = root.join("XanDesk");
    let path = dir.join("host-identity.json");
    if let Ok(data) = fs::read(&path) {
        if let Ok(identity) = serde_json::from_slice::<HostIdentity>(&data) {
            if identity.room.len() == 32 && identity.room.bytes().all(|b| b.is_ascii_hexdigit()) {
                return Ok(identity);
            }
        }
    }
    fs::create_dir_all(&dir)?;
    let identity = HostIdentity {
        room: random_hex::<16>(),
    };
    fs::write(path, serde_json::to_vec(&identity)?)?;
    Ok(identity)
}

fn random_hex<const N: usize>() -> String {
    let mut bytes = [0; N];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn random_pairing_code() -> String {
    let mut bytes = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let alphabet = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let code: String = bytes
        .iter()
        .map(|byte| alphabet[(byte & 31) as usize] as char)
        .collect();
    format!("{}-{}-{}", &code[..4], &code[4..8], &code[8..12])
}

struct App {
    runtime: tokio::runtime::Runtime,
    status: Shared,
    commands: Option<mpsc::UnboundedSender<Command>>,
    server: String,
    room: String,
    token: String,
    pairing_code: String,
    bitrate: u32,
    allow_clipboard: bool,
    away_access: bool,
    _tray: Option<TrayIcon>,
    tray_events: TrayReceiver<String>,
    open_pair_when_ready: bool,
    approval_window_opened: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, runtime: tokio::runtime::Runtime) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        let (tray_tx, tray_events) = std_mpsc::channel();
        let wake_ctx = cc.egui_ctx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let _ = tray_tx.send(event.id.0);
            wake_ctx.request_repaint();
        }));
        let pair_item = MenuItem::with_id("pair", "Pair this PC...", true, None);
        let open_item = MenuItem::with_id("open", "Open XanDesk", true, None);
        let pause_item = MenuItem::with_id("pause", "Pause (go offline)", true, None);
        let quit_item = MenuItem::with_id("quit", "Quit XanDesk", true, None);
        let separator = PredefinedMenuItem::separator();
        let tray = Menu::with_items(&[&pair_item, &separator, &open_item, &pause_item, &quit_item])
            .ok()
            .and_then(|menu| {
                let mut rgba = vec![0u8; 16 * 16 * 4];
                for y in 2..14 {
                    for x in 2..14 {
                        let i = (y * 16 + x) * 4;
                        rgba[i..i + 4].copy_from_slice(&[200, 242, 123, 255]);
                    }
                }
                let icon = Icon::from_rgba(rgba, 16, 16).ok()?;
                TrayIconBuilder::new()
                    .with_menu(Box::new(menu))
                    .with_icon(icon)
                    .with_tooltip("XanDesk — remote PC host")
                    .build()
                    .ok()
            });
        if tray.is_none() {
            cc.egui_ctx
                .send_viewport_cmd(egui::ViewportCommand::Visible(true));
        }
        Self {
            runtime,
            status: Arc::new(Mutex::new(Status {
                message: "XanDesk is running in the system tray.".into(),
                ..Default::default()
            })),
            commands: None,
            server: DEFAULT_RELAY.to_owned(),
            room: String::new(),
            token: String::new(),
            pairing_code: String::new(),
            bitrate: 80,
            allow_clipboard: false,
            away_access: false,
            _tray: tray,
            tray_events,
            open_pair_when_ready: false,
            approval_window_opened: false,
        }
    }
    fn start(&mut self) {
        let base = match session::server_url(&self.server) {
            Ok(url) => url,
            Err(e) => {
                self.status.lock().unwrap().message = e.to_string();
                return;
            }
        };
        match host_identity() {
            Ok(identity) => {
                self.room = identity.room;
                self.token = random_hex::<32>();
            }
            Err(error) => {
                self.status.lock().unwrap().message =
                    format!("Could not create this PC's secure identity: {error}");
                return;
            }
        }
        self.pairing_code = random_pairing_code();
        let (tx, rx) = mpsc::unbounded_channel();
        self.commands = Some(tx);
        *self.status.lock().unwrap() = Status {
            running: true,
            message: "Connecting to the signaling server…".into(),
            ..Default::default()
        };
        let status = self.status.clone();
        let room = self.room.clone();
        let token = self.token.clone();
        let pairing_code = self.pairing_code.clone();
        let bitrate = self.bitrate;
        let clipboard = self.allow_clipboard;
        let away_access = self.away_access;
        self.runtime.spawn(async move {
            let result = session::run(
                base,
                room,
                token,
                pairing_code,
                bitrate,
                clipboard,
                away_access,
                status.clone(),
                rx,
            )
            .await;
            let mut s = status.lock().unwrap();
            s.running = false;
            s.registered = false;
            s.pending = false;
            s.pair_pending = false;
            s.capture_fps = 0.0;
            s.encoded_fps = 0.0;
            s.message = match result {
                Ok(()) => "Sharing stopped. The pairing link is no longer active.".into(),
                Err(e) => format!("Sharing stopped: {e:#}"),
            };
        });
    }
    fn command(&self, command: Command) {
        if let Some(tx) = &self.commands {
            let _ = tx.send(command);
        }
    }

    fn open_pair_page(&mut self) {
        let site = std::env::var("XANDESK_SITE_URL")
            .unwrap_or_else(|_| "https://xandesk.pages.dev".into())
            .trim_end_matches('/')
            .to_owned();
        if self.pairing_code.is_empty() {
            return;
        }
        let url = format!("{site}/#/dashboard?pair={}", self.pairing_code);
        if let Err(error) = webbrowser::open(&url) {
            self.status.lock().unwrap().message =
                format!("Could not open XanDesk website: {error}");
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
        while let Ok(event) = self.tray_events.try_recv() {
            match event.as_str() {
                "open" => ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true)),
                "pair" => {
                    let status = self.status.lock().unwrap();
                    let running = status.running;
                    let registered = status.registered;
                    drop(status);
                    if running && registered {
                        self.open_pair_page();
                    } else {
                        self.open_pair_when_ready = true;
                        if !running {
                            self.start();
                        }
                    }
                }
                "pause" => self.command(Command::Stop),
                "quit" => {
                    self.command(Command::Stop);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                _ => {}
            }
        }
        let (pending, pair_pending, registered, running) = {
            let s = self.status.lock().unwrap();
            (s.pending, s.pair_pending, s.registered, s.running)
        };
        if self.open_pair_when_ready && !running && !registered {
            self.open_pair_when_ready = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        }
        if pending || pair_pending {
            self.approval_window_opened = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        } else if self.approval_window_opened && running {
            self.approval_window_opened = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        if registered && self.open_pair_when_ready {
            self.open_pair_when_ready = false;
            self.open_pair_page();
        }
        let running = self.status.lock().unwrap().running;
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(18.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("X").size(22.0).strong().color(egui::Color32::from_rgb(200,242,123)));
                ui.vertical(|ui| { ui.heading(egui::RichText::new("XanDesk").size(25.0).strong()); ui.label(egui::RichText::new("WINDOWS HOST").size(10.0).color(egui::Color32::from_rgb(155,165,155))); });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (label, color) = if running { ("●  SHARING", egui::Color32::from_rgb(160,235,130)) } else { ("●  OFFLINE", egui::Color32::from_rgb(155,165,155)) };
                    ui.label(egui::RichText::new(label).size(11.0).strong().color(color));
                });
            });
            ui.add_space(15.0);
            ui.label(egui::RichText::new("Your PC, ready when you are.").size(15.0).color(egui::Color32::from_rgb(200,206,200)));
            ui.add_space(18.0);
            egui::Frame::group(ui.style()).inner_margin(egui::Margin::same(16)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new("CONNECTION").size(10.0).strong().color(egui::Color32::from_rgb(200,242,123)));
            ui.add_space(7.0);
            ui.label("XanDesk relay is configured automatically. No server setup needed.");
            ui.add_space(12.0);
            ui.add_enabled_ui(!running, |ui| {
                ui.checkbox(&mut self.allow_clipboard, "Allow remote browser to write my text clipboard");
                ui.checkbox(&mut self.away_access, "Allow my private pairing link to connect while I’m away");
                if self.away_access { ui.small("Anyone with the active pairing code can control the unlocked PC until you stop sharing."); }
            });
            ui.add_space(8.0);
            if running {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("Pairing code  {}", self.pairing_code)).size(21.0).strong());
                    if ui.button("Copy code").clicked() { ctx.copy_text(self.pairing_code.clone()); }
                });
                ui.small("Enter this code on the XanDesk website. It is valid only while sharing is active.");
                ui.add_space(10.0);
                if ui.add_sized([ui.available_width(),38.0],egui::Button::new("Stop sharing").fill(egui::Color32::from_rgb(92,43,43))).clicked() { self.command(Command::Stop); }
            } else if ui.add_sized([ui.available_width(),42.0],egui::Button::new(egui::RichText::new("Start sharing").strong()).fill(egui::Color32::from_rgb(117,151,57))).clicked() { self.start(); }
            });
            ui.add_space(12.0);
            let s = self.status.lock().unwrap();
            ui.label(egui::RichText::new("ACTIVITY").size(10.0).strong().color(egui::Color32::from_rgb(155,165,155)));
            ui.add_space(4.0); ui.label(&s.message);
            if s.pending || s.pair_pending {
                ui.add_space(12.0);
                if pair_pending {
                    ui.colored_label(egui::Color32::YELLOW,"Someone wants to add this PC to their XanDesk account.");
                    ui.horizontal(|ui| { if ui.button("Allow pairing").clicked() { self.command(Command::PairApprove); } if ui.button("Decline").clicked() { self.command(Command::PairReject); } });
                } else {
                    ui.colored_label(egui::Color32::YELLOW,"Someone wants to see and control this PC.");
                    ui.horizontal(|ui| { if ui.button("Allow this session").clicked() { self.command(Command::Approve); } if ui.button("Decline").clicked() { self.command(Command::Reject); } });
                }
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.label(format!("Capture  {:.1} FPS",s.capture_fps)); ui.separator();
                ui.label(format!("Encode  {:.1} FPS",s.encoded_fps)); ui.separator();
                ui.label(format!("{} × {}",s.dimensions.0,s.dimensions.1));
            });
            ui.add_space(12.0); ui.separator(); ui.add_space(7.0);
            ui.small("Capture performance follows your display and PC hardware. Keep Windows unlocked for access.");
            });
        });
    }
}
impl Drop for App {
    fn drop(&mut self) {
        self.command(Command::Stop);
    }
}

fn main() -> eframe::Result {
    let runtime = tokio::runtime::Runtime::new().expect("Unable to start network runtime");
    eframe::run_native(
        "XanDesk — Windows host",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_visible(false)
                .with_inner_size([520.0, 480.0])
                .with_min_inner_size([480.0, 440.0]),
            ..Default::default()
        },
        Box::new(move |cc| Ok(Box::new(App::new(cc, runtime)))),
    )
}
