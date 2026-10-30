//! Remote Desk: the everyday app. One window, the same on both computers: your IP address, a HOST IP field
//! to control another PC, and a button to share this one.
//!
//! Optional arguments (handy for testing): `--host` starts hosting on launch, `--connect <ip[:port]>` fills in
//! the HOST IP field and connects, `--port <n>` sets the port.

#![windows_subsystem = "windows"]

mod gui;

fn install_panic_hook() {
    // A windows-subsystem program has no console, so keep a record of crashes where the user can find it.
    std::panic::set_hook(Box::new(|info| {
        let path = std::env::temp_dir().join("remote-desk-crash.log");
        let _ = std::fs::write(&path, format!("{info}\n"));
        remote_desk::log::say(format!("Neočekivana greška: {info}"));
    }));
}

fn main() -> eframe::Result {
    install_panic_hook();
    let startup = gui::Startup::from_args(std::env::args().skip(1).collect());
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Remote Desk")
            .with_inner_size([580.0, 900.0])
            .with_min_inner_size([500.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native("Remote Desk", options, Box::new(|cc| Ok(Box::new(gui::App::new(cc, startup)))))
}
