// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

pub struct DelayLine {
    buffer: Vec<f32>,
    size: usize,
    write_index: usize,
}

impl DelayLine {
    pub fn new(size: usize) -> Self {
        let size = size.max(1);
        Self {
            buffer: vec![0.0; size],
            size,
            write_index: 0,
        }
    }

    pub fn write(&mut self, sample: f32) {
        self.buffer[self.write_index] = sample;
        self.write_index = (self.write_index + 1) % self.size;
    }

    pub fn read(&self, delay_in_samples: f32) -> f32 {
        let safe_delay = delay_in_samples.max(0.0).min((self.size - 1) as f32);
        let int_delay = safe_delay as usize;
        let frac = safe_delay - int_delay as f32;

        let idx0 = (self.write_index + self.size - int_delay) % self.size;
        let idx1 = (self.write_index + self.size - int_delay - 1) % self.size;

        let s0 = self.buffer[idx0];
        let s1 = self.buffer[idx1];
        s0 * (1.0 - frac) + s1 * frac
    }

    pub fn clear(&mut self) {
        self.buffer.fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::DelayLine;

    #[test]
    fn zero_sized_delay_line_does_not_divide_by_zero() {
        let mut line = DelayLine::new(0);
        line.write(1.0);
        assert_eq!(line.read(0.0), 1.0);
    }

    #[test]
    fn fractional_delay_interpolates_between_taps() {
        let mut line = DelayLine::new(4);
        line.write(0.0);
        line.write(1.0);

        assert_eq!(line.read(1.0), 1.0);
        assert_eq!(line.read(0.5), 0.5);
    }
}
