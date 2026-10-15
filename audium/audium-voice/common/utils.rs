// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use std::{
    sync::Mutex as StdMutex,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

// ANSI Color Codes
pub const RESET: &str = "\x1b[0m";
pub const BOLD: &str = "\x1b[1m";
pub const DIM: &str = "\x1b[2m";

pub const ORANGE: &str = "\x1b[38;5;208m";
pub const GREEN: &str = "\x1b[32m";
pub const CYAN: &str = "\x1b[36m";
pub const YELLOW: &str = "\x1b[33m";
pub const BLUE: &str = "\x1b[34m";
pub const MAGENTA: &str = "\x1b[35m";
pub const RED: &str = "\x1b[31m";

// Log Level Colors
pub const COLOR_ERROR: &str = RED;
pub const COLOR_WARN: &str = YELLOW;
pub const COLOR_INFO: &str = GREEN;
pub const COLOR_DEBUG: &str = BLUE;
pub const COLOR_TRACE: &str = MAGENTA;

/// Whether ANSI color escape codes should be emitted to stdout (N07).
///
/// Precedence follows the ecosystem conventions:
/// 1. `CLICOLOR_FORCE` set to anything other than `0` — always color.
/// 2. `NO_COLOR` set to a non-empty value — never color (no-color.org).
/// 3. `CLICOLOR=0` — never color.
/// 4. `TERM=dumb` — never color.
/// 5. otherwise: only when stdout is an attached terminal.
pub fn colors_enabled() -> bool {
    use std::io::IsTerminal as _;

    if std::env::var("CLICOLOR_FORCE").is_ok_and(|v| v != "0") {
        return true;
    }
    if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    if std::env::var("CLICOLOR").is_ok_and(|v| v == "0") {
        return false;
    }
    if std::env::var("TERM").is_ok_and(|v| v == "dumb") {
        return false;
    }
    std::io::stdout().is_terminal()
}

/// Returns the current time in milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Truncate `s` to at most `max_chars` characters, appending `…` when cut.
///
/// Byte slicing (`&s[..47]`) panics when the offset lands inside a multi-byte
/// UTF-8 character, which is reachable from any user-supplied text (TTS
/// queries, search terms). Count characters, not bytes.
pub fn truncate_chars(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let mut out = String::new();
    for _ in 0..max_chars {
        match chars.next() {
            Some(c) => out.push(c),
            None => return out,
        }
    }
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

/// Simple ANSI stripper to prevent the log file from being polluted with escape sequences.
pub fn strip_ansi_escapes(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if let Some('[') = chars.peek() {
                chars.next();
                while let Some(&nc) = chars.peek() {
                    chars.next();
                    if nc.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

static RESOURCE_CACHE: StdMutex<Option<(String, Instant)>> = StdMutex::new(None);

/// Queries the program's current memory profile.
pub fn memory_usage_report() -> String {
    let mut guard = RESOURCE_CACHE.lock().unwrap_or_else(|e| e.into_inner());

    if let Some((report, timestamp)) = guard.as_ref()
        && timestamp.elapsed().as_secs() < 1
    {
        return report.clone();
    }

    let mut system = sysinfo::System::new();
    let pid = sysinfo::Pid::from_u32(std::process::id());
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[pid]),
        true,
        sysinfo::ProcessRefreshKind::nothing().with_memory(),
    );

    let bytes = system.process(pid).map(|p| p.memory()).unwrap_or(0);
    let formatted = format_byte_size(bytes);

    *guard = Some((formatted.clone(), Instant::now()));
    formatted
}

fn format_byte_size(bytes: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut index = 0;

    while size >= 1024.0 && index < units.len() - 1 {
        size /= 1024.0;
        index += 1;
    }

    format!("{:.2} {}", size, units[index])
}

pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36";

/// Returns the default User-Agent string.
pub fn default_user_agent() -> String {
    DEFAULT_USER_AGENT.to_string()
}

#[cfg(test)]
mod tests {
    use super::truncate_chars;

    #[test]
    fn truncate_chars_never_splits_a_multibyte_character() {
        // The old `&text[..47]` idiom panicked here: byte 47 lands inside a
        // 3-byte CJK character.
        let text = "楽".repeat(80);
        let out = truncate_chars(&text, 50);
        assert_eq!(out.chars().filter(|c| *c == '楽').count(), 50);
        assert!(out.ends_with('…'));

        let emoji = "🎵".repeat(30);
        let out = truncate_chars(&emoji, 10);
        assert_eq!(out.chars().filter(|c| *c == '🎵').count(), 10);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn truncate_chars_leaves_short_strings_untouched() {
        assert_eq!(truncate_chars("hello", 50), "hello");
        assert_eq!(truncate_chars("", 5), "");
        assert_eq!(truncate_chars("exact", 5), "exact");
        assert_eq!(truncate_chars(&"楽".repeat(30), 50), "楽".repeat(30));
    }

    #[test]
    fn truncate_chars_zero_budget_yields_only_the_ellipsis() {
        assert_eq!(truncate_chars("abc", 0), "…");
        assert_eq!(truncate_chars("", 0), "");
    }
}
