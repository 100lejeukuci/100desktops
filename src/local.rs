//! Single-machine benchmark: capture -> encode -> decode -> display, no network.

use crate::{capture, decoder, encoder, viewer};
use anyhow::Result;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

fn report(name: &str, mut v: Vec<Duration>) {
    if v.is_empty() {
        println!("{name}: no samples");
        return;
    }
    v.sort();
    let at = |p: f64| v[((v.len() - 1) as f64 * p) as usize];
    println!("{name}: p50 {:?}, p99 {:?}, max {:?}", at(0.5), at(0.99), v.last().unwrap());
}

pub fn run() -> Result<()> {
    let cap = capture::Capturer::new()?;
    let tex = cap.frame_texture()?;
    let fps = 60;
    let mut enc = encoder::Encoder::new(&cap.device, &cap.context, &tex, cap.width, cap.height, fps, 20_000_000)?;
    let mut dec = decoder::Decoder::new(&cap.device)?;
    let mut viewer = viewer::Viewer::new(&cap.device, &cap.context, cap.width, cap.height, 1280, 720)?;
    println!("Local: capture -> encode -> decode -> display {}x{} @ {fps} fps", cap.width, cap.height);

    let start = Instant::now();
    let (mut frames, mut decoded, mut bytes) = (0u32, 0u32, 0usize);
    let mut in_flight: VecDeque<Instant> = VecDeque::new();
    let (mut enc_lat, mut total_lat, mut dec_lat) = (Vec::new(), Vec::new(), Vec::new());
    while start.elapsed() < Duration::from_secs(8) && viewer.pump() {
        if let capture::Frame::New = cap.next_frame(&tex, 100)? {
            frames += 1;
            in_flight.push_back(Instant::now());
            for p in enc.encode()? {
                bytes += p.data.len();
                enc_lat.push(p.latency);
                let t = Instant::now();
                let out = dec.decode(&p.data)?;
                dec_lat.push(t.elapsed());
                for frame in out {
                    decoded += 1;
                    viewer.present(&frame)?;
                    if let Some(t) = in_flight.pop_front() {
                        total_lat.push(t.elapsed());
                    }
                }
            }
        }
    }
    let secs = start.elapsed().as_secs_f64();
    println!("{frames} captured, {decoded} decoded, {:.1} Mbit/s", bytes as f64 * 8.0 / secs / 1e6);
    report("convert+encode", enc_lat);
    report("decode() call", dec_lat);
    report("capture->decoded+presented (no network)", total_lat);
    Ok(())
}
