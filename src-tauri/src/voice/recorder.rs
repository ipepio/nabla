//! Native cross-platform microphone capture via `cpal` (CoreAudio / WASAPI / ALSA).
//!
//! Runs the `cpal::Stream` on a dedicated OS thread (since `Stream` is `!Send`
//! on some backends) and collects mono `f32` samples until `stop()` or `cancel()`.

use std::sync::{Arc, Mutex, mpsc};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

struct ActiveCapture {
    stop_tx: mpsc::Sender<bool>, // true = return samples, false = cancel
    result_rx: mpsc::Receiver<Result<(Vec<f32>, u32), String>>,
}

/// Manages one active native microphone recording session.
#[derive(Default)]
pub struct VoiceRecorderState {
    active: Mutex<Option<ActiveCapture>>,
}

impl VoiceRecorderState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start(&self) -> Result<(), String> {
        let mut guard = self
            .active
            .lock()
            .map_err(|_| "recorder lock poisoned".to_string())?;
        if guard.is_some() {
            return Ok(());
        }

        let (ready_tx, ready_rx) = mpsc::channel::<Result<(), String>>();
        let (stop_tx, stop_rx) = mpsc::channel::<bool>();
        let (result_tx, result_rx) = mpsc::channel::<Result<(Vec<f32>, u32), String>>();

        std::thread::Builder::new()
            .name("nabla-mic-capture".to_string())
            .spawn(move || {
                capture_thread(ready_tx, stop_rx, result_tx);
            })
            .map_err(|e| format!("failed to spawn microphone thread: {e}"))?;

        ready_rx
            .recv()
            .map_err(|_| "microphone thread exited unexpectedly".to_string())??;

        *guard = Some(ActiveCapture { stop_tx, result_rx });
        Ok(())
    }

    pub fn stop(&self) -> Result<(Vec<f32>, u32), String> {
        let capture = {
            let mut guard = self
                .active
                .lock()
                .map_err(|_| "recorder lock poisoned".to_string())?;
            guard.take()
        }
        .ok_or_else(|| "no active recording".to_string())?;

        let _ = capture.stop_tx.send(true);
        capture
            .result_rx
            .recv()
            .map_err(|_| "microphone capture did not return audio".to_string())?
    }

    pub fn cancel(&self) {
        if let Ok(mut guard) = self.active.lock()
            && let Some(capture) = guard.take()
        {
            let _ = capture.stop_tx.send(false);
        }
    }
}

fn capture_thread(
    ready_tx: mpsc::Sender<Result<(), String>>,
    stop_rx: mpsc::Receiver<bool>,
    result_tx: mpsc::Sender<Result<(Vec<f32>, u32), String>>,
) {
    let host = cpal::default_host();
    let Some(device) = host.default_input_device() else {
        let _ = ready_tx.send(Err("No microphone input device found.".to_string()));
        return;
    };
    let supported = match device.default_input_config() {
        Ok(cfg) => cfg,
        Err(e) => {
            let _ = ready_tx.send(Err(format!(
                "Cannot open microphone (check OS microphone permission): {e}"
            )));
            return;
        }
    };

    let sample_rate = supported.sample_rate().0;
    let channels = supported.channels() as usize;
    let samples: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::with_capacity(
        (sample_rate as usize) * 10,
    )));
    let err_fn = |err: cpal::StreamError| {
        eprintln!("[nabla-voice] microphone stream error: {err}");
    };

    let buf_clone = Arc::clone(&samples);
    let stream_res = match supported.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &supported.into(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                push_mono_f32(&buf_clone, data, channels);
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            &supported.into(),
            move |data: &[i16], _: &cpal::InputCallbackInfo| {
                if let Ok(mut out) = buf_clone.lock() {
                    for frame in data.chunks(channels.max(1)) {
                        let sum: f32 = frame.iter().map(|&s| (s as f32) / 32768.0).sum();
                        out.push(sum / (frame.len().max(1) as f32));
                    }
                }
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            &supported.into(),
            move |data: &[u16], _: &cpal::InputCallbackInfo| {
                if let Ok(mut out) = buf_clone.lock() {
                    for frame in data.chunks(channels.max(1)) {
                        let sum: f32 = frame
                            .iter()
                            .map(|&s| ((s as f32) - 32768.0) / 32768.0)
                            .sum();
                        out.push(sum / (frame.len().max(1) as f32));
                    }
                }
            },
            err_fn,
            None,
        ),
        other => {
            let _ = ready_tx.send(Err(format!("Unsupported microphone format: {other:?}")));
            return;
        }
    };

    let stream = match stream_res {
        Ok(s) => s,
        Err(e) => {
            let _ = ready_tx.send(Err(format!("Failed to build microphone stream: {e}")));
            return;
        }
    };

    if let Err(e) = stream.play() {
        let _ = ready_tx.send(Err(format!("Failed to start microphone capture: {e}")));
        return;
    }

    let _ = ready_tx.send(Ok(()));

    let keep = stop_rx.recv().unwrap_or(false);
    drop(stream);

    if keep {
        let collected = samples
            .lock()
            .map(|mut g| std::mem::take(&mut *g))
            .unwrap_or_default();
        let _ = result_tx.send(Ok((collected, sample_rate)));
    }
}

fn push_mono_f32(dest: &Mutex<Vec<f32>>, data: &[f32], channels: usize) {
    if let Ok(mut out) = dest.lock() {
        let ch = channels.max(1);
        for frame in data.chunks(ch) {
            let sum: f32 = frame.iter().copied().sum();
            out.push(sum / (frame.len().max(1) as f32));
        }
    }
}
