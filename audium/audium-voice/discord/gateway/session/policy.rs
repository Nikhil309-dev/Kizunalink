// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use super::types::SessionOutcome;

/// Defines the strategy for handling session failures and closure codes.
pub struct FailurePolicy {
    max_retries: u32,
}

impl FailurePolicy {
    pub const fn new(max_retries: u32) -> Self {
        Self { max_retries }
    }

    /// Determines if a WebSocket close code is retryable without notifying the client.
    ///
    /// these codes trigger an internal reconnection attempt
    /// and should be suppressed from external event emissions during the initial retry phase.
    pub const fn is_retryable(&self, code: u16, attempt: u32) -> bool {
        if attempt >= self.max_retries {
            return false;
        }

        matches!(
            code,
            1000 | 1001
                | 1006
                | 4000
                | 4001
                | 4002
                | 4003
                | 4005
                | 4006
                | 4009
                | 4012
                | 4013
                | 4014
                | 4015
                | 4016
                | 4020
                | 4900
        )
    }

    /// Maps a Discord closure code to a high-level session outcome.
    ///
    /// Codes are taken from Discord's voice close-code table. The three outcomes
    /// are deliberately distinct:
    /// - `Shutdown`: the session cannot be recovered by this node (bad token,
    ///   kicked, rate limited, channel gone). Reconnecting would loop.
    /// - `Identify`: the websocket is fine but the session is stale, so a fresh
    ///   identify is required rather than a bare reconnect.
    /// - `Reconnect`: a plain reconnect is the right recovery.
    pub fn classify(&self, code: u16) -> SessionOutcome {
        match code {
            4004 | 4011 | 4021 | 4022 => SessionOutcome::Shutdown,

            4006 | 4009 | 4014 => SessionOutcome::Identify,

            4017 => SessionOutcome::Shutdown,
            _ => SessionOutcome::Reconnect,
        }
    }

    /// True when a close code indicates the node is misconfigured or
    /// unsupported, as opposed to a transient network failure.
    ///
    /// Callers use this to log an actionable message and stop retrying rather
    /// than emitting a bare "reconnecting" line.
    pub const fn is_configuration_failure(code: u16) -> bool {
        matches!(code, 4004 | 4011 | 4017)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_policy_new() {
        let policy = FailurePolicy::new(5);
        assert!(policy.is_retryable(1000, 0));
    }

    #[test]
    fn test_is_retryable_normal_codes() {
        let policy = FailurePolicy::new(3);

        assert!(policy.is_retryable(1000, 0));
        assert!(policy.is_retryable(1001, 1));
        assert!(policy.is_retryable(1006, 2));

        assert!(!policy.is_retryable(1000, 3));
        assert!(!policy.is_retryable(1006, 5));
    }

    #[test]
    fn test_is_retryable_discord_codes() {
        let policy = FailurePolicy::new(3);

        assert!(policy.is_retryable(4000, 0));
        assert!(policy.is_retryable(4001, 1));
        assert!(policy.is_retryable(4002, 0));
        assert!(policy.is_retryable(4003, 1));
        assert!(policy.is_retryable(4005, 2));
        assert!(policy.is_retryable(4006, 0));
        assert!(policy.is_retryable(4009, 1));
        assert!(policy.is_retryable(4012, 0));
        assert!(policy.is_retryable(4015, 1));
        assert!(policy.is_retryable(4016, 0));
        assert!(policy.is_retryable(4020, 1));
        assert!(policy.is_retryable(4900, 2));
    }

    #[test]
    fn test_is_not_retryable_codes() {
        let policy = FailurePolicy::new(3);

        assert!(!policy.is_retryable(4004, 0));
        assert!(!policy.is_retryable(4011, 1));
        assert!(!policy.is_retryable(4999, 0));
        assert!(!policy.is_retryable(1002, 1));
    }

    #[test]
    fn test_classify_shutdown_codes() {
        let policy = FailurePolicy::new(3);

        assert_eq!(policy.classify(4004), SessionOutcome::Shutdown);
        assert_eq!(policy.classify(4011), SessionOutcome::Shutdown);
        assert_eq!(policy.classify(4021), SessionOutcome::Shutdown);
        assert_eq!(policy.classify(4022), SessionOutcome::Shutdown);
    }

    #[test]
    fn test_classify_identify_codes() {
        let policy = FailurePolicy::new(3);

        assert_eq!(policy.classify(4006), SessionOutcome::Identify);
        assert_eq!(policy.classify(4009), SessionOutcome::Identify);
        assert_eq!(policy.classify(4014), SessionOutcome::Identify);
    }

    #[test]
    fn test_classify_reconnect_codes() {
        let policy = FailurePolicy::new(3);

        assert_eq!(policy.classify(1000), SessionOutcome::Reconnect);
        assert_eq!(policy.classify(1001), SessionOutcome::Reconnect);
        assert_eq!(policy.classify(4000), SessionOutcome::Reconnect);
        assert_eq!(policy.classify(4001), SessionOutcome::Reconnect);
        assert_eq!(policy.classify(4999), SessionOutcome::Reconnect);
        assert_eq!(policy.classify(5000), SessionOutcome::Reconnect);
    }

    #[test]
    fn test_retry_boundary() {
        let policy = FailurePolicy::new(3);

        assert!(policy.is_retryable(1000, 2));

        assert!(!policy.is_retryable(1000, 3));

        assert!(!policy.is_retryable(1000, 4));
        assert!(!policy.is_retryable(1000, 100));
    }

    #[test]
    fn test_zero_retries() {
        let policy = FailurePolicy::new(0);

        assert!(!policy.is_retryable(1000, 0));
        assert!(!policy.is_retryable(4000, 0));
    }

    #[test]
    fn e2ee_required_is_terminal_not_a_reconnect_loop() {
        let policy = FailurePolicy::new(5);
        assert_eq!(policy.classify(4017), SessionOutcome::Shutdown);
        assert!(FailurePolicy::is_configuration_failure(4017));
    }

    #[test]
    fn webrtc_crash_is_retryable() {
        let policy = FailurePolicy::new(5);
        assert!(policy.is_retryable(4013, 0));
        assert_eq!(policy.classify(4013), SessionOutcome::Reconnect);
    }

    #[test]
    fn voice_server_move_reidentifies_and_is_retryable() {
        let policy = FailurePolicy::new(5);
        assert_eq!(policy.classify(4014), SessionOutcome::Identify);
        assert!(policy.is_retryable(4014, 0));
    }

    #[test]
    fn only_misconfiguration_codes_are_configuration_failures() {
        assert!(!FailurePolicy::is_configuration_failure(4022));
        assert!(!FailurePolicy::is_configuration_failure(4021));
        assert!(!FailurePolicy::is_configuration_failure(4014));
        assert!(FailurePolicy::is_configuration_failure(4004));
        assert!(FailurePolicy::is_configuration_failure(4011));
    }
}
