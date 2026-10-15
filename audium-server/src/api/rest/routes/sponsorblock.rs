// Copyright (c) 2026 nikcodex (Audium)
// Licensed under the MIT License

use std::sync::Arc;

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};

use crate::api::rest::json::{ApiJson, ApiPath};
use crate::server::AppState;

/// GET /v4/sessions/{sessionId}/players/{guildId}/sponsorblock/categories
///
/// Mirror of the `SponsorBlock-Plugin` endpoint. Returns the set of categories
/// the player currently skips.
pub async fn get_categories(
    ApiPath((session_id, guild_id)): ApiPath<(String, String)>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let session_id = audium::common::types::SessionId(session_id);
    let guild_id = audium::common::types::GuildId(guild_id);

    let Some(session) = state.sessions.get(&session_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Session not found" })),
        )
            .into_response();
    };
    let Some(player_arc) = session.players.get(&guild_id).map(|kv| kv.value().clone()) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Player not found" })),
        )
            .into_response();
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
    ApiPath((session_id, guild_id)): ApiPath<(String, String)>,
    State(state): State<Arc<AppState>>,
    ApiJson(body): ApiJson<Vec<String>>,
) -> impl IntoResponse {
    let session_id = audium::common::types::SessionId(session_id);
    let guild_id = audium::common::types::GuildId(guild_id);

    let Some(session) = state.sessions.get(&session_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Session not found" })),
        )
            .into_response();
    };
    let Some(player_arc) = session.players.get(&guild_id).map(|kv| kv.value().clone()) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Player not found" })),
        )
            .into_response();
    };

    let cleaned = clean_categories(body);

    let mut player = player_arc.write().await;
    player.config.sponsorblock.categories = cleaned.clone();
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

#[cfg(test)]
mod tests {
    use super::clean_categories;

    #[test]
    fn keeps_only_known_categories() {
        let out = clean_categories(vec!["sponsor".into(), "not-a-category".into()]);
        assert_eq!(out, vec!["sponsor".to_string()]);
    }

    #[test]
    fn collapses_duplicates_so_the_stored_set_stays_bounded() {
        // A caller can repeat a known category as many times as the 8 MiB body
        // allows; the stored list must not grow with it.
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
}

/// DELETE /v4/sessions/{sessionId}/players/{guildId}/sponsorblock/categories
///
/// Disables SponsorBlock for this player by clearing all categories.
pub async fn delete_categories(
    ApiPath((session_id, guild_id)): ApiPath<(String, String)>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let session_id = audium::common::types::SessionId(session_id);
    let guild_id = audium::common::types::GuildId(guild_id);

    let Some(session) = state.sessions.get(&session_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Session not found" })),
        )
            .into_response();
    };
    let Some(player_arc) = session.players.get(&guild_id).map(|kv| kv.value().clone()) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Player not found" })),
        )
            .into_response();
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
    ApiPath((session_id, guild_id)): ApiPath<(String, String)>,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let session_id = audium::common::types::SessionId(session_id);
    let guild_id = audium::common::types::GuildId(guild_id);

    let Some(session) = state.sessions.get(&session_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Session not found" })),
        )
            .into_response();
    };
    let Some(player_arc) = session.players.get(&guild_id).map(|kv| kv.value().clone()) else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Player not found" })),
        )
            .into_response();
    };

    let player = player_arc.read().await;
    let segments = player.sponsorblock.segments.read().await.clone();
    (StatusCode::OK, Json(segments)).into_response()
}
