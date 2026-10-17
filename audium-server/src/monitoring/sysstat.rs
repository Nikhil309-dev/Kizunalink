// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

//! Process CPU sampling for the `/v4/stats` `lavalinkLoad` field.
//!
//! This replaces the `perf_monitor` crate, which was used for this single
//! metric but pulled `bindgen` (a libclang build-time requirement) and, through
//! it, the unsound/unmaintained `atty` and `ansi_term` into the build graph.
//! `sysinfo` is already a dependency and reports the same figure.

use std::process;

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

/// Samples this process's CPU usage between two refreshes.
///
/// `sysinfo` derives CPU usage from the delta of accumulated process CPU time,
/// so a single refresh always reads as zero. The first refresh in [`Self::cur`]
/// primes the baseline and [`Self::cpu`] refreshes again before reading, which
/// gives a real value from the second call onward.
pub struct ProcessStat {
    system: System,
    pid: sysinfo::Pid,
}

impl ProcessStat {
    pub fn cur() -> Self {
        let pid = sysinfo::Pid::from_u32(process::id());
        let mut system = System::new();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().with_cpu(),
        );
        Self { system, pid }
    }

    /// Refreshes and returns this process's CPU usage in percent.
    ///
    /// `sysinfo` reports 100% per fully-busy core, so the caller divides by the
    /// core count to get Lavalink's 0.0–1.0 `lavalinkLoad`.
    pub fn cpu(&mut self) -> Option<f64> {
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[self.pid]),
            true,
            ProcessRefreshKind::nothing().with_cpu(),
        );
        self.system
            .process(self.pid)
            .map(|process| f64::from(process.cpu_usage()))
    }
}

#[cfg(test)]
mod tests {
    use super::ProcessStat;

    #[test]
    fn reports_a_finite_cpu_percentage() {
        let mut stat = ProcessStat::cur();
        // First sample may be 0.0 (no delta yet) but must always be a number.
        let first = stat.cpu().expect("current process is visible to sysinfo");
        assert!(first.is_finite() && first >= 0.0, "cpu was {first}");

        std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
        let second = stat.cpu().expect("current process is visible to sysinfo");
        assert!(second.is_finite() && second >= 0.0, "cpu was {second}");
    }
}
