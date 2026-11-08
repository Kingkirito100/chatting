//! Аудио для звонков: микрофон → PCM 16 кГц → сеть, сеть → динамики.
//!
//! Без Opus: для голоса в компании друзей достаточно сырого PCM 16 кГц
//! (32 КБ/с). Буферы и ресемплинг — свои, без внешних зависимостей.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use krug_client::Client;

const RATE: u32 = 16_000;
const FRAME: usize = 320; // 20 мс

pub struct AudioEngine {
    in_rate: u32,
    out_rate: u32,
    capture: Arc<Mutex<VecDeque<f32>>>,
    playback: Arc<Mutex<VecDeque<f32>>>,
    pending: Vec<i16>,
    phase: f32,
    _input: cpal::Stream,
    _output: cpal::Stream,
    pub muted: bool,
}

fn push_mono_f32(data: &[f32], channels: usize, rx: &Arc<Mutex<VecDeque<f32>>>) {
    let mut q = rx.lock().unwrap_or_else(|e| e.into_inner());
    for frame in data.chunks(channels.max(1)) {
        let sum: f32 = frame.iter().sum();
        q.push_back(sum / frame.len().max(1) as f32);
    }
    while q.len() > 48_000 {
        q.pop_front();
    }
}

fn push_mono_i16(data: &[i16], channels: usize, rx: &Arc<Mutex<VecDeque<f32>>>) {
    let mut q = rx.lock().unwrap_or_else(|e| e.into_inner());
    for frame in data.chunks(channels.max(1)) {
        let sum: f32 = frame.iter().map(|s| *s as f32 / 32768.0).sum();
        q.push_back(sum / frame.len().max(1) as f32);
    }
    while q.len() > 48_000 {
        q.pop_front();
    }
}

impl AudioEngine {
    pub fn start() -> Result<AudioEngine, String> {
        let host = cpal::default_host();
        let input = host.default_input_device().ok_or("нет микрофона")?;
        let output = host.default_output_device().ok_or("нет динамиков")?;
        let in_cfg = input.default_input_config().map_err(|e| e.to_string())?;
        let out_cfg = output.default_output_config().map_err(|e| e.to_string())?;
        let in_rate = in_cfg.sample_rate().0;
        let out_rate = out_cfg.sample_rate().0;
        let in_channels = in_cfg.channels() as usize;
        let out_channels = out_cfg.channels() as usize;

        let capture: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
        let playback: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
        let cap = capture.clone();
        let err = |e| eprintln!("[krug] аудио: {e}");

        let input_stream = match in_cfg.sample_format() {
            cpal::SampleFormat::F32 => input.build_input_stream(
                &in_cfg.into(),
                move |data: &[f32], _| push_mono_f32(data, in_channels, &cap),
                err,
                None,
            ),
            cpal::SampleFormat::I16 => input.build_input_stream(
                &in_cfg.into(),
                move |data: &[i16], _| push_mono_i16(data, in_channels, &cap),
                err,
                None,
            ),
            other => return Err(format!("формат микрофона не поддержан: {other:?}")),
        }
        .map_err(|e| e.to_string())?;
        input_stream.play().map_err(|e| e.to_string())?;

        let play = playback.clone();
        let out_stream = match out_cfg.sample_format() {
            cpal::SampleFormat::F32 => output.build_output_stream(
                &out_cfg.into(),
                move |data: &mut [f32], _| {
                    let mut q = play.lock().unwrap_or_else(|e| e.into_inner());
                    for frame in data.chunks_mut(out_channels) {
                        let sample = q.pop_front().unwrap_or(0.0);
                        for s in frame {
                            *s = sample;
                        }
                    }
                },
                err,
                None,
            ),
            cpal::SampleFormat::I16 => output.build_output_stream(
                &out_cfg.into(),
                move |data: &mut [i16], _| {
                    let mut q = play.lock().unwrap_or_else(|e| e.into_inner());
                    for frame in data.chunks_mut(out_channels) {
                        let sample = q.pop_front().unwrap_or(0.0);
                        for s in frame {
                            *s = (sample * 32767.0) as i16;
                        }
                    }
                },
                err,
                None,
            ),
            other => return Err(format!("формат динамиков не поддержан: {other:?}")),
        }
        .map_err(|e| e.to_string())?;
        out_stream.play().map_err(|e| e.to_string())?;

        Ok(AudioEngine {
            in_rate,
            out_rate,
            capture,
            playback,
            pending: Vec::new(),
            phase: 0.0,
            _input: input_stream,
            _output: out_stream,
            muted: false,
        })
    }

    pub fn in_rate(&self) -> u32 {
        self.in_rate
    }

    pub fn take_capture(&self) -> Vec<f32> {
        let mut q = self.capture.lock().unwrap_or_else(|e| e.into_inner());
        q.drain(..).collect()
    }

    pub fn play_pcm(&self, data: &[u8]) {
        let samples: Vec<i16> = data
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect();
        let step = RATE as f32 / self.out_rate as f32;
        let mut pos = 0.0f32;
        let mut q = self.playback.lock().unwrap_or_else(|e| e.into_inner());
        while pos < samples.len() as f32 {
            q.push_back(samples[pos as usize] as f32 / 32768.0);
            pos += step;
        }
        while q.len() > self.out_rate as usize {
            q.pop_front();
        }
    }

    pub fn capture_rms(&self) -> f32 {
        let q = self.capture.lock().unwrap_or_else(|e| e.into_inner());
        if q.is_empty() {
            return 0.0;
        }
        let sum: f32 = q.iter().map(|s| s * s).sum();
        (sum / q.len() as f32).sqrt()
    }

    pub fn pump(&mut self, client: &Client) {
        let captured: Vec<f32> = {
            let mut q = self.capture.lock().unwrap_or_else(|e| e.into_inner());
            q.drain(..).collect()
        };
        if !self.muted {
            let step = self.in_rate as f32 / RATE as f32;
            for sample in captured {
                self.phase += 1.0;
                if self.phase >= step {
                    self.phase -= step;
                    self.pending.push((sample.clamp(-1.0, 1.0) * 32767.0) as i16);
                }
            }
            while self.pending.len() >= FRAME {
                let mut bytes = Vec::with_capacity(FRAME * 2);
                for sample in self.pending.drain(..FRAME) {
                    bytes.extend_from_slice(&sample.to_le_bytes());
                }
                client.send_voice(bytes);
            }
            if self.pending.len() > FRAME * 8 {
                self.pending.drain(..self.pending.len() - FRAME * 8);
            }
        }

        for (_, data) in client.take_voice() {
            let samples: Vec<i16> = data
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]))
                .collect();
            let step = RATE as f32 / self.out_rate as f32;
            let mut pos = 0.0f32;
            let mut q = self.playback.lock().unwrap_or_else(|e| e.into_inner());
            while pos < samples.len() as f32 {
                q.push_back(samples[pos as usize] as f32 / 32768.0);
                pos += step;
            }
            while q.len() > self.out_rate as usize {
                q.pop_front();
            }
        }
    }
}
