//! Small DSP helpers. Whisper wants mono f32 samples at 16 kHz.

pub const SAMPLE_RATE: u32 = 16_000;

/// Root mean square level of a block of samples.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f32 = samples.iter().map(|s| s * s).sum();
    (sum / samples.len() as f32).sqrt()
}

/// Averages interleaved frames down to one channel.
pub fn interleaved_to_mono(data: &[f32], channels: usize) -> Vec<f32> {
    if channels <= 1 {
        return data.to_vec();
    }
    data.chunks_exact(channels)
        .map(|frame| frame.iter().sum::<f32>() / channels as f32)
        .collect()
}

/// Second-order low-pass filter (RBJ cookbook), used before downsampling so
/// content above 8 kHz does not fold back into the speech band.
struct LowPass {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl LowPass {
    fn new(sample_rate: f32, cutoff: f32) -> Self {
        let w0 = 2.0 * std::f32::consts::PI * cutoff / sample_rate;
        let alpha = w0.sin() / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let cos = w0.cos();
        let a0 = 1.0 + alpha;
        Self {
            b0: (1.0 - cos) / 2.0 / a0,
            b1: (1.0 - cos) / a0,
            b2: (1.0 - cos) / 2.0 / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha) / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// Streaming resampler to 16 kHz: low-pass filter, then linear interpolation.
///
/// Keeps its state between calls, so audio delivered in arbitrary block
/// sizes resamples the same as if it arrived in one piece.
pub struct Resampler {
    filters: Option<[LowPass; 2]>,
    step: f64,
    /// Position of the next output sample, relative to the start of `carry`.
    pos: f64,
    /// Last input sample of the previous block, needed to interpolate across blocks.
    carry: Option<f32>,
}

impl Resampler {
    pub fn new(input_rate: u32) -> Self {
        let filters = (input_rate > SAMPLE_RATE).then(|| {
            let cutoff = SAMPLE_RATE as f32 * 0.45;
            [
                LowPass::new(input_rate as f32, cutoff),
                LowPass::new(input_rate as f32, cutoff),
            ]
        });
        Self {
            filters,
            step: input_rate as f64 / SAMPLE_RATE as f64,
            pos: 0.0,
            carry: None,
        }
    }

    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if input.is_empty() {
            return Vec::new();
        }
        if (self.step - 1.0).abs() < f64::EPSILON {
            return input.to_vec();
        }
        let filtered: Vec<f32>;
        let input = match &mut self.filters {
            Some([a, b]) => {
                filtered = input.iter().map(|&x| b.process(a.process(x))).collect();
                &filtered[..]
            }
            None => input,
        };
        // Virtual buffer: [carry?] + input
        let offset = usize::from(self.carry.is_some());
        let len = input.len() + offset;
        let at = |i: usize| -> f32 {
            if i < offset {
                self.carry.unwrap_or(0.0)
            } else {
                input[i - offset]
            }
        };
        let mut out = Vec::with_capacity((input.len() as f64 / self.step) as usize + 1);
        while self.pos + 1.0 < len as f64 {
            let i = self.pos.floor() as usize;
            let frac = (self.pos - i as f64) as f32;
            out.push(at(i) * (1.0 - frac) + at(i + 1) * frac);
            self.pos += self.step;
        }
        // Re-base so the last input sample becomes index 0 of the next call.
        self.pos -= (len - 1) as f64;
        self.carry = Some(input[input.len() - 1]);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_of_constant_signal() {
        assert_eq!(rms(&[0.5, -0.5, 0.5, -0.5]), 0.5);
        assert_eq!(rms(&[]), 0.0);
    }

    #[test]
    fn stereo_to_mono_averages() {
        assert_eq!(interleaved_to_mono(&[1.0, 0.0, 0.5, 0.5], 2), vec![0.5, 0.5]);
        assert_eq!(interleaved_to_mono(&[1.0, 2.0], 1), vec![1.0, 2.0]);
    }

    #[test]
    fn resampler_output_length_matches_ratio() {
        for rate in [44_100u32, 48_000, 24_000] {
            let mut r = Resampler::new(rate);
            let out = r.process(&vec![0.0; rate as usize]);
            let expected = SAMPLE_RATE as i64;
            assert!((out.len() as i64 - expected).abs() <= 1, "{rate}: {}", out.len());
        }
    }

    #[test]
    fn resampler_is_independent_of_block_size() {
        let input: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.01).sin()).collect();
        let whole = Resampler::new(48_000).process(&input);
        let mut r = Resampler::new(48_000);
        let mut pieces = Vec::new();
        for block in input.chunks(517) {
            pieces.extend(r.process(block));
        }
        assert_eq!(whole.len(), pieces.len());
        for (a, b) in whole.iter().zip(&pieces) {
            assert!((a - b).abs() < 1e-5);
        }
    }

    #[test]
    fn resampler_preserves_a_low_tone() {
        // A 440 Hz tone keeps its zero-crossing rate after 48k -> 16k.
        let input: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin())
            .collect();
        let out = Resampler::new(48_000).process(&input);
        let crossings = out.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        assert!((438..=442).contains(&crossings), "{crossings}");
    }

    #[test]
    fn resampler_removes_content_above_8k() {
        // A 12 kHz tone would alias to 4 kHz without the low-pass filter.
        let input: Vec<f32> = (0..48_000)
            .map(|i| (2.0 * std::f32::consts::PI * 12_000.0 * i as f32 / 48_000.0).sin())
            .collect();
        let out = Resampler::new(48_000).process(&input);
        assert!(rms(&out[1_000..]) < 0.05, "{}", rms(&out[1_000..]));
    }

    #[test]
    fn passthrough_at_16k() {
        let mut r = Resampler::new(16_000);
        assert_eq!(r.process(&[0.1, 0.2]), vec![0.1, 0.2]);
    }
}
