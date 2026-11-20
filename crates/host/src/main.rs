#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod capture;
mod input;
mod session;

use eframe::egui;
use rand::RngCore;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

#[derive(Default)]
pub struct Status {
    pub message: String,
    pub pending: bool,
    pub running: bool,
    pub capture_fps: f64,
    pub encoded_fps: f64,
    pub dimensions: (usize, usize),
}
pub enum Command {
    Approve,
    Reject,
    Stop,
}
type Shared = Arc<Mutex<Status>>;

fn random_hex<const N: usize>() -> String {
    let mut bytes = [0; N];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn random_pairing_code() -> String {
    let mut bytes = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    let alphabet = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let code: String = bytes.iter().map(|byte| alphabet[(byte & 31) as usize] as char).collect();
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
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, runtime: tokio::runtime::Runtime) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::dark());
        Self {
            runtime,
            status: Arc::new(Mutex::new(Status {
                message: "Ready. Start sharing when you need this PC.".into(),
                ..Default::default()
            })),
            commands: None,
            server: String::new(),
            room: String::new(),
            token: String::new(),
            pairing_code: String::new(),
            bitrate: 20,
            allow_clipboard: false,
            away_access: false,
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
        self.room = random_hex::<16>();
        self.token = random_hex::<32>();
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
            s.pending = false;
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
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
        let running = self.status.lock().unwrap().running;
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(14.0);
            ui.horizontal(|ui| { ui.heading(egui::RichText::new("▣ XanDesk").size(30.0).color(egui::Color32::from_rgb(159,230,196))); ui.label("WINDOWS HOST · PREVIEW"); });
            ui.label("Your PC, within reach."); ui.add_space(20.0);
            ui.add_enabled_ui(!running, |ui| {
                ui.label("XanDesk relay server"); ui.add(egui::TextEdit::singleline(&mut self.server).hint_text("https://relay.your-domain.example").desired_width(f32::INFINITY));
                ui.label("Enter the public HTTPS relay address for remote access. For a same-PC test, enter http://localhost:8787.");
                ui.add_space(12.0);
                ui.add(egui::Slider::new(&mut self.bitrate, 2..=100).suffix(" Mbps").text("Target video bitrate"));
                ui.checkbox(&mut self.allow_clipboard, "Allow remote browser to write my text clipboard");
                ui.checkbox(&mut self.away_access, "Allow my private pairing link to connect while I’m away");
                if self.away_access { ui.small("Anyone with the active pairing code can control the unlocked PC until you stop sharing."); }
            });
            ui.add_space(16.0);
            if running {
                if ui.add_sized([ui.available_width(),38.0],egui::Button::new("Stop sharing & disconnect").fill(egui::Color32::from_rgb(105,42,50))).clicked() { self.command(Command::Stop); }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("Pairing code  {}", self.pairing_code)).size(21.0).strong());
                    if ui.button("Copy code").clicked() { ctx.copy_text(self.pairing_code.clone()); }
                });
                ui.small("Enter this code on the XanDesk website. It is valid only while sharing is active.");
            } else if ui.add_sized([ui.available_width(),38.0],egui::Button::new("Start sharing").fill(egui::Color32::from_rgb(31,91,68))).clicked() { self.start(); }
            ui.add_space(16.0); ui.separator();
            let s = self.status.lock().unwrap();
            ui.add_space(10.0); ui.label(&s.message);
            if s.pending {
                ui.add_space(12.0);
                ui.colored_label(egui::Color32::YELLOW,"Someone with your pairing key wants to see and control this PC.");
                ui.horizontal(|ui| { if ui.button("Allow this session").clicked() { self.command(Command::Approve); } if ui.button("Decline").clicked() { self.command(Command::Reject); } });
            }
            ui.add_space(18.0);
            ui.horizontal(|ui| {
                ui.label(format!("Capture  {:.1} FPS",s.capture_fps)); ui.separator();
                ui.label(format!("Encode  {:.1} FPS",s.encoded_fps)); ui.separator();
                ui.label(format!("{} × {}",s.dimensions.0,s.dimensions.1));
            });
            ui.add_space(18.0); ui.separator(); ui.add_space(8.0);
            ui.small("No subscriptions. No daily timer. No application FPS cap.");
            ui.small("Primary monitor · Software H.264 · Keyboard / mouse · Optional text clipboard");
            ui.small("Keep Windows unlocked. Audio, controllers and sign-in-screen access are not in this preview.");
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
                .with_inner_size([600.0, 580.0])
                .with_min_inner_size([550.0, 560.0]),
            ..Default::default()
        },
        Box::new(move |cc| Ok(Box::new(App::new(cc, runtime)))),
    )
}
