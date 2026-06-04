use std::sync::{Arc, RwLock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use rustfft::{FftPlanner, num_complex::Complex32};

use crate::audio::buffer::AudioConsumer;

pub const FFT_SIZE: usize = 2048;
pub const SPECTRUM_BINS: usize = 96;
pub const WAVEFORM_POINTS: usize = 160;

#[derive(Clone, Debug)]
pub struct VisualState {
    pub waveform: Vec<f32>,
    pub spectrum: Vec<f32>,
    pub level: f32,
}

impl VisualState {
    pub fn silence() -> Self {
        Self {
            waveform: vec![0.0; WAVEFORM_POINTS],
            spectrum: vec![0.0; SPECTRUM_BINS],
            level: 0.0,
        }
    }
}

pub fn spawn_analyzer(
    consumer: AudioConsumer,
    state: Arc<RwLock<VisualState>>,
    running: Arc<AtomicBool>,
) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let mut planner = FftPlanner::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let mut input = vec![0.0_f32; FFT_SIZE];
        let mut scratch = vec![0.0_f32; FFT_SIZE];
        let mut complex = vec![Complex32::ZERO; FFT_SIZE];
        let mut spectrum = vec![0.0_f32; SPECTRUM_BINS];
        let mut waveform = vec![0.0_f32; WAVEFORM_POINTS];

        while running.load(Ordering::Acquire) {
            let count = consumer.pop_into(&mut scratch);
            if count == 0 {
                thread::sleep(Duration::from_millis(4));
                continue;
            }

            shift_append(&mut input, &scratch[..count]);
            analyze_frame(&input, &mut complex, &mut spectrum, &mut waveform, &fft);

            let rms = (input.iter().map(|sample| sample * sample).sum::<f32>() / input.len() as f32)
                .sqrt()
                .min(1.0);

            if let Ok(mut guard) = state.write() {
                smooth_into(&mut guard.spectrum, &spectrum, 0.35);
                smooth_into(&mut guard.waveform, &waveform, 0.55);
                guard.level = guard.level.mul_add(0.75, rms * 0.25);
            }
        }
    })
}

fn shift_append(buffer: &mut [f32], incoming: &[f32]) {
    let incoming = if incoming.len() > buffer.len() {
        &incoming[incoming.len() - buffer.len()..]
    } else {
        incoming
    };
    buffer.rotate_left(incoming.len());
    let start = buffer.len() - incoming.len();
    buffer[start..].copy_from_slice(incoming);
}

fn analyze_frame(
    input: &[f32],
    complex: &mut [Complex32],
    spectrum: &mut [f32],
    waveform: &mut [f32],
    fft: &Arc<dyn rustfft::Fft<f32>>,
) {
    let len = input.len();
    for (index, slot) in complex.iter_mut().enumerate() {
        let window = 0.5 - 0.5 * ((2.0 * std::f32::consts::PI * index as f32) / len as f32).cos();
        *slot = Complex32::new(input[index] * window, 0.0);
    }

    fft.process(complex);

    let usable_bins = len / 2;
    let spectrum_len = spectrum.len();
    let mut bands = vec![0.0_f32; spectrum_len];
    for (index, slot) in spectrum.iter_mut().enumerate() {
        let start = log_band_index(index, spectrum_len, usable_bins);
        let end = log_band_index(index + 1, spectrum_len, usable_bins).max(start + 1);
        let magnitude = complex[start..end]
            .iter()
            .map(|sample| sample.norm())
            .sum::<f32>()
            / (end - start) as f32;

        *slot = ((magnitude.log10().max(-3.0) + 3.0) / 3.0).clamp(0.0, 1.0);
    }

    shape_spectrum(spectrum, &mut bands);
    spectrum.copy_from_slice(&bands);

    let waveform_len = waveform.len();
    for (index, slot) in waveform.iter_mut().enumerate() {
        let source_index = index * input.len() / waveform_len;
        *slot = input[source_index].clamp(-1.0, 1.0);
    }
}

fn log_band_index(index: usize, band_count: usize, usable_bins: usize) -> usize {
    let min_bin = 1.0_f32;
    let max_bin = (usable_bins.saturating_sub(1)).max(1) as f32;
    let position = index as f32 / band_count.max(1) as f32;

    (min_bin * (max_bin / min_bin).powf(position))
        .round()
        .clamp(min_bin, max_bin) as usize
}

fn shape_spectrum(spectrum: &[f32], shaped: &mut [f32]) {
    const KERNEL: &[(isize, f32)] = &[
        (-3, 0.03),
        (-2, 0.08),
        (-1, 0.18),
        (0, 0.42),
        (1, 0.18),
        (2, 0.08),
        (3, 0.03),
    ];

    for index in 0..spectrum.len() {
        let mut total = 0.0;
        let mut weight = 0.0;

        for &(offset, amount) in KERNEL {
            let neighbor = (index as isize + offset)
                .clamp(0, spectrum.len().saturating_sub(1) as isize)
                as usize;
            total += spectrum[neighbor] * amount;
            weight += amount;
        }

        shaped[index] = (total / weight).powf(0.78).clamp(0.0, 1.0);
    }

    for index in 1..spectrum.len().saturating_sub(1) {
        let left = shaped[index - 1];
        let center = shaped[index];
        let right = shaped[index + 1];
        let valley = ((left + right) * 0.5 - center).max(0.0);
        shaped[index] = (center + valley * 0.34).clamp(0.0, 1.0);
    }

    let previous = shaped.to_vec();
    for index in 0..previous.len() {
        let left_peak = previous[index.saturating_sub(5)..=index]
            .iter()
            .copied()
            .fold(0.0_f32, f32::max);
        let right_peak = previous[index..(index + 6).min(previous.len())]
            .iter()
            .copied()
            .fold(0.0_f32, f32::max);
        let bridge = left_peak.min(right_peak) * 0.7;
        shaped[index] = previous[index].max(previous[index] * 0.78 + bridge * 0.22);
    }
}

fn smooth_into(current: &mut [f32], next: &[f32], factor: f32) {
    for (current, next) in current.iter_mut().zip(next) {
        *current = current.mul_add(1.0 - factor, next * factor);
    }
}
