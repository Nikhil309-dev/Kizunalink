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
            1000 | // NORMAL (User requested suppression)
            1001 | // GOING_AWAY
            1006 | // ABNORMAL_CLOSURE
            4000 | // INTERNAL_ERROR
            4001 | // UNKNOWN_OPCODE
            4002 | // FAILED_TO_DECODE_PAYLOAD
            4003 | // NOT_AUTHENTICATED
            4005 | // ALREADY_AUTHENTICATED
            4006 | // SESSION_NO_LONGER_VALID
            4009 | // SESSION_TIMEOUT
            4012 | // UNKNOWN_PROTOCOL
            4013 | // WEBRTC_CRASHED (resume)
            4014 | // DISCONNECTED (voice-server move; see classify)
            4015 | // VOICE_SERVER_CRASHED
            4016 | // UNKNOWN_ENCRYPTION_MODE
            4020 | // BAD_REQUEST
            4900 // RECONNECT
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
            // Terminal: bad auth, server not found, rate limited, channel
            // deleted / voice server changed / call ended.
            4004 | 4011 | 4021 | 4022 => SessionOutcome::Shutdown,
            // Stale session: re-identify instead of reconnecting.
            //
            // 4014 is "disconnected" and the docs say not to reconnect, but the
            // same code is raised on a benign voice-server change when the bot
            // moves channels. Re-identifying is the recovery that keeps playback
            // working across a move, which is the behaviour users expect.
            4006 | 4009 | 4014 => SessionOutcome::Identify,
            // E2EE required. Audium implements DAVE, so this should not fire; if
            // it does, it means the negotiated protocol version was one we do not
            // support. Reconnecting immediately would hot-loop exactly like the
            // broken clients in the wild, so surface it as a terminal error with
            // a nameable cause instead.
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
        matches!(
            code,
            4004 | // AUTHENTICATION_FAILED
            4011 | // SERVER_NOT_FOUND
            4017 // E2EE_REQUIRED (unsupported DAVE version)
        )
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

        // Within retry limit
        assert!(policy.is_retryable(1000, 0)); // NORMAL
        assert!(policy.is_retryable(1001, 1)); // GOING_AWAY
        assert!(policy.is_retryable(1006, 2)); // ABNORMAL_CLOSURE

        // At retry limit
        assert!(!policy.is_retryable(1000, 3));
        assert!(!policy.is_retryable(1006, 5));
    }

    #[test]
    fn test_is_retryable_discord_codes() {
        let policy = FailurePolicy::new(3);

        assert!(policy.is_retryable(4000, 0)); // INTERNAL_ERROR
        assert!(policy.is_retryable(4001, 1)); // UNKNOWN_OPCODE
        assert!(policy.is_retryable(4002, 0)); // FAILED_TO_DECODE_PAYLOAD
        assert!(policy.is_retryable(4003, 1)); // NOT_AUTHENTICATED
        assert!(policy.is_retryable(4005, 2)); // ALREADY_AUTHENTICATED
        assert!(policy.is_retryable(4006, 0)); // SESSION_NO_LONGER_VALID
        assert!(policy.is_retryable(4009, 1)); // SESSION_TIMEOUT
        assert!(policy.is_retryable(4012, 0)); // UNKNOWN_PROTOCOL
        assert!(policy.is_retryable(4015, 1)); // VOICE_SERVER_CRASHED
        assert!(policy.is_retryable(4016, 0)); // UNKNOWN_ENCRYPTION_MODE
        assert!(policy.is_retryable(4020, 1)); // BAD_REQUEST
        assert!(policy.is_retryable(4900, 2)); // RECONNECT
    }

    #[test]
    fn test_is_not_retryable_codes() {
        let policy = FailurePolicy::new(3);

        // Non-retryable codes
        assert!(!policy.is_retryable(4004, 0)); // Not in retryable list
        assert!(!policy.is_retryable(4011, 1));
        assert!(!policy.is_retryable(4999, 0)); // Unknown code
        assert!(!policy.is_retryable(1002, 1)); // Not in retryable list
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

        // All other codes should result in Reconnect
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

        // Just before limit
        assert!(policy.is_retryable(1000, 2));

        // At limit
        assert!(!policy.is_retryable(1000, 3));

        // Beyond limit
        assert!(!policy.is_retryable(1000, 4));
        assert!(!policy.is_retryable(1000, 100));
    }

    #[test]
    fn test_zero_retries() {
        let policy = FailurePolicy::new(0);

        // No retries allowed
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
