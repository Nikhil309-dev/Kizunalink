// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

pub const TARGET_SAMPLE_RATE: u32 = 48_000;
pub const SAMPLE_RATE_F64: f64 = 48_000.0;
pub const FRAME_SIZE_SAMPLES: usize = 960 * 2;
pub const MIXER_CHANNELS: usize = 2;
pub const OPUS_SAMPLE_RATE: u64 = 48_000;

// Q13: Compile-time assertion — if you change TARGET_SAMPLE_RATE, these modules break:
// - EQ biquad coefficients (hardcoded 48 kHz)
// - Sinc resampler (assumes 48 kHz)
// - Opus encoder (960 samples = 20ms at 48 kHz)
const _: () = assert!(
    TARGET_SAMPLE_RATE == 48_000,
    "TARGET_SAMPLE_RATE must be 48000"
);

// ── i16 PCM clip boundaries ──────────────────────────────────────────────────

pub const INT16_MAX_F: f32 = 32_767.0;
pub const INT16_MIN_F: f32 = -32_768.0;
pub const INT16_MAX_F64: f64 = 32_768.0;
pub const INV_INT16: f64 = 1.0 / INT16_MAX_F64;

// ── Codec ────────────────────────────────────────────────────────────────────

pub const MAX_OPUS_FRAME_SIZE: usize = 5_760;

// ── Buffer pool (byte pool) ──────────────────────────────────────────────────

pub const MAX_POOL_BYTES: usize = 2 * 1_024 * 1_024;
pub const MAX_BUCKET_ENTRIES: usize = 8;
pub const POOL_IDLE_CLEAR_SECS: u64 = 180;

// ── Audio mixer layers ───────────────────────────────────────────────────────

pub const MAX_LAYERS: usize = 5;
pub const LAYER_BUFFER_SIZE: usize = 1_024 * 1_024;

// ── Segmented remote reader ──────────────────────────────────────────────────

/// Stack reservation for a per-track decode/prefetch thread.
///
/// Each playing track runs one decoder thread (and the HTTP path a second
/// prefetch thread). Decoders are iterative and use only a few KiB of stack, so
/// Rust's default 2 MiB per-thread reservation is almost entirely unused. At
/// 1 000 concurrent tracks that is ~2 000 threads and ~4 GiB of needless
/// address-space reservation. Measured with `tools/maps_hist.py`: 50 players
/// reserve 124 × 2 MiB anonymous mappings before this change and 101 × 512 KiB
/// after. 512 KiB is comfortably above the deepest decoder frame.
pub const DECODE_THREAD_STACK_SIZE: usize = 512 * 1_024;

pub const CHUNK_SIZE: usize = 256 * 1_024;
pub const PREFETCH_CHUNKS: usize = 4;
pub const MAX_CONCURRENT_FETCHES: usize = 4;
pub const HTTP_CLIENT_TIMEOUT_SECS: u64 = 15;
pub const MAX_FETCH_RETRIES: u32 = 5;
pub const WORKER_IDLE_MS: u64 = 50;
pub const FETCH_WAIT_MS: u64 = 250;
pub const PROBE_TIMEOUT_SECS: u64 = 10;

// ── HttpSource ───────────────────────────────────────────────────────────────

pub const HTTP_PREFETCH_BUFFER_SIZE: usize = 2 * 1_024 * 1_024;

/// Default ceiling on the per-track HTTP prefetch buffer.
///
/// This buffer is allocated **per playing track**, so it is the dominant term
/// in steady-state memory when many guilds play at once. It counts *compressed*
/// bytes (the raw HTTP body the prefetcher holds in `SharedState::chunks`), not
/// decoded PCM: at 512 KiB and a typical 128 kbit/s stream that is ~30 seconds
/// of audio of readahead, which is far more than a decoder needs to absorb
/// network jitter.
///
/// Measured with `tools/mem_steady.sh` at 100 live players: steady RSS is
/// 482 MB at 2 MiB, 300 MB at 512 KiB and 298 MB at 256 KiB — i.e. the curve
/// saturates at 512 KiB and paying for 2 MiB buys nothing. Live single-player
/// A/B at 1 MiB held 31.6 MB with zero audio defects.
///
/// Operators can raise it for unusually slow origins via the
/// `AUDIUM_HTTP_BUF_BYTES` environment variable (see [`max_http_buf_bytes`]).
pub const MAX_HTTP_BUF_BYTES: usize = 512 * 1_024;

/// Lower bound accepted from `AUDIUM_HTTP_BUF_BYTES`. Below this the decoder
/// can starve on ordinary network jitter.
pub const MIN_HTTP_BUF_BYTES: usize = 256 * 1_024;
/// Upper bound accepted from `AUDIUM_HTTP_BUF_BYTES`, so a typo cannot
/// re-introduce an unbounded buffer.
pub const MAX_HTTP_BUF_BYTES_LIMIT: usize = 64 * 1_024 * 1_024;

/// Effective per-track prefetch ceiling: [`MAX_HTTP_BUF_BYTES`] unless
/// `AUDIUM_HTTP_BUF_BYTES` overrides it (clamped to
/// [`MIN_HTTP_BUF_BYTES`]..[`MAX_HTTP_BUF_BYTES_LIMIT`]). Read once.
pub fn max_http_buf_bytes() -> usize {
    use std::sync::OnceLock;
    static V: OnceLock<usize> = OnceLock::new();
    *V.get_or_init(|| {
        resolve_http_buf_bytes(std::env::var("AUDIUM_HTTP_BUF_BYTES").ok().as_deref())
    })
}

/// Parse and clamp an `AUDIUM_HTTP_BUF_BYTES` value. Split out from
/// [`max_http_buf_bytes`] so the bounds are unit-testable without the
/// process-wide cache. `None` or an unparsable value falls back to the default.
fn resolve_http_buf_bytes(raw: Option<&str>) -> usize {
    raw.and_then(|s| s.trim().parse::<usize>().ok())
        .map(|n| n.clamp(MIN_HTTP_BUF_BYTES, MAX_HTTP_BUF_BYTES_LIMIT))
        .unwrap_or(MAX_HTTP_BUF_BYTES)
}

/// Upper bound on a tracker module read fully into memory for playback. Real
/// MOD/XM/S3M/IT files are a few MiB at most; the cap only stops a hostile or
/// mislabeled URL from exhausting memory.
pub const MAX_MODULE_BYTES: usize = 32 * 1_024 * 1_024;
pub const HTTP_INITIAL_BUF_CAPACITY: usize = 256 * 1_024;
pub const HTTP_SOCKET_SKIP_LIMIT: u64 = 1_000_000;
pub const HTTP_FETCH_CHUNK_LIMIT: u64 = 2 * 1_024 * 1_024;

// ── Effects ──────────────────────────────────────────────────────────────────

pub const HALF_PI: f32 = std::f32::consts::PI / 2.0;

// ── Route Planner ────────────────────────────────────────────────────────────

pub const ROUTE_PLANNER_FAIL_EXPIRE_MS: u64 = 7 * 24 * 60 * 60 * 1_000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_buf_bytes_defaults_when_unset_or_invalid() {
        assert_eq!(resolve_http_buf_bytes(None), MAX_HTTP_BUF_BYTES);
        assert_eq!(resolve_http_buf_bytes(Some("nonsense")), MAX_HTTP_BUF_BYTES);
        assert_eq!(resolve_http_buf_bytes(Some("")), MAX_HTTP_BUF_BYTES);
    }

    #[test]
    fn http_buf_bytes_is_clamped_to_the_safe_range() {
        assert_eq!(resolve_http_buf_bytes(Some("0")), MIN_HTTP_BUF_BYTES);
        assert_eq!(resolve_http_buf_bytes(Some("1024")), MIN_HTTP_BUF_BYTES);
        assert_eq!(
            resolve_http_buf_bytes(Some(&(MAX_HTTP_BUF_BYTES_LIMIT * 4).to_string())),
            MAX_HTTP_BUF_BYTES_LIMIT
        );
        assert_eq!(resolve_http_buf_bytes(Some("1048576")), 1_048_576);
    }
}
