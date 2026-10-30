//! remote-desk: low-latency screen sharing and remote control for Windows.
//! Shared by the GUI app (`remote-desk.exe`) and the command-line tool (`remote-desk-cli.exe`).

pub mod capture;
pub mod client;
pub mod decoder;
pub mod encoder;
pub mod host;
pub mod input;
pub mod local;
pub mod log;
pub mod netinfo;
pub mod probe;
pub mod proto;
pub mod status;
pub mod viewer;
