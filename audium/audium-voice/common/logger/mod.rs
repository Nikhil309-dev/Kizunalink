// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use std::{fs, path::Path, sync::OnceLock};

use tracing_subscriber::{
    EnvFilter, Layer, filter::filter_fn, fmt, layer::SubscriberExt, util::SubscriberInitExt,
};

use crate::{common::utils::strip_ansi_escapes, config::LoggingConfig};

pub mod formatter;
pub mod writer;

pub use formatter::CustomFormatter;
pub use writer::CircularFileWriter;

pub(crate) static GLOBAL_FILE_WRITER: OnceLock<CircularFileWriter> = OnceLock::new();

#[allow(clippy::print_stdout)]
#[macro_export]
macro_rules! log_print {
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        std::print!("{}", msg);
        $crate::common::logger::append_to_file_raw(&msg);
    }};
}

#[allow(clippy::print_stdout)]
#[macro_export]
macro_rules! log_println {
    () => {{
        std::println!();
        $crate::common::logger::append_to_file_raw("\n");
    }};
    ($($arg:tt)*) => {{
        let msg = format!($($arg)*);
        std::println!("{}", msg);
        $crate::common::logger::append_to_file_raw(&format!("{}\n", msg));
    }};
}

/// Appends a raw message to the global log file, stripping ANSI escapes.
pub fn append_to_file_raw(msg: &str) {
    if let Some(mut writer) = GLOBAL_FILE_WRITER.get().cloned() {
        use std::io::Write;
        let clean_msg = strip_ansi_escapes(msg);
        let _ = writer.write_all(clean_msg.as_bytes());
    }
}

/// Initializes the global logger with the provided configuration.
///
/// The console is **always** enabled so Pterodactyl (and any process
/// supervisor) captures logs automatically:
///
/// * `TRACE`/`DEBUG`/`INFO` are written to **stdout**.
/// * `WARN`/`ERROR` are written to **stderr**.
///
/// The two console layers carry complementary, non-overlapping level filters,
/// so each event is emitted exactly once — no duplicate lines.
///
/// File logging is optional: it mirrors both levels to a rotating file only
/// when `[logging.file]` is present with `enabled = true` (the default for the
/// section). Console output never depends on it.
pub fn init(config: &LoggingConfig) {
    let log_level = config.level.as_deref().unwrap_or("info");

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(filter_string(log_level, config.filters.as_deref())));

    let ansi = crate::common::utils::colors_enabled();

    let stdout_layer = fmt::layer()
        .event_format(CustomFormatter::new(true))
        .with_ansi(ansi)
        .with_filter(filter_fn(|meta| console_target(meta.level()).stdout));

    let stderr_layer = fmt::layer()
        .with_writer(std::io::stderr)
        .event_format(CustomFormatter::new(true))
        .with_ansi(ansi)
        .with_filter(filter_fn(|meta| console_target(meta.level()).stderr));

    let file_config = config.file.as_ref().filter(|f| f.enabled);
    let file_layer = file_config.map(|file_config| {
        if let Some(parent) = Path::new(&file_config.path).parent()
            && !parent.as_os_str().is_empty()
        {
            let _ = fs::create_dir_all(parent);
        }

        let writer = CircularFileWriter::new(
            file_config.path.clone(),
            file_config.max_lines,
            file_config.max_files,
            file_config.rotate_daily,
        );
        let _ = GLOBAL_FILE_WRITER.set(writer.clone());

        fmt::layer()
            .with_writer(writer)
            .event_format(CustomFormatter::new(false))
            .with_ansi(false)
    });

    match tracing_subscriber::registry()
        .with(env_filter)
        .with(stdout_layer)
        .with(stderr_layer)
        .with(file_layer)
        .try_init()
    {
        Ok(()) => {
            let sink = match file_config {
                Some(f) => format!("console + file ({})", f.path),
                None => "console only".to_string(),
            };
            tracing::info!("logging initialized: level={log_level}, sink={sink}");
        }
        Err(e) => {
            use std::io::Write as _;
            let _ = writeln!(
                std::io::stderr(),
                "[audium] warning: failed to install tracing subscriber: {e}"
            );
        }
    }
}

/// Builds the `EnvFilter` directive string from the configured level and the
/// optional per-target filter overrides (e.g. `audium=debug,davey=off`).
fn filter_string(level: &str, filters: Option<&str>) -> String {
    match filters {
        Some(f) if !f.is_empty() => format!("{level},{f}"),
        _ => level.to_string(),
    }
}

/// Which console streams an event at `level` is written to.
///
/// `tracing::Level` orders by severity with ERROR as the *smallest*
/// (`ERROR < WARN < INFO < DEBUG < TRACE`), so anything more verbose than WARN
/// is stdout and WARN/ERROR is stderr. The two fields are mutually exclusive,
/// guaranteeing each event is emitted exactly once (no duplicate lines).
#[derive(Debug, PartialEq, Eq)]
struct ConsoleTarget {
    stdout: bool,
    stderr: bool,
}

fn console_target(level: &tracing::Level) -> ConsoleTarget {
    let to_stderr = *level <= tracing::Level::WARN;
    ConsoleTarget {
        stdout: !to_stderr,
        stderr: to_stderr,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_string_uses_level_only_without_filters() {
        assert_eq!(filter_string("info", None), "info");
        assert_eq!(filter_string("debug", Some("")), "debug");
    }

    #[test]
    fn filter_string_appends_target_overrides() {
        assert_eq!(
            filter_string("info", Some("audium=debug,davey=off")),
            "info,audium=debug,davey=off"
        );
    }

    #[test]
    fn filter_string_accepts_valid_levels() {
        for level in ["trace", "debug", "info", "warn", "error", "off"] {
            assert_eq!(filter_string(level, None), level);
        }
    }

    #[test]
    fn info_and_debug_go_to_stdout_only() {
        for level in [
            tracing::Level::TRACE,
            tracing::Level::DEBUG,
            tracing::Level::INFO,
        ] {
            let t = console_target(&level);
            assert!(t.stdout, "{level} should reach stdout");
            assert!(!t.stderr, "{level} must not duplicate onto stderr");
        }
    }

    #[test]
    fn warn_and_error_go_to_stderr_only() {
        for level in [tracing::Level::WARN, tracing::Level::ERROR] {
            let t = console_target(&level);
            assert!(t.stderr, "{level} should reach stderr");
            assert!(!t.stdout, "{level} must not duplicate onto stdout");
        }
    }

    #[test]
    fn every_level_is_emitted_exactly_once() {
        for level in [
            tracing::Level::TRACE,
            tracing::Level::DEBUG,
            tracing::Level::INFO,
            tracing::Level::WARN,
            tracing::Level::ERROR,
        ] {
            let t = console_target(&level);
            assert!(
                t.stdout ^ t.stderr,
                "{level} must be written to exactly one console stream"
            );
        }
    }
}
