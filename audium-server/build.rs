// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! Stamps build provenance into the binary so `/v4/info` can report real
//! `buildTime` / `git` metadata (matching Lavalink and NodeLink) instead of the
//! `0` / `"unknown"` placeholders.
//!
//! Values are read at compile time via `option_env!` in `info.rs`. Anything the
//! CI environment already exports (`BUILD_TIME`, `GIT_COMMIT`, …) wins; otherwise
//! we best-effort derive it from the local git checkout, and fall back to a
//! sentinel that `info.rs` treats as "unset" so we never emit a bogus timestamp.

use std::{
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Emit a `cargo:rustc-env` only when we have a trustworthy value; leaving the
/// variable unset lets `option_env!` yield `None` and the handler fall back.
fn set(key: &str, value: Option<String>) {
    if let Some(v) = value {
        println!("cargo:rustc-env={key}={v}");
    }
}

/// Normalises an epoch timestamp to milliseconds.
///
/// `/v4/info` reports `buildTime` and `git.commitTime` as *millisecond* unix
/// timestamps, matching Lavalink (`InfoSerializerTest`) and NodeLink. CI exports
/// epoch *seconds* (`date -u +%s`, `git log --format=%ct`) and the local
/// fallbacks below produce seconds too, so a value that is too small to be
/// milliseconds is scaled up. Already-millisecond input is passed through.
fn to_millis(epoch: &str) -> String {
    match epoch.trim().parse::<i64>() {
        // Anything below ~1973-03 in milliseconds is really a second count.
        Ok(secs) if secs > 0 && secs < 100_000_000_000 => (secs * 1000).to_string(),
        _ => epoch.trim().to_string(),
    }
}

fn main() {
    // Re-run when the checked-out revision changes.
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs/heads");
    println!("cargo:rerun-if-env-changed=BUILD_TIME");
    println!("cargo:rerun-if-env-changed=GIT_COMMIT");
    println!("cargo:rerun-if-env-changed=GIT_BRANCH");
    println!("cargo:rerun-if-env-changed=GIT_COMMIT_TIME");

    // Build time: prefer the CI-provided epoch, else derive at compile time.
    let build_time = std::env::var("BUILD_TIME").ok().or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs().to_string())
    });
    set("BUILD_TIME", build_time.as_deref().map(to_millis));

    set(
        "GIT_COMMIT",
        std::env::var("GIT_COMMIT")
            .ok()
            .or_else(|| git(&["rev-parse", "HEAD"])),
    );
    set(
        "GIT_COMMIT_SHORT",
        std::env::var("GIT_COMMIT_SHORT")
            .ok()
            .or_else(|| git(&["rev-parse", "--short", "HEAD"])),
    );
    set(
        "GIT_BRANCH",
        std::env::var("GIT_BRANCH")
            .ok()
            .or_else(|| git(&["rev-parse", "--abbrev-ref", "HEAD"])),
    );
    set(
        "GIT_COMMIT_TIME",
        std::env::var("GIT_COMMIT_TIME")
            .ok()
            .or_else(|| git(&["log", "-1", "--format=%ct"]))
            .as_deref()
            .map(to_millis),
    );
}
