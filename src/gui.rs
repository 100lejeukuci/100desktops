//! The window: this PC's address, HOST/VIEWER controls, live status and an event log.

use eframe::egui::{self, Align, Align2, Color32, CornerRadius, Layout, Margin, RichText, Sense, Stroke, vec2};
use remote_desk::host::Permission;
use remote_desk::status::{self, HostStatus, ViewerStatus};
use remote_desk::{client, host, log, netinfo};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const DEFAULT_PORT: u16 = 7000;
const FIREWALL_RULE: &str = "Remote Desk";

// Colors
const BG: Color32 = Color32::from_rgb(0x12, 0x14, 0x1a);
const CARD: Color32 = Color32::from_rgb(0x1b, 0x1f, 0x29);
const LINE: Color32 = Color32::from_rgb(0x2b, 0x31, 0x3f);
const TEXT: Color32 = Color32::from_rgb(0xe6, 0xe9, 0xf0);
const MUTED: Color32 = Color32::from_rgb(0x8b, 0x93, 0xa7);
const ACCENT: Color32 = Color32::from_rgb(0x4c, 0x8d, 0xff);
const GREEN: Color32 = Color32::from_rgb(0x3e, 0xcf, 0x8e);
const AMBER: Color32 = Color32::from_rgb(0xf2, 0xb1, 0x34);
const RED: Color32 = Color32::from_rgb(0xef, 0x53, 0x50);
const IDLE: Color32 = Color32::from_rgb(0x5b, 0x62, 0x73);

/// Options given on the command line.
#[derive(Default)]
pub struct Startup {
    host: bool,
    connect: Option<String>,
    port: Option<u16>,
}

impl Startup {
    pub fn from_args(args: Vec<String>) -> Self {
        let mut s = Startup::default();
        let mut it = args.into_iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--host" => s.host = true,
                "--connect" => s.connect = it.next(),
                "--port" => s.port = it.next().and_then(|p| p.parse().ok()),
                _ => {}
            }
        }
        s
    }
}

struct ConsentRequest {
    peer: SocketAddr,
    reply: Sender<Permission>,
}

struct Worker {
    flag: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

enum Action {
    StartHost,
    StopHost,
    Connect,
    Disconnect,
    RefreshIps,
    Copy(String),
    AllowFirewall,
    Answer(Permission),
}

pub struct App {
    ctx: egui::Context,
    ips: Vec<Ipv4Addr>,
    port_input: String,
    host_ip_input: String,
    host_worker: Option<Worker>,
    viewer_worker: Option<Worker>,
    consent_tx: Sender<ConsentRequest>,
    consent_rx: Receiver<ConsentRequest>,
    pending_consent: Option<ConsentRequest>,
    form_error: Option<String>,
    copied_at: Option<Instant>,
    firewall_ok: Arc<Mutex<Option<bool>>>,
    firewall_note: Option<String>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, startup: Startup) -> Self {
        apply_style(&cc.egui_ctx);
        let (saved_ip, saved_port) = load_settings();
        let (consent_tx, consent_rx) = mpsc::channel();
        let mut app = App {
            ctx: cc.egui_ctx.clone(),
            ips: netinfo::local_ips(),
            port_input: startup.port.map(|p| p.to_string()).unwrap_or(saved_port),
            host_ip_input: startup.connect.clone().unwrap_or(saved_ip),
            host_worker: None,
            viewer_worker: None,
            consent_tx,
            consent_rx,
            pending_consent: None,
            form_error: None,
            copied_at: None,
            firewall_ok: Arc::new(Mutex::new(None)),
            firewall_note: None,
        };
        log::say("Aplikacija je pokrenuta.");
        app.check_firewall();
        if startup.host {
            app.start_host();
        }
        if startup.connect.is_some() {
            app.connect();
        }
        app
    }

    fn port(&self) -> Option<u16> {
        self.port_input.trim().parse().ok().filter(|p| *p > 0)
    }

    fn primary_ip_text(&self) -> String {
        self.ips.first().map(|ip| ip.to_string()).unwrap_or_else(|| "nepoznata".into())
    }

    // ------------------------------------------------------------------ host

    fn host_running(&self) -> bool {
        self.host_worker.is_some()
    }

    fn start_host(&mut self) {
        let Some(port) = self.port() else {
            self.form_error = Some("Port mora biti broj između 1 i 65535.".into());
            return;
        };
        self.form_error = None;
        save_settings(&self.host_ip_input, &self.port_input);
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let tx = self.consent_tx.clone();
        let ctx = self.ctx.clone();
        let handle = std::thread::spawn(move || {
            let ask = |peer: SocketAddr| -> Permission {
                let (reply, answer) = mpsc::channel();
                if tx.send(ConsentRequest { peer, reply }).is_err() {
                    return Permission::Deny;
                }
                ctx.request_repaint();
                let asked = Instant::now();
                loop {
                    match answer.recv_timeout(Duration::from_millis(250)) {
                        Ok(p) => return p,
                        Err(_) if worker_stop.load(Ordering::Relaxed) => return Permission::Deny,
                        Err(_) if asked.elapsed() > Duration::from_secs(90) => return Permission::Deny,
                        Err(_) => {}
                    }
                }
            };
            if let Err(e) = host::run(port, ask, &worker_stop) {
                log::say(format!("Host se zaustavio zbog greške: {e:#}"));
            }
        });
        self.host_worker = Some(Worker { flag: stop, handle });
    }

    fn stop_host(&mut self) {
        if let Some(w) = &self.host_worker {
            w.flag.store(true, Ordering::Relaxed);
        }
    }

    // ---------------------------------------------------------------- viewer

    fn connect(&mut self) {
        let Some(port) = self.port() else {
            self.form_error = Some("Port mora biti broj između 1 i 65535.".into());
            return;
        };
        let addr = match parse_target(&self.host_ip_input, port) {
            Ok(a) => a,
            Err(e) => {
                self.form_error = Some(e);
                return;
            }
        };
        self.form_error = None;
        save_settings(&self.host_ip_input, &self.port_input);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let handle = std::thread::spawn(move || {
            let opts = client::Options { seconds: None, screenshot: false, drop_percent: 0, script: false };
            if let Err(e) = client::run(addr, opts, &worker_cancel) {
                log::say(format!("Greška: {e:#}"));
            }
        });
        self.viewer_worker = Some(Worker { flag: cancel, handle });
    }

    fn disconnect(&mut self) {
        if let Some(w) = &self.viewer_worker {
            w.flag.store(true, Ordering::Relaxed);
        }
    }

    // -------------------------------------------------------------- firewall

    fn check_firewall(&self) {
        let slot = self.firewall_ok.clone();
        let port = self.port().unwrap_or(DEFAULT_PORT);
        std::thread::spawn(move || {
            let ok = firewall_rule_present(port);
            if let Ok(mut g) = slot.lock() {
                *g = Some(ok);
            }
        });
    }

    fn allow_firewall(&mut self) {
        let Some(port) = self.port() else {
            self.form_error = Some("Port mora biti broj između 1 i 65535.".into());
            return;
        };
        match request_firewall_rule(port) {
            Ok(()) => {
                self.firewall_note = Some("Potvrdi Windows prozor (UAC). Status će se osvežiti za nekoliko sekundi.".into());
                let slot = self.firewall_ok.clone();
                let ctx = self.ctx.clone();
                std::thread::spawn(move || {
                    // Give the user time to answer the UAC prompt, then look again.
                    for _ in 0..6 {
                        std::thread::sleep(Duration::from_secs(3));
                        let ok = firewall_rule_present(port);
                        if let Ok(mut g) = slot.lock() {
                            *g = Some(ok);
                        }
                        ctx.request_repaint();
                        if ok {
                            break;
                        }
                    }
                });
            }
            Err(e) => self.firewall_note = Some(e),
        }
    }

    // ----------------------------------------------------------------- misc

    fn reap_finished(&mut self) {
        if self.host_worker.as_ref().is_some_and(|w| w.handle.is_finished()) {
            if let Some(w) = self.host_worker.take() {
                let _ = w.handle.join();
            }
        }
        if self.viewer_worker.as_ref().is_some_and(|w| w.handle.is_finished()) {
            if let Some(w) = self.viewer_worker.take() {
                let _ = w.handle.join();
            }
        }
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::StartHost => self.start_host(),
            Action::StopHost => self.stop_host(),
            Action::Connect => self.connect(),
            Action::Disconnect => self.disconnect(),
            Action::RefreshIps => {
                self.ips = netinfo::local_ips();
                self.check_firewall();
            }
            Action::Copy(text) => {
                self.ctx.copy_text(text);
                self.copied_at = Some(Instant::now());
            }
            Action::AllowFirewall => self.allow_firewall(),
            Action::Answer(p) => {
                if let Some(req) = self.pending_consent.take() {
                    let _ = req.reply.send(p);
                }
            }
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.reap_finished();
        // New connection request from a viewer: bring the window to the front so it cannot be missed.
        while let Ok(req) = self.consent_rx.try_recv() {
            if self.pending_consent.is_some() {
                let _ = req.reply.send(Permission::Deny);
            } else {
                self.pending_consent = Some(req);
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Critical));
            }
        }
        // The host answers on its own (timeout / stop): drop a dialog nobody can answer any more.
        if self.pending_consent.is_some() && !self.host_running() {
            self.pending_consent = None;
        }
        ctx.request_repaint_after(Duration::from_millis(250));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let mut action: Option<Action> = None;

        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(Margin::same(16))).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                self.header(ui);
                ui.add_space(12.0);
                self.this_pc_card(ui, &mut action);
                ui.add_space(12.0);
                self.host_card(ui, &mut action);
                ui.add_space(12.0);
                self.viewer_card(ui, &mut action);
                ui.add_space(12.0);
                self.log_card(ui);
            });
        });

        self.consent_dialog(&ctx, &mut action);

        if let Some(a) = action {
            self.apply(a);
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        // Let both workers finish cleanly (the host releases any keys it was holding for the viewer).
        for worker in [self.host_worker.take(), self.viewer_worker.take()].into_iter().flatten() {
            worker.flag.store(true, Ordering::Relaxed);
            let _ = worker.handle.join();
        }
    }
}

// ------------------------------------------------------------------- panels

impl App {
    fn header(&self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Remote Desk").size(28.0).strong().color(TEXT));
        ui.label(RichText::new("Daljinski pristup ekranu sa što manjom latencijom").size(13.5).color(MUTED));
    }

    fn this_pc_card(&mut self, ui: &mut egui::Ui, action: &mut Option<Action>) {
        card(ui, "OVAJ RAČUNAR", "Tvoja IP adresa u mreži", |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(self.primary_ip_text()).monospace().size(30.0).strong().color(TEXT));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(secondary_button("Osveži")).clicked() {
                        *action = Some(Action::RefreshIps);
                    }
                    let copied = self.copied_at.is_some_and(|t| t.elapsed() < Duration::from_millis(1500));
                    let label = if copied { "Kopirano" } else { "Kopiraj" };
                    if ui.add(secondary_button(label)).clicked() {
                        *action = Some(Action::Copy(self.primary_ip_text()));
                    }
                });
            });
            if self.ips.len() > 1 {
                let others: Vec<String> = self.ips.iter().skip(1).map(|ip| ip.to_string()).collect();
                ui.label(RichText::new(format!("Ostale adrese: {}", others.join(", "))).size(12.5).color(MUTED));
            }
            ui.label(
                RichText::new("Ovu adresu upiši u polje „HOST IP“ na drugom računaru kad ovaj deli ekran.")
                    .size(12.5)
                    .color(MUTED),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Port").color(MUTED));
                ui.add(
                    egui::TextEdit::singleline(&mut self.port_input)
                        .desired_width(70.0)
                        .font(egui::TextStyle::Monospace),
                );
                ui.label(RichText::new("(UDP, isti na oba računara)").size(12.5).color(MUTED));
            });
            if let Some(err) = &self.form_error {
                ui.label(RichText::new(err.as_str()).color(RED));
            }
        });
    }

    fn host_card(&mut self, ui: &mut egui::Ui, action: &mut Option<Action>) {
        card(ui, "HOST", "Deli ekran ovog računara", |ui| {
            let running = self.host_running();
            let stopping = running && self.host_worker.as_ref().is_some_and(|w| w.flag.load(Ordering::Relaxed));
            let (color, text) = match status::host() {
                _ if stopping => (AMBER, "Zaustavljanje...".to_string()),
                HostStatus::Off => (IDLE, "Isključen".to_string()),
                HostStatus::Waiting { port } => {
                    (GREEN, format!("Čeka viewer-a na {}:{port}", self.primary_ip_text()))
                }
                HostStatus::AwaitingConsent(peer) => (AMBER, format!("Zahtev od {peer} čeka tvoj odgovor")),
                HostStatus::Streaming { peer, control } => (
                    GREEN,
                    format!("Prenosi ekran ka {peer} ({})", if control { "kontrola dozvoljena" } else { "samo gledanje" }),
                ),
            };
            status_line(ui, color, &text);
            ui.add_space(4.0);
            if running {
                if ui.add_enabled(!stopping, danger_button("Zaustavi hosting")).clicked() {
                    *action = Some(Action::StopHost);
                }
            } else if ui.add(primary_button("Pokreni hosting")).clicked() {
                *action = Some(Action::StartHost);
            }
            ui.add_space(4.0);

            let fw = self.firewall_ok.lock().ok().and_then(|g| *g);
            ui.horizontal(|ui| {
                match fw {
                    Some(true) => status_line(ui, GREEN, "Firewall: dozvoljeno"),
                    Some(false) => status_line(ui, AMBER, "Firewall: nije podešen"),
                    None => status_line(ui, IDLE, "Firewall: proveravam..."),
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if fw != Some(true) && ui.add(secondary_button("Dozvoli u firewall-u")).clicked() {
                        *action = Some(Action::AllowFirewall);
                    }
                });
            });
            if let Some(note) = &self.firewall_note {
                ui.label(RichText::new(note.as_str()).size(12.5).color(MUTED));
            }
            ui.label(
                RichText::new("Svaka veza traži tvoju dozvolu. Hitni prekid kontrole: Ctrl+Alt+Shift+F12.")
                    .size(12.5)
                    .color(MUTED),
            );
        });
    }

    fn viewer_card(&mut self, ui: &mut egui::Ui, action: &mut Option<Action>) {
        card(ui, "VIEWER", "Gledaj i upravljaj drugim računarom", |ui| {
            let busy = self.viewer_worker.is_some();
            ui.label(RichText::new("HOST IP").size(12.5).color(MUTED));
            ui.horizontal(|ui| {
                let field = ui.add_enabled(
                    !busy,
                    egui::TextEdit::singleline(&mut self.host_ip_input)
                        .hint_text("npr. 192.168.100.72")
                        .font(egui::TextStyle::Monospace)
                        .desired_width(ui.available_width() - 130.0),
                );
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if busy {
                    if ui.add(danger_button("Prekini")).clicked() {
                        *action = Some(Action::Disconnect);
                    }
                } else if ui.add(primary_button("Poveži se")).clicked() || enter {
                    *action = Some(Action::Connect);
                }
            });
            ui.add_space(4.0);

            match status::viewer() {
                ViewerStatus::Off => status_line(ui, IDLE, "Nije povezan"),
                ViewerStatus::Connecting { host, hint } => {
                    status_line(ui, AMBER, &format!("Povezivanje na {host}... host mora da odobri vezu"));
                    if let Some(h) = hint {
                        ui.add(egui::Label::new(RichText::new(h).size(12.5).color(AMBER)).wrap());
                    }
                }
                ViewerStatus::Connected { host, width, height, control } => {
                    status_line(
                        ui,
                        GREEN,
                        &format!(
                            "Povezan na {host} ({width}x{height}, {})",
                            if control { "kontrola dozvoljena" } else { "samo gledanje" }
                        ),
                    );
                    let s = status::stats();
                    ui.add_space(2.0);
                    ui.columns(4, |cols| {
                        stat(&mut cols[0], "FPS", format!("{:.0}", s.fps));
                        stat(&mut cols[1], "Protok", format!("{:.1} Mbit/s", s.mbps));
                        stat(&mut cols[2], "Kašnjenje (proc.)", format!("{:.1} ms", s.latency_p50_ms));
                        stat(&mut cols[3], "RTT", format!("{:.1} ms", s.rtt_ms));
                    });
                    ui.label(
                        RichText::new(format!(
                            "p99 kašnjenja {:.1} ms · ponovo poslato paketa {} · odbačeno kadrova {}",
                            s.latency_p99_ms, s.nacks, s.frames_given_up
                        ))
                        .size(12.0)
                        .color(MUTED),
                    );
                    if control {
                        ui.label(
                            RichText::new("Miš i tastatura u prozoru sa ekranom upravljaju drugim računarom.")
                                .size(12.5)
                                .color(MUTED),
                        );
                    }
                }
            }
        });
    }

    fn log_card(&self, ui: &mut egui::Ui) {
        card(ui, "DNEVNIK", "Šta se dešava", |ui| {
            egui::ScrollArea::vertical().max_height(110.0).stick_to_bottom(true).auto_shrink([false, true]).show(ui, |ui| {
                for line in log::lines() {
                    ui.add(egui::Label::new(RichText::new(line).monospace().size(12.0).color(MUTED)).wrap());
                }
            });
        });
    }

    fn consent_dialog(&mut self, ctx: &egui::Context, action: &mut Option<Action>) {
        let Some(req) = &self.pending_consent else { return };
        let peer = req.peer;
        egui::Window::new("Zahtev za povezivanje")
            .collapsible(false)
            .resizable(false)
            .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
            .frame(
                egui::Frame::new()
                    .fill(CARD)
                    .stroke(Stroke::new(1.5, ACCENT))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(Margin::same(18)),
            )
            .show(ctx, |ui| {
                ui.set_width(380.0);
                ui.label(RichText::new(format!("{peer} želi da se poveže na ovaj računar.")).size(16.0).color(TEXT));
                ui.add_space(4.0);
                ui.label(
                    RichText::new("Dozvoli samo ako znaš ko se povezuje. Sa kontrolom, druga strana upravlja tvojim mišem i tastaturom.")
                        .size(12.5)
                        .color(MUTED),
                );
                ui.add_space(10.0);
                if ui.add_sized([ui.available_width(), 34.0], primary_button("Dozvoli gledanje i kontrolu")).clicked() {
                    *action = Some(Action::Answer(Permission::Control));
                }
                if ui.add_sized([ui.available_width(), 34.0], secondary_button("Dozvoli samo gledanje")).clicked() {
                    *action = Some(Action::Answer(Permission::ViewOnly));
                }
                if ui.add_sized([ui.available_width(), 34.0], danger_button("Odbij")).clicked() {
                    *action = Some(Action::Answer(Permission::Deny));
                }
            });
    }
}

// ------------------------------------------------------------------ widgets

fn card<R>(ui: &mut egui::Ui, tag: &str, title: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(tag).size(11.0).strong().color(ACCENT));
            ui.label(RichText::new(title).size(17.0).strong().color(TEXT));
            ui.add_space(6.0);
            add(ui)
        })
        .inner
}

fn status_line(ui: &mut egui::Ui, color: Color32, text: &str) {
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
        ui.painter().circle_filled(rect.center(), 5.5, color);
        ui.label(RichText::new(text).size(14.5).color(TEXT));
    });
}

fn stat(ui: &mut egui::Ui, name: &str, value: String) {
    ui.vertical(|ui| {
        ui.add(egui::Label::new(RichText::new(name).size(11.5).color(MUTED)).extend());
        ui.add(egui::Label::new(RichText::new(value).size(16.0).strong().color(TEXT)).extend());
    });
}

fn primary_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_string()).strong().color(Color32::WHITE))
        .fill(ACCENT)
        .corner_radius(CornerRadius::same(8))
        .min_size(vec2(110.0, 32.0))
}

fn secondary_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_string()).color(TEXT))
        .fill(Color32::from_rgb(0x2a, 0x30, 0x3e))
        .corner_radius(CornerRadius::same(8))
        .min_size(vec2(90.0, 30.0))
}

fn danger_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_string()).strong().color(Color32::WHITE))
        .fill(Color32::from_rgb(0xb4, 0x3a, 0x3a))
        .corner_radius(CornerRadius::same(8))
        .min_size(vec2(110.0, 32.0))
}

fn apply_style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = CARD;
    v.extreme_bg_color = Color32::from_rgb(0x0e, 0x10, 0x15); // text field background
    v.override_text_color = None;
    v.selection.bg_fill = ACCENT.linear_multiply(0.45);
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = vec2(10.0, 8.0);
        s.spacing.button_padding = vec2(14.0, 6.0);
        s.spacing.interact_size.y = 30.0;
    });
}

// ---------------------------------------------------------------- utilities

fn parse_target(input: &str, default_port: u16) -> Result<SocketAddr, String> {
    let s = input.trim();
    if s.is_empty() {
        return Err("Upiši IP adresu hosta (polje „HOST IP“).".into());
    }
    if let Ok(addr) = s.parse::<SocketAddr>() {
        return Ok(addr);
    }
    if let Ok(ip) = s.parse::<Ipv4Addr>() {
        return Ok(SocketAddr::new(ip.into(), default_port));
    }
    Err("Neispravna IP adresa. Primer: 192.168.100.72".into())
}

fn settings_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("remote-desk").join("settings.txt"))
}

/// Last HOST IP and port, remembered between runs.
fn load_settings() -> (String, String) {
    let mut ip = String::new();
    let mut port = DEFAULT_PORT.to_string();
    if let Some(text) = settings_path().and_then(|p| std::fs::read_to_string(p).ok()) {
        for line in text.lines() {
            match line.split_once('=') {
                Some(("host_ip", v)) => ip = v.trim().to_string(),
                Some(("port", v)) if v.trim().parse::<u16>().is_ok() => port = v.trim().to_string(),
                _ => {}
            }
        }
    }
    (ip, port)
}

fn save_settings(host_ip: &str, port: &str) {
    if let Some(path) = settings_path() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, format!("host_ip={}\nport={}\n", host_ip.trim(), port.trim()));
    }
}

/// True when the "Remote Desk" inbound rule exists for this exe and port (reading rules needs no admin rights).
fn firewall_rule_present(port: u16) -> bool {
    use std::os::windows::process::CommandExt;
    let Ok(exe) = std::env::current_exe() else { return false };
    let out = std::process::Command::new("netsh")
        .args(["advfirewall", "firewall", "show", "rule", &format!("name={FIREWALL_RULE}"), "verbose"])
        .creation_flags(0x0800_0000)
        .output();
    let Ok(out) = out else { return false };
    let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
    text.contains(&exe.to_string_lossy().to_lowercase()) && text.contains(&port.to_string())
}

/// Ask Windows (UAC prompt) to add the inbound UDP rule. The user has to approve it; nothing is changed silently.
fn request_firewall_rule(port: u16) -> Result<(), String> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
    use windows::core::{HSTRING, w};

    let exe = std::env::current_exe().map_err(|e| format!("Ne mogu da nađem putanju programa: {e}"))?;
    let args = format!(
        "/c netsh advfirewall firewall delete rule name=\"{FIREWALL_RULE}\" >nul 2>&1 & \
         netsh advfirewall firewall add rule name=\"{FIREWALL_RULE}\" dir=in action=allow protocol=UDP \
         localport={port} program=\"{}\" profile=any",
        exe.display()
    );
    let result = unsafe { ShellExecuteW(None, w!("runas"), w!("cmd.exe"), &HSTRING::from(args), None, SW_HIDE) };
    if result.0 as isize > 32 {
        Ok(())
    } else {
        Err("Windows nije pokrenuo zahtev za administratorska prava (odbijeno ili otkazano).".into())
    }
}
