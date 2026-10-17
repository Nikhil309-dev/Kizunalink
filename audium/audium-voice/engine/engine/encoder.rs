// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use crate::opus::{Application, Bitrate, Channels, Encoder as OpusEncoder, SampleRate};

use crate::common::types::AnyResult;

/// Expected UDP loss on the Discord voice link, in percent.
///
/// libopus uses this to size the in-band FEC redundancy it emits and to bias
/// bit allocation towards robustness. Discord's own voice client reports a
/// similar figure, and the value only affects the encoder when `inband_fec` is
/// enabled.
pub const ASSUMED_PACKET_LOSS_PERCENT: u8 = 10;

pub struct Encoder {
    encoder: OpusEncoder,
}

impl Encoder {
    pub fn new() -> AnyResult<Self> {
        let encoder = OpusEncoder::new(SampleRate::Hz48000, Channels::Stereo, Application::Audio)
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
        encoder
            .set_bitrate(Bitrate::Auto)
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
        // Voice UDP is lossy and has no retransmission. In-band FEC embeds a
        // redundant copy of the previous frame so the receiver can conceal a
        // single lost packet instead of producing an audible gap. libopus
        // defaults both of these to 0 (off), which is wrong for this transport.
        encoder
            .set_packet_loss_perc(ASSUMED_PACKET_LOSS_PERCENT)
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
        encoder
            .set_inband_fec(true)
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
        Ok(Self { encoder })
    }

    pub fn encode(&mut self, input: &[i16], output: &mut [u8]) -> AnyResult<usize> {
        let size = self
            .encoder
            .encode(input, output)
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;
        Ok(size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encoder_new() {
        let result = Encoder::new();
        assert!(result.is_ok(), "Encoder creation should succeed");
    }

    #[test]
    fn voice_encoder_enables_fec_and_packet_loss_estimate() {
        let encoder = Encoder::new().unwrap();
        assert!(
            encoder.encoder.inband_fec().unwrap(),
            "in-band FEC must be on for a lossy UDP voice link"
        );
        assert_eq!(
            encoder.encoder.packet_loss_perc().unwrap(),
            ASSUMED_PACKET_LOSS_PERCENT,
            "encoder must be tuned for the assumed UDP loss rate"
        );
    }

    #[test]
    fn test_encoder_encode_silence() {
        let mut encoder = Encoder::new().unwrap();
        let input = vec![0i16; 960 * 2]; // 20ms stereo frame at 48kHz
        let mut output = vec![0u8; 4000];

        let result = encoder.encode(&input, &mut output);
        assert!(result.is_ok(), "Encoding silence should succeed");

        let size = result.unwrap();
        assert!(size > 0, "Encoded size should be greater than 0");
        assert!(
            size < output.len(),
            "Encoded size should be less than buffer"
        );
    }

    #[test]
    fn test_encoder_encode_tone() {
        let mut encoder = Encoder::new().unwrap();
        // Generate a simple 440Hz tone
        let mut input = vec![0i16; 960 * 2];
        for (i, sample) in input.iter_mut().enumerate() {
            let t = (i / 2) as f32 / 48000.0;
            *sample = (10000.0 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()) as i16;
        }
        let mut output = vec![0u8; 4000];

        let result = encoder.encode(&input, &mut output);
        assert!(result.is_ok(), "Encoding tone should succeed");

        let size = result.unwrap();
        assert!(size > 0, "Encoded size should be greater than 0");
    }

    #[test]
    fn test_encoder_encode_max_amplitude() {
        let mut encoder = Encoder::new().unwrap();
        let input = vec![i16::MAX; 960 * 2];
        let mut output = vec![0u8; 4000];

        let result = encoder.encode(&input, &mut output);
        assert!(result.is_ok(), "Encoding max amplitude should succeed");
        assert!(result.unwrap() > 0);
    }

    #[test]
    fn test_encoder_encode_min_amplitude() {
        let mut encoder = Encoder::new().unwrap();
        let input = vec![i16::MIN; 960 * 2];
        let mut output = vec![0u8; 4000];

        let result = encoder.encode(&input, &mut output);
        assert!(result.is_ok(), "Encoding min amplitude should succeed");
        assert!(result.unwrap() > 0);
    }

    #[test]
    fn test_encoder_encode_multiple_frames() {
        let mut encoder = Encoder::new().unwrap();
        let input = vec![1000i16; 960 * 2];
        let mut output = vec![0u8; 4000];

        // Encode multiple frames to ensure state is maintained
        for _ in 0..5 {
            let result = encoder.encode(&input, &mut output);
            assert!(result.is_ok(), "Multiple encodings should succeed");
            assert!(result.unwrap() > 0);
        }
    }

    #[test]
    fn test_encoder_output_varies_with_input() {
        let mut encoder1 = Encoder::new().unwrap();
        let mut output1 = vec![0u8; 4000];

        // Silence
        let silence = vec![0i16; 960 * 2];
        let size1 = encoder1.encode(&silence, &mut output1).unwrap();

        let mut encoder2 = Encoder::new().unwrap();
        let mut output2 = vec![0u8; 4000];

        // Tone
        let tone = vec![5000i16; 960 * 2];
        let size2 = encoder2.encode(&tone, &mut output2).unwrap();

        assert!(size1 > 0);
        assert!(size2 > 0);
        assert_ne!(
            &output1[..size1],
            &output2[..size2],
            "Encoded silence and tone payloads should differ"
        );
    }
}
