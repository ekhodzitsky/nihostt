use crate::inference::{HOP_LENGTH, N_FFT, N_MELS};
use anyhow::Context;
use rustfft::{num_complex::Complex, FftPlanner};

/// Compute 80-bin log-mel spectrogram for 16kHz audio.
pub fn compute_mel(samples: &[f32], sample_rate: u32) -> anyhow::Result<Vec<Vec<f32>>> {
    assert_eq!(sample_rate, 16000, "expected 16kHz audio");

    let stft = compute_stft(samples, N_FFT, HOP_LENGTH)?;
    let mel_filterbank = mel_filterbank(N_MELS, N_FFT, sample_rate);

    let n_frames = stft.len();
    let mut mel_spec = vec![vec![0.0f32; N_MELS]; n_frames];

    for (t, frame) in stft.iter().enumerate() {
        for (m, mel_weights) in mel_filterbank.iter().enumerate() {
            let mut sum = 0.0f32;
            for (k, &weight) in mel_weights.iter().enumerate() {
                if k < frame.len() {
                    sum += frame[k] * weight;
                }
            }
            mel_spec[t][m] = sum.max(1e-10).ln();
        }
    }

    Ok(mel_spec)
}

fn compute_stft(
    samples: &[f32],
    n_fft: usize,
    hop_length: usize,
) -> anyhow::Result<Vec<Vec<f32>>> {
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(n_fft);

    let mut frames = Vec::new();
    let mut buffer = vec![Complex::new(0.0f32, 0.0f32); n_fft];

    for start in (0..samples.len()).step_by(hop_length) {
        if start + n_fft > samples.len() {
            break;
        }

        // Apply Hann window
        for (i, s) in samples[start..start + n_fft].iter().enumerate() {
            let window = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (n_fft as f32 - 1.0)).cos();
            buffer[i] = Complex::new(s * window, 0.0);
        }
        for i in samples[start..start + n_fft].len()..n_fft {
            buffer[i] = Complex::new(0.0, 0.0);
        }

        fft.process(&mut buffer);

        let magnitudes: Vec<f32> = buffer[..n_fft / 2 + 1]
            .iter()
            .map(|c| (c.re * c.re + c.im * c.im).sqrt())
            .collect();

        frames.push(magnitudes);
    }

    Ok(frames)
}

fn mel_filterbank(n_mels: usize, n_fft: usize, sample_rate: u32) -> Vec<Vec<f32>> {
    let f_min = 0.0f32;
    let f_max = sample_rate as f32 / 2.0;

    let mel_min = hz_to_mel(f_min);
    let mel_max = hz_to_mel(f_max);

    let mut mel_points = vec![0.0f32; n_mels + 2];
    for i in 0..n_mels + 2 {
        mel_points[i] = mel_min + (mel_max - mel_min) * i as f32 / (n_mels + 1) as f32;
    }

    let hz_points: Vec<f32> = mel_points.iter().map(|&m| mel_to_hz(m)).collect();
    let bin_points: Vec<f32> = hz_points
        .iter()
        .map(|&h| (n_fft as f32 + 1.0) * h / sample_rate as f32)
        .collect();

    let mut filterbank = vec![vec![0.0f32; n_fft / 2 + 1]; n_mels];

    for m in 0..n_mels {
        for k in 0..n_fft / 2 + 1 {
            let kf = k as f32;
            if kf >= bin_points[m] && kf < bin_points[m + 1] {
                filterbank[m][k] = (kf - bin_points[m]) / (bin_points[m + 1] - bin_points[m]);
            } else if kf >= bin_points[m + 1] && kf < bin_points[m + 2] {
                filterbank[m][k] = (bin_points[m + 2] - kf) / (bin_points[m + 2] - bin_points[m + 1]);
            }
        }
    }

    filterbank
}

fn hz_to_mel(hz: f32) -> f32 {
    2595.0 * (1.0 + hz / 700.0).log10()
}

fn mel_to_hz(mel: f32) -> f32 {
    700.0 * (10.0f32.powf(mel / 2595.0) - 1.0)
}
