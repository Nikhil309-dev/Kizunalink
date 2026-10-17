// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use std::sync::Arc;

use axum::{
    Json,
    extract::{OriginalUri, State},
    http::StatusCode,
    response::IntoResponse,
};

use crate::api::rest::json::{self, ApiJson, ApiPath};
use crate::server::AppState;

/// GET /v4/sessions/{sessionId}/players/{guildId}/sponsorblock/categories
///
/// Mirror of the `SponsorBlock-Plugin` endpoint. Returns the set of categories
/// the player currently skips.
pub async fn get_categories(
    OriginalUri(uri): OriginalUri,
    ApiPath((session_id, guild_id)): ApiPath<(String, String)>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let session_id = audium::common::types::SessionId(session_id);
    let guild_id = audium::common::types::GuildId(guild_id);
    let path = json::path_of(&uri);

    let Some(session) = state.sessions.get(&session_id) else {
        return json::not_found(format!("Session not found: {session_id}"), &path);
    };
    let Some(player_arc) = session.players.get(&guild_id).map(|kv| kv.value().clone()) else {
        return json::not_found(format!("Player not found for guild: {guild_id}"), &path);
    };

    let player = player_arc.read().await;
    let categories = player.config.sponsorblock.categories.clone();
    (StatusCode::OK, Json(categories)).into_response()
}

/// PUT /v4/sessions/{sessionId}/players/{guildId}/sponsorblock/categories
///
/// Replaces the set of categories the player skips. Must be a subset of the
/// categories the SponsorBlock API knows.
pub async fn set_categories(
    OriginalUri(uri): OriginalUri,
    ApiPath((session_id, guild_id)): ApiPath<(String, String)>,
    State(state): State<Arc<AppState>>,
    ApiJson(body): ApiJson<Vec<String>>,
) -> impl IntoResponse {
    let session_id = audium::common::types::SessionId(session_id);
    let guild_id = audium::common::types::GuildId(guild_id);
    let path = json::path_of(&uri);

    let Some(session) = state.sessions.get(&session_id) else {
        return json::not_found(format!("Session not found: {session_id}"), &path);
    };
    let Some(player_arc) = session.players.get(&guild_id).map(|kv| kv.value().clone()) else {
        return json::not_found(format!("Player not found for guild: {guild_id}"), &path);
    };

    let cleaned = clean_categories(body);

    let mut player = player_arc.write().await;
    player.config.sponsorblock.categories = cleaned.clone();

    player.config.sponsorblock.enabled = !cleaned.is_empty();
    (StatusCode::OK, Json(cleaned)).into_response()
}

/// Keep only known SponsorBlock categories, de-duplicated.
///
/// The request body is bounded only by `server.max_body_bytes` (8 MiB), so a
/// client can send a long run of duplicate names. Filtering to the known set and
/// de-duplicating bounds the stored category list to `KNOWN_CATEGORIES.len()`.
fn clean_categories(body: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    body.into_iter()
        .filter(|c| {
            audium::lavalink::sponsorblock::KNOWN_CATEGORIES
                .iter()
                .any(|k| k == c)
        })
        .filter(|c| seen.insert(c.clone()))
        .collect()
}

/// DELETE /v4/sessions/{sessionId}/players/{guildId}/sponsorblock/categories
///
/// Disables SponsorBlock for this player by clearing all categories.
pub async fn delete_categories(
    OriginalUri(uri): OriginalUri,
    ApiPath((session_id, guild_id)): ApiPath<(String, String)>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let session_id = audium::common::types::SessionId(session_id);
    let guild_id = audium::common::types::GuildId(guild_id);
    let path = json::path_of(&uri);

    let Some(session) = state.sessions.get(&session_id) else {
        return json::not_found(format!("Session not found: {session_id}"), &path);
    };
    let Some(player_arc) = session.players.get(&guild_id).map(|kv| kv.value().clone()) else {
        return json::not_found(format!("Player not found for guild: {guild_id}"), &path);
    };

    let mut player = player_arc.write().await;
    player.config.sponsorblock.categories.clear();
    player.config.sponsorblock.enabled = false;
    (StatusCode::OK, Json(serde_json::json!({}))).into_response()
}

/// GET /v4/sessions/{sessionId}/players/{guildId}/sponsorblock/segments
///
/// Returns the currently-cached segments for the active track (or `[]`).
pub async fn get_segments(
    OriginalUri(uri): OriginalUri,
    ApiPath((session_id, guild_id)): ApiPath<(String, String)>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let session_id = audium::common::types::SessionId(session_id);
    let guild_id = audium::common::types::GuildId(guild_id);
    let path = json::path_of(&uri);

    let Some(session) = state.sessions.get(&session_id) else {
        return json::not_found(format!("Session not found: {session_id}"), &path);
    };
    let Some(player_arc) = session.players.get(&guild_id).map(|kv| kv.value().clone()) else {
        return json::not_found(format!("Player not found for guild: {guild_id}"), &path);
    };

    let player = player_arc.read().await;
    let segments = player.sponsorblock.segments.read().await.clone();
    (StatusCode::OK, Json(segments)).into_response()
}

#[cfg(test)]
mod tests {
    use super::clean_categories;

    /// Mirrors what `set_categories` writes to the player for a cleaned body, so
    /// the round-trip with `delete_categories` can be asserted without an HTTP
    /// harness.
    fn put(cfg: &mut audium::config::player::SponsorBlockConfig, body: Vec<String>) {
        let cleaned = clean_categories(body);
        cfg.categories = cleaned.clone();
        cfg.enabled = !cleaned.is_empty();
    }

    fn delete(cfg: &mut audium::config::player::SponsorBlockConfig) {
        cfg.categories.clear();
        cfg.enabled = false;
    }

    #[test]
    fn keeps_only_known_categories() {
        let out = clean_categories(vec!["sponsor".into(), "not-a-category".into()]);
        assert_eq!(out, vec!["sponsor".to_string()]);
    }

    #[test]
    fn collapses_duplicates_so_the_stored_set_stays_bounded() {
        let out = clean_categories(vec!["sponsor".into(); 100_000]);
        assert_eq!(out, vec!["sponsor".to_string()]);
    }

    #[test]
    fn preserves_order_of_first_occurrence() {
        let out = clean_categories(vec![
            "intro".into(),
            "sponsor".into(),
            "intro".into(),
            "outro".into(),
        ]);
        assert_eq!(out, vec!["intro", "sponsor", "outro"]);
    }

    #[test]
    fn delete_then_put_turns_skipping_back_on() {
        let mut cfg = audium::config::player::SponsorBlockConfig {
            enabled: true,
            ..Default::default()
        };

        delete(&mut cfg);
        assert!(!cfg.enabled, "DELETE must disable skipping");

        put(&mut cfg, vec!["sponsor".into()]);
        assert_eq!(cfg.categories, vec!["sponsor".to_string()]);
        assert!(
            cfg.enabled,
            "PUT of a non-empty category set must re-enable skipping"
        );
    }

    #[test]
    fn put_of_only_unknown_categories_leaves_skipping_disabled() {
        let mut cfg = audium::config::player::SponsorBlockConfig::default();
        put(&mut cfg, vec!["bogus".into()]);
        assert!(cfg.categories.is_empty());
        assert!(
            !cfg.enabled,
            "an empty cleaned set must not enable skipping"
        );
    }
}
