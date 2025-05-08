use rustfft::{num_complex::Complex, FftPlanner};

// TODO: Add fields for frequency binning, peak frequency, etc. later
// Represents a single chunk of derived audio meta
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct AudioAnalysisData {
    pub rms_amplitude: f32,
    // TODO: Use `peak_amplitude`, `frequency_magnitudes`, `fft_size` later
    pub peak_amplitude: f32,
    // N/2 + 1 points
    pub frequency_magnitudes: Vec<f32>,
    pub fft_size: usize,
}

pub struct AudioProcessor {
    fft_planner: FftPlanner<f32>,
    fft_size: usize,
    // Hann window values
    // We use a hann window to help pre process audio into pretty packages before
    // running other logic on audio chunks via FFT
    window: Vec<f32>,
    fft_input_buffer: Vec<Complex<f32>>,
    fft_output_buffer: Vec<Complex<f32>>,
    // For incomplete chunks
    sample_buffer: Vec<f32>,
}

impl AudioProcessor {
    pub fn new(fft_size: usize) -> Self {
        if !fft_size.is_power_of_two() {
            tracing::warn!(
                "FFT size {} is not a power of two. This may impact performance.",
                fft_size
            );
        }

        let window = hann_window(fft_size);
        // Pre-plan the FFT and turn it into an Arc<dyn Fft<f32>> for reuse
        let mut planner = FftPlanner::<f32>::new();
        planner.plan_fft_forward(fft_size);

        AudioProcessor {
            fft_planner: planner,
            fft_size,
            window,
            fft_input_buffer: vec![Complex::new(0.0, 0.0); fft_size],
            fft_output_buffer: vec![Complex::new(0.0, 0.0); fft_size],
            // Capacity for buffering
            sample_buffer: Vec::with_capacity(fft_size * 2),
        }
    }

    // Processes incoming raw audio samples (mono assumed for now).
    // Buffers samples until a full FFT window is available.
    // Returns analysis data if a full FFT window was processed.
    pub fn process_samples(&mut self, new_samples: &[f32]) -> Option<AudioAnalysisData> {
        self.sample_buffer.extend_from_slice(new_samples);

        if self.sample_buffer.len() >= self.fft_size {
            // We have enough samples for at least one FFT window
            let mut peak_amplitude = 0.0f32;
            let mut rms_sum_sq = 0.0f32;

            // Prepare FFT input buffer, apply the window, then calculate amplitude metrics
            for i in 0..self.fft_size {
                let sample = self.sample_buffer[i];
                let windowed_sample = sample * self.window[i];
                self.fft_input_buffer[i] = Complex::new(windowed_sample, 0.0);

                let abs_sample = sample.abs();
                if abs_sample > peak_amplitude {
                    peak_amplitude = abs_sample;
                }
                rms_sum_sq += sample * sample;
            }

            let rms_amplitude = (rms_sum_sq / self.fft_size as f32).sqrt();

            // Perform FFT
            let fft = self.fft_planner.plan_fft_forward(self.fft_size);
            // Use `process_with_scratch` if input/output buffers are separate & sized correctly
            // If fft_input_buffer can be modified in place, use fft.process(&mut self.fft_input_buffer);
            fft.process_with_scratch(&mut self.fft_input_buffer, &mut self.fft_output_buffer);

            // Calculate frequency magnitudes (power spectrum)
            let num_freq_bins = self.fft_size / 2 + 1;
            let frequency_magnitudes: Vec<f32> = self
                .fft_output_buffer
                .iter()
                .take(num_freq_bins)
                .map(|c| c.norm() / self.fft_size as f32) // Calculate magnitude and normalize by N
                .collect();

            // Remove processed samples from the buffer
            // drain is efficient for removing from the beginning
            self.sample_buffer.drain(0..self.fft_size);

            Some(AudioAnalysisData {
                rms_amplitude,
                peak_amplitude,
                frequency_magnitudes,
                fft_size: self.fft_size,
            })
        } else {
            // Not enough samples yet
            None
        }
    }
}

// Helper function for Hann window
fn hann_window(size: usize) -> Vec<f32> {
    if size == 0 {
        return vec![];
    }
    let norm_factor = (size as f32 - 1.0).max(1.0); // Dividing by zero is bad, mmkay

    (0..size)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / norm_factor).cos()))
        .collect()
}
