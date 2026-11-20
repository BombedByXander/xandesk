use crate::Shared;
use anyhow::Context;
use openh264::{
    encoder::{BitRate, Encoder, EncoderConfig, FrameRate, Level, Profile, UsageType},
    formats::{BgraSliceU8, YUVBuffer},
    OpenH264API, Timestamp,
};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

pub struct Frame {
    pub data: Vec<u8>,
    pub captured: Instant,
}

fn encoder(bitrate: u32) -> anyhow::Result<Encoder> {
    // OpenH264's nominal rate is used for encoder rate-control accounting, not pacing.
    // skip_frames(false) submits every selected frame, including timestamps above 120 Hz.
    let config = EncoderConfig::new()
        .bitrate(BitRate::from_bps(bitrate * 1_000_000))
        .max_frame_rate(FrameRate::from_hz(120.0))
        .skip_frames(false)
        // Preserve fine edges such as UI text with spatial quality adaptation.
        .adaptive_quantization(true)
        .background_detection(false)
        .profile(Profile::Baseline)
        .level(Level::Level_5_1)
        .usage_type(UsageType::ScreenContentRealTime);
    Ok(Encoder::with_api_config(
        OpenH264API::from_source(),
        config,
    )?)
}

// Runs on a dedicated OS thread because DXGI resources are thread-affine.
// No timer paces produced frames. DXGI availability and encoder throughput drive capture.
pub fn run(
    tx: mpsc::Sender<Frame>,
    stopped: Arc<AtomicBool>,
    keyframe: Arc<AtomicBool>,
    bitrate: u32,
    status: Shared,
) -> anyhow::Result<()> {
    let mut retry_delay = Duration::from_millis(500);
    loop {
        if stopped.load(Ordering::Acquire) {
            return Ok(());
        }
        if let Err(error) = run_attempt(
            tx.clone(), stopped.clone(), keyframe.clone(), bitrate, status.clone(),
        ) {
            if stopped.load(Ordering::Acquire) {
                return Ok(());
            }
            status.lock().unwrap().message = format!(
                "Desktop capture interrupted: {error:#}. Retrying…"
            );
            let until = Instant::now() + retry_delay;
            while !stopped.load(Ordering::Acquire) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(100));
            }
            retry_delay = (retry_delay * 2).min(Duration::from_secs(10));
        } else {
            return Ok(());
        }
    }
}

fn run_attempt(
    tx: mpsc::Sender<Frame>,
    stopped: Arc<AtomicBool>,
    keyframe: Arc<AtomicBool>,
    bitrate: u32,
    status: Shared,
) -> anyhow::Result<()> {
    let display = scrap::Display::primary().context("Could not open the primary monitor")?;
    let mut capturer = scrap::Capturer::new(display).context("DXGI desktop capture failed")?;
    let (width, height) = (capturer.width(), capturer.height());
    anyhow::ensure!(
        width % 2 == 0 && height % 2 == 0,
        "H.264 requires an even-sized monitor resolution"
    );
    status.lock().unwrap().dimensions = (width, height);
    {
        let mut state = status.lock().unwrap();
        if state.message.starts_with("Desktop capture interrupted:") {
            state.message = "Desktop capture restored. The host remains online.".into();
        }
    }
    let mut encoder = encoder(bitrate)?;
    let mut pixels = vec![0u8; width * height * 4];
    let mut yuv = YUVBuffer::new(width, height);
    let mut last_keyframe = Instant::now() - Duration::from_secs(5);
    let mut report = Instant::now();
    let mut captured_count = 0u64;
    let mut encoded_count = 0u64;
    let started = Instant::now();
    while !stopped.load(Ordering::Acquire) {
        if report.elapsed() >= Duration::from_secs(1) {
            let seconds = report.elapsed().as_secs_f64();
            let mut s = status.lock().unwrap();
            s.capture_fps = captured_count as f64 / seconds;
            s.encoded_fps = encoded_count as f64 / seconds;
            captured_count = 0;
            encoded_count = 0;
            report = Instant::now();
        }
        let frame =
            match capturer.frame() {
                Ok(frame) => frame,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
                Err(e) => return Err(e).context(
                    "Desktop capture ended (screen locked, display changed, or GPU unavailable)",
                ),
            };
        let captured = Instant::now();
        captured_count += 1;
        // Backpressure before encoding preserves H.264 reference continuity. Never drop encoded P frames.
        let permit = match tx.try_reserve() {
            Ok(p) => p,
            Err(mpsc::error::TrySendError::Full(_)) => continue,
            Err(_) => break,
        };
        let stride = frame.len() / height;
        for row in 0..height {
            pixels[row * width * 4..(row + 1) * width * 4]
                .copy_from_slice(&frame[row * stride..row * stride + width * 4]);
        }
        drop(frame);
        yuv.read_bgra8(BgraSliceU8::new(&pixels, (width, height)));
        if keyframe.swap(false, Ordering::AcqRel)
            || last_keyframe.elapsed() >= Duration::from_secs(2)
        {
            encoder.force_intra_frame();
            last_keyframe = Instant::now();
        }
        let data = encoder
            .encode_at(
                &yuv,
                Timestamp::from_millis(started.elapsed().as_millis() as u64),
            )?
            .to_vec();
        if !data.is_empty() {
            encoded_count += 1;
            permit.send(Frame { data, captured });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encoder_does_not_gate_frames_at_60_or_120_fps() -> anyhow::Result<()> {
        let mut encoder = encoder(5)?;
        let pixels = vec![127u8; 64 * 64 * 4];
        let yuv = YUVBuffer::from_bgra8_source(BgraSliceU8::new(&pixels, (64, 64)));
        for n in 0..240 {
            let bytes = encoder
                .encode_at(&yuv, Timestamp::from_millis(n * 2))?
                .to_vec();
            anyhow::ensure!(
                !bytes.is_empty(),
                "Encoder dropped frame {n} despite disabled frame skipping"
            );
        }
        Ok(())
    }
}
