// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use std::collections::VecDeque;

use super::AudioFilter;

pub struct EchoFilter {
    echo_length: f32,
    decay: f32,

    buffer: VecDeque<i16>,
    delay_samples: usize,
}

impl EchoFilter {
    pub fn new(echo_length: f32, decay: f32) -> Self {
        let length = echo_length.clamp(0.001, 5.0);
        let decay = decay.clamp(0.0, 1.0);

        let frames = (48000.0 * length) as usize;
        let samples = frames * 2;

        let mut buffer = VecDeque::with_capacity(samples);
        buffer.extend(std::iter::repeat_n(0, samples));

        Self {
            echo_length: length,
            decay,
            buffer,
            delay_samples: samples,
        }
    }
}

impl AudioFilter for EchoFilter {
    fn process(&mut self, samples: &mut [i16]) {
        if self.echo_length <= 0.0 || self.decay <= 0.0 {
            return;
        }

        for sample in samples.iter_mut() {
            let delayed = self.buffer.pop_front().unwrap_or(0);

            let mixed = (*sample as f32) + (delayed as f32 * self.decay);
            let out = mixed.clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            *sample = out;

            self.buffer.push_back(out);
        }
    }

    fn is_enabled(&self) -> bool {
        self.echo_length > 0.0 && self.decay > 0.0
    }

    fn reset(&mut self) {
        self.buffer.clear();
        self.buffer
            .extend(std::iter::repeat_n(0, self.delay_samples));
    }
}
