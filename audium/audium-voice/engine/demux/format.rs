// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use crate::common::types::AudioFormat;

/// Sniff the container format from the first bytes of arbitrary data.
///
/// Requires at least 4 bytes. Returns `AudioFormat::Unknown` for anything
pub fn detect_format(header: &[u8]) -> AudioFormat {
    if header.len() < 4 {
        return AudioFormat::Unknown;
    }

    if header.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return AudioFormat::Webm;
    }

    if header.len() >= 8 && &header[4..8] == b"ftyp" {
        return AudioFormat::Mp4;
    }

    if header.starts_with(b"OggS") {
        return AudioFormat::Ogg;
    }

    if header.starts_with(b"fLaC") {
        return AudioFormat::Flac;
    }

    if header.starts_with(b"RIFF") && header.len() >= 12 && &header[8..12] == b"WAVE" {
        return AudioFormat::Wav;
    }

    if header.starts_with(b"ID3") {
        return AudioFormat::Mp3;
    }
    if header[0] == 0xFF && (header[1] & 0xE0) == 0xE0 {
        return AudioFormat::Mp3;
    }

    if is_tracker_magic(header) {
        return AudioFormat::Tracker;
    }

    AudioFormat::Unknown
}

/// True when `header` begins a tracker module Audium can replay.
///
/// Only the formats the bundled replayer imports are recognised — MOD, XM, S3M
/// and IT. Advertising a signature we cannot decode would turn a load into a
/// runtime failure, so related-but-unsupported magics are deliberately omitted.
///
/// `header` should hold the first ~1084 bytes so the MOD signature at offset
/// 1080 is visible; shorter buffers still match the leading-magic formats.
pub fn is_tracker_magic(header: &[u8]) -> bool {
    if header.len() < 4 {
        return false;
    }

    if header.starts_with(b"Extended Module: ") {
        return true;
    }

    if header.starts_with(b"IMPM") {
        return true;
    }

    if header.len() >= 48 && &header[44..48] == b"SCRM" {
        return true;
    }

    if header.len() >= 1084 {
        let sig = &header[1080..1084];

        const SIGS: [&[u8; 4]; 12] = [
            b"M.K.", b"M!K!", b"M&K!", b"N.T.", b"FLT4", b"FLT8", b"4CHN", b"6CHN", b"8CHN",
            b"OCTA", b"OKTA", b"CD81",
        ];
        if SIGS.iter().any(|s| s.as_slice() == sig) {
            return true;
        }

        if sig[2] == b'C' && sig[3] == b'H' && sig[0].is_ascii_digit() && sig[1].is_ascii_digit() {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_webm() {
        let hdr = [0x1A, 0x45, 0xDF, 0xA3, 0x00, 0x00, 0x00, 0x00];
        assert_eq!(detect_format(&hdr), AudioFormat::Webm);
    }

    #[test]
    fn detect_mp4() {
        let hdr = b"\x00\x00\x00\x1Cftypisom";
        assert_eq!(detect_format(hdr), AudioFormat::Mp4);
    }

    #[test]
    fn detect_ogg() {
        assert_eq!(detect_format(b"OggS\x00"), AudioFormat::Ogg);
    }

    #[test]
    fn detect_unknown() {
        assert_eq!(
            detect_format(&[0x00, 0x00, 0x00, 0x00]),
            AudioFormat::Unknown
        );
    }

    #[test]
    fn detect_xm_module() {
        let mut hdr = b"Extended Module: ".to_vec();
        hdr.extend(vec![0u8; 60]);
        assert_eq!(detect_format(&hdr), AudioFormat::Tracker);
    }

    #[test]
    fn detect_it_module() {
        let mut hdr = b"IMPM".to_vec();
        hdr.extend(vec![0u8; 60]);
        assert_eq!(detect_format(&hdr), AudioFormat::Tracker);
    }

    #[test]
    fn detect_s3m_module() {
        let mut hdr = vec![0u8; 44];
        hdr.extend(b"SCRM");
        hdr.extend(vec![0u8; 16]);
        assert_eq!(detect_format(&hdr), AudioFormat::Tracker);
    }

    #[test]
    fn detect_mod_module() {
        let mut hdr = vec![0u8; 1080];
        hdr.extend(b"M.K.");
        hdr.extend(vec![0u8; 16]);
        assert_eq!(detect_format(&hdr), AudioFormat::Tracker);
    }

    #[test]
    fn mod_magic_needs_the_signature_offset() {
        assert!(!is_tracker_magic(&[0u8; 512]));

        assert!(!is_tracker_magic(b"RIFFxxxxWAVE"));
    }
}
