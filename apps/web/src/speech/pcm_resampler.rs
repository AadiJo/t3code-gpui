//! Downsampling to the 16 kHz the server reads (`pcmResampler.ts`): a 63-tap Hamming-windowed
//! low-pass at 7.2 kHz, with filter history and sample phase kept across capture blocks.

use t3code_client_runtime::voice_input::speech::SPEECH_SAMPLE_RATE;

/// One capture's resampler.
pub struct PcmResampler {
    coefficients: Vec<f64>,
    history: Vec<f32>,
    position: usize,
    phase: f64,
    ratio: f64,
}

impl PcmResampler {
    /// `None` below 16 kHz, which the server cannot use.
    pub fn new(sample_rate: u32) -> Option<Self> {
        if sample_rate < SPEECH_SAMPLE_RATE {
            return None;
        }
        let taps = if sample_rate == SPEECH_SAMPLE_RATE { 1 } else { 63 };
        let middle = (taps - 1) as f64 / 2.;
        let cutoff = 7_200. / sample_rate as f64;
        let mut coefficients: Vec<f64> = (0..taps)
            .map(|i| {
                let x = i as f64 - middle;
                let sinc = if x == 0. { 2. * cutoff } else { (2. * std::f64::consts::PI * cutoff * x).sin() / (std::f64::consts::PI * x) };
                let window = if middle == 0. { 1. } else { 0.54 - 0.46 * (std::f64::consts::PI * i as f64 / middle).cos() };
                sinc * window
            })
            .collect();
        let sum: f64 = coefficients.iter().sum();
        coefficients.iter_mut().for_each(|c| *c /= sum);
        Some(Self { history: vec![0.; taps], coefficients, position: 0, phase: 0., ratio: sample_rate as f64 / SPEECH_SAMPLE_RATE as f64 })
    }

    /// Filters `samples` and appends the 16 kHz output to `output`.
    pub fn push(&mut self, samples: &[f32], output: &mut Vec<f32>) {
        let len = self.history.len();
        for &sample in samples {
            self.history[self.position] = sample;
            self.phase += 1.;
            if self.phase >= self.ratio {
                self.phase -= self.ratio;
                let mut value = 0.;
                for (tap, coefficient) in self.coefficients.iter().enumerate() {
                    value += self.history[(self.position + len - tap) % len] as f64 * coefficient;
                }
                output.push(value.clamp(-1., 1.) as f32);
            }
            self.position = (self.position + 1) % len;
        }
    }

    /// The filter's tail: zeros through the history.
    pub fn flush(&mut self, output: &mut Vec<f32>) {
        if self.coefficients.len() > 1 {
            let zeros = vec![0.; self.history.len() - 1];
            self.push(&zeros, output);
        }
    }
}
