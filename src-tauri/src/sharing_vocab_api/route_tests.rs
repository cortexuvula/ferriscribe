//! Route-level tests for the sharing vocab API — the HTTP surface paired
//! clients drive against the machine holding the PHI database. These were
//! the app's least-tested code before 2026-08-25 (~1,900 lines, one test):
//! routing, bearer auth, payload validation, and the CRUD/merge handlers
//! are exercised here through the real axum router (`tower::ServiceExt::
//! oneshot`), with an in-memory database, a real `TokenStore` in a temp
//! dir, and tauri's `MockRuntime` app handle.
//!
//! Deliberately NOT covered here: SSE streams (`/events` endpoints need a
//! long-lived connection shape that oneshot can't express) and the audio
//! byte round-trip (encrypted-file plumbing) — both are covered end-to-end
//! by the medical-sharing integration tests from the client side.

use std::sync::Arc;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use medical_db::Database;
use medical_sharing::token_store::TokenStore;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{ApiState, build_router};

struct TestApp {
    router: Router,
    token: String,
    tokens: Arc<TokenStore>,
    /// In-memory database (also inside the router's state) for direct
    /// seeding/inspection in tests.
    db: Arc<Database>,
    /// The router's own state, retained for direct handler-core tests
    /// (e.g. the generate-validation module below).
    state: ApiState<tauri::test::MockRuntime>,
    /// Keeps the mock app (event loop owner) and the temp token-store dir
    /// alive for the lifetime of the test.
    _app: tauri::App<tauri::test::MockRuntime>,
    _tmp: tempfile::TempDir,
}

async fn test_app() -> TestApp {
    let db = Arc::new(Database::open_in_memory().expect("in-memory db"));
    let db_for_app = Arc::clone(&db);
    let tmp = tempfile::tempdir().expect("tempdir");
    let tokens =
        Arc::new(TokenStore::open(tmp.path().join("tokens.db"), &[7u8; 32]).expect("token store"));
    let issued = tokens.issue("route-tests").expect("issue token");
    let tokens_for_state = Arc::clone(&tokens);
    let app = tauri::test::mock_app();
    let jobs = Arc::new(super::mobile::JobRegistry::new());
    // Same bridge the real spawn() installs — lets tests drive the registry
    // by emitting pipeline/generation events the way the commands do.
    // (Never detached here: the mock app dies with the test.)
    let _ = super::mobile::attach_event_forwarders(app.handle(), &jobs);
    let state = ApiState {
        db,
        tokens: tokens_for_state,
        chips_changed_tx: tokio::sync::broadcast::channel(16).0,
        dict_changed_tx: tokio::sync::broadcast::channel(16).0,
        content_changed_tx: tokio::sync::broadcast::channel(32).0,
        data_dir: tmp.path().to_path_buf(),
        app_handle: app.handle().clone(),
        merge_lock: Arc::new(tokio::sync::Mutex::new(())),
        fail_limiter: Arc::new(std::sync::Mutex::new(
            medical_security::rate_limiter::RateLimiter::new(5),
        )),
        jobs,
    };
    TestApp {
        router: build_router(state.clone()),
        token: issued.token,
        tokens,
        db: db_for_app,
        state,
        _app: app,
        _tmp: tmp,
    }
}

/// One-shot request through the real router. Returns (status, parsed body).
async fn req(
    app: &TestApp,
    method: &str,
    uri: &str,
    bearer: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(b) = bearer {
        builder = builder.header("authorization", format!("Bearer {b}"));
    }
    let body = match body {
        Some(v) => Body::from(v.to_string()),
        None => Body::empty(),
    };
    let response = app
        .router
        .clone()
        .oneshot(builder.body(body).expect("request"))
        .await
        .expect("router responds");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("body");
    let json = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
    };
    (status, json)
}

fn authed(app: &TestApp) -> Option<&str> {
    Some(app.token.as_str())
}

// ── Auth ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn auth_rejects_missing_and_wrong_bearer_on_every_route_group() {
    // Fresh app per route: authorize now consumes a shared fail-limiter
    // bucket on bad/missing bearers (5 slots), and 12 failures in one app
    // would trip 429s instead of the exact 401s asserted here.
    for uri in [
        "/v1/vocabulary",
        "/v1/context-templates",
        "/v1/user-dictionary",
        "/v1/condition-chips",
        "/v1/content/sync/meta",
        "/v1/content/audio/00000000-0000-0000-0000-000000000000",
    ] {
        let app = test_app().await;
        let (status, _) = req(&app, "GET", uri, None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "no bearer on {uri}");
        let (status, _) = req(&app, "GET", uri, Some("forged-token"), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "bad bearer on {uri}");
    }
}

#[tokio::test]
async fn auth_rate_limits_after_budget_exhausted() {
    let app = test_app().await;
    // 5-slot bucket: the first 5 forged attempts get honest 401s, the 6th
    // onward get 429 until the bucket refills.
    let mut saw_401 = 0;
    let mut saw_429 = false;
    for _ in 0..8 {
        let (status, _) = req(&app, "GET", "/v1/vocabulary", Some("forged-token"), None).await;
        match status {
            StatusCode::UNAUTHORIZED => saw_401 += 1,
            StatusCode::TOO_MANY_REQUESTS => saw_429 = true,
            other => panic!("expected 401 or 429, got {other} on /v1/vocabulary"),
        }
    }
    assert_eq!(saw_401, 5, "exactly the bucket size gets honest 401s");
    assert!(saw_429, "budget exhaustion must throttle");
}

#[tokio::test]
async fn auth_rejects_revoked_token() {
    let app = test_app().await;
    let (status, _) = req(&app, "GET", "/v1/vocabulary", authed(&app), None).await;
    assert_eq!(status, StatusCode::OK);

    // Revoke every client row; the previously valid token must now 401 —
    // pairing revocation is the PHI boundary for this whole API.
    for row in app.tokens.list().expect("list clients") {
        app.tokens.revoke(row.id).expect("revoke");
    }
    let (status, _) = req(&app, "GET", "/v1/vocabulary", authed(&app), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ── Vocabulary ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn vocabulary_crud_roundtrip() {
    let app = test_app().await;

    // Insert.
    let (status, entry) = req(
        &app,
        "POST",
        "/v1/vocabulary",
        authed(&app),
        Some(json!({"find_text": "hte", "replacement": "the"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "insert: {entry}");
    let id = entry["id"].as_str().expect("id").to_string();

    // List contains it; count is (total, enabled).
    let (status, list) = req(&app, "GET", "/v1/vocabulary", authed(&app), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().expect("list").len(), 1);
    let (status, count) = req(&app, "GET", "/v1/vocabulary/count", authed(&app), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(count, json!([1, 1]), "count = {count}");

    // Update by id.
    let (status, updated) = req(
        &app,
        "PUT",
        &format!("/v1/vocabulary/{id}"),
        authed(&app),
        Some(json!({"find_text": "hte", "replacement": "THE", "enabled": false})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "update: {updated}");
    assert_eq!(updated["replacement"], "THE");
    assert_eq!(updated["enabled"], false);

    // Malformed uuid → 400, not 500.
    let (status, _) = req(
        &app,
        "PUT",
        "/v1/vocabulary/not-a-uuid",
        authed(&app),
        Some(json!({"find_text": "x", "replacement": "y"})),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Delete one → 204, list empty again.
    let (status, _) = req(
        &app,
        "DELETE",
        &format!("/v1/vocabulary/{id}"),
        authed(&app),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (_, count) = req(&app, "GET", "/v1/vocabulary/count", authed(&app), None).await;
    assert_eq!(count, json!([0, 0]));
}

// ── Context templates ───────────────────────────────────────────────────────

#[tokio::test]
async fn templates_upsert_rename_delete() {
    let app = test_app().await;

    let (status, tpl) = req(
        &app,
        "POST",
        "/v1/context-templates/upsert",
        authed(&app),
        Some(json!({"name": "Cardio follow-up", "body": "Cardiac history: ..."})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "upsert: {tpl}");
    assert_eq!(tpl["name"], "Cardio follow-up");

    let (_, list) = req(&app, "GET", "/v1/context-templates", authed(&app), None).await;
    assert_eq!(list.as_array().expect("list").len(), 1);

    let (status, renamed) = req(
        &app,
        "POST",
        "/v1/context-templates/rename",
        authed(&app),
        Some(json!({"old_name": "Cardio follow-up", "new_name": "Cardio FU"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "rename: {renamed}");
    assert_eq!(renamed["name"], "Cardio FU");

    let (status, _) = req(
        &app,
        "POST",
        "/v1/context-templates/delete",
        authed(&app),
        Some(json!({"name": "Cardio FU"})),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Deleting a missing template errors (500 today — 'not found' maps to
    // the generic handler error), never silently 204s. Pin whichever it is
    // so a change is noticed.
    let (status, _) = req(
        &app,
        "POST",
        "/v1/context-templates/delete",
        authed(&app),
        Some(json!({"name": "missing"})),
    )
    .await;
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
}

// ── User dictionary ─────────────────────────────────────────────────────────

/// The feedback-storm fix (tracked item 7): every client list pushes a full
/// sync, so the server must broadcast `dict_changed` ONLY when a merge
/// actually wrote something — otherwise each broadcast makes other clients
/// re-list, which pushes again, amplifying forever.
#[tokio::test]
async fn dictionary_sync_suppresses_broadcast_on_noop_merges() {
    let app = test_app().await;
    // Subscribe to the SSE-side broadcast channel before syncing.
    let mut events = app.state.dict_changed_tx.subscribe();

    // An empty push merges nothing — no broadcast.
    let (status, body) = req(
        &app,
        "POST",
        "/v1/user-dictionary/sync",
        authed(&app),
        Some(json!([])),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "empty sync: {body}");
    assert!(
        matches!(
            events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ),
        "a no-op merge must not broadcast"
    );

    // A push carrying a new word DOES broadcast.
    let (status, body) = req(
        &app,
        "POST",
        "/v1/user-dictionary/sync",
        authed(&app),
        Some(json!([{
            "id": "11111111-1111-1111-1111-111111111111",
            "word": "metformin",
            "updated_at": "2026-09-07T00:00:00.000Z",
            "deleted_at": null
        }])),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "word sync: {body}");
    assert!(events.try_recv().is_ok(), "a real change must broadcast");

    // Re-pushing the same (now stale) word is a no-op again.
    let (status, _) = req(
        &app,
        "POST",
        "/v1/user-dictionary/sync",
        authed(&app),
        Some(json!([{
            "id": "11111111-1111-1111-1111-111111111111",
            "word": "metformin",
            "updated_at": "2026-09-07T00:00:00.000Z",
            "deleted_at": null
        }])),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        matches!(
            events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ),
        "a stale re-push must not broadcast"
    );
}

#[tokio::test]
async fn dictionary_add_list_remove_roundtrip() {
    let app = test_app().await;

    let (status, added) = req(
        &app,
        "POST",
        "/v1/user-dictionary",
        authed(&app),
        Some(json!({"word": "metformin"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(added, json!(true));

    // Duplicate add is a no-op, not an error.
    let (status, added_again) = req(
        &app,
        "POST",
        "/v1/user-dictionary",
        authed(&app),
        Some(json!({"word": "metformin"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(added_again, json!(false));

    let (_, words) = req(&app, "GET", "/v1/user-dictionary", authed(&app), None).await;
    assert_eq!(words, json!(["metformin"]));

    let (status, removed) = req(
        &app,
        "DELETE",
        "/v1/user-dictionary/metformin",
        authed(&app),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(removed, json!(true));

    let (status, removed_again) = req(
        &app,
        "DELETE",
        "/v1/user-dictionary/metformin",
        authed(&app),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(removed_again, json!(false));

    let (_, words) = req(&app, "GET", "/v1/user-dictionary", authed(&app), None).await;
    assert_eq!(words, json!([]));
}

// ── Condition chips ─────────────────────────────────────────────────────────

#[tokio::test]
async fn condition_chips_sync_merges_and_returns_full_list() {
    let app = test_app().await;

    // Client pushes one active chip; server merges and returns the full
    // list (active + tombstones) per the sync contract.
    let chip = json!({
        "id": "hypertension",
        "text": "Hypertension",
        "updated_at": "2026-08-25T00:00:00Z",
        "deleted_at": null,
        "sort_order": 0,
        "use_count": 0,
    });
    let (status, merged) = req(
        &app,
        "POST",
        "/v1/condition-chips/sync",
        authed(&app),
        Some(json!([chip])),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "sync: {merged}");
    let arr = merged.as_array().expect("merged list");
    assert!(arr.iter().any(|c| c["id"] == "hypertension"));

    // The list endpoint returns the same full view.
    let (status, list) = req(&app, "GET", "/v1/condition-chips", authed(&app), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list.as_array().expect("list").len(), arr.len());

    // A tombstone pushed by another client must propagate: same id with a
    // newer timestamp and deleted_at set. The GET contract is the FULL list
    // (active + tombstones) — clients derive the active view locally via
    // merge_incoming, so deletions travel through this endpoint too.
    let tombstone = json!({
        "id": "hypertension",
        "text": "Hypertension",
        "updated_at": "2026-08-26T00:00:00Z",
        "deleted_at": "2026-08-26T00:00:00Z",
        "sort_order": 0,
        "use_count": 0,
    });
    let (status, _) = req(
        &app,
        "POST",
        "/v1/condition-chips/sync",
        authed(&app),
        Some(json!([tombstone])),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, list) = req(&app, "GET", "/v1/condition-chips", authed(&app), None).await;
    let stored = list
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "hypertension")
        .expect("tombstone must survive in the full list");
    assert!(
        stored["deleted_at"].is_string(),
        "tombstoned chip must carry deleted_at: {stored}"
    );
}

// ── Mobile API ───────────────────────────────────────────────────────────────

mod mobile_api_tests {
    use super::*;
    use tauri::Emitter as _;

    /// Insert a recording row directly for document/export tests.
    fn seed_recording(app: &TestApp, transcript: Option<&str>) -> String {
        let conn = app.db.conn().expect("conn");
        let mut rec = medical_core::types::recording::Recording::new(
            "consult.wav",
            std::path::PathBuf::from("/tmp/consult.wav"),
        );
        rec.transcript = transcript.map(|s| s.to_string());
        medical_db::recordings::RecordingsRepo::insert(&conn, &rec).expect("seed insert");
        rec.id.to_string()
    }

    /// One-shot raw-bytes request (export responses are not JSON).
    async fn req_bytes(app: &TestApp, uri: &str, bearer: Option<&str>) -> (StatusCode, Vec<u8>) {
        let mut builder = Request::builder().method("GET").uri(uri).header(
            "authorization",
            format!("Bearer {}", bearer.expect("bearer")),
        );
        builder = builder.header("content-type", "application/json");
        let response = app
            .router
            .clone()
            .oneshot(builder.body(Body::empty()).expect("request"))
            .await
            .expect("router responds");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
            .await
            .expect("body");
        (status, bytes.to_vec())
    }

    #[tokio::test]
    async fn create_recording_roundtrip() {
        let app = test_app().await;
        let (status, body) = req(
            &app,
            "POST",
            "/v1/recordings",
            authed(&app),
            Some(json!({"filename": "consult-2026-09-06.wav", "duration_seconds": 91.5})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "create: {body}");
        let id = body["id"].as_str().expect("id").to_string();
        assert!(uuid::Uuid::parse_str(&id).is_ok(), "server-minted uuid");

        // Duplicate id → 409.
        let (status, _) = req(
            &app,
            "POST",
            "/v1/recordings",
            authed(&app),
            Some(json!({"id": id, "filename": "again.wav"})),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);

        // Validation: empty filename → 400.
        let (status, _) = req(
            &app,
            "POST",
            "/v1/recordings",
            authed(&app),
            Some(json!({"filename": "   "})),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn job_status_requires_known_job() {
        let app = test_app().await;
        let rid = uuid::Uuid::new_v4().to_string();
        let (status, _) = req(&app, "GET", &format!("/v1/jobs/{rid}"), authed(&app), None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn pipeline_events_drive_job_registry() {
        let app = test_app().await;
        let rid = uuid::Uuid::new_v4().to_string();

        // The commands emit these exact payload shapes — mirror them.
        app._app
            .emit(
                "pipeline-progress",
                serde_json::json!({"recording_id": rid, "stage": "transcribing", "error": null}),
            )
            .unwrap();
        // Events dispatch synchronously on the mock runtime's listeners?
        // Give the (same-thread) dispatch a tick to land.
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        let (status, body) = req(&app, "GET", &format!("/v1/jobs/{rid}"), authed(&app), None).await;
        assert_eq!(status, StatusCode::OK, "status: {body}");
        assert_eq!(body["stage"], "transcribing");

        // Terminal state, then SSE payload must NOT carry error text.
        app._app
            .emit(
                "pipeline-progress",
                serde_json::json!({"recording_id": rid, "stage": "failed", "error": "PHI-ADJACENT diagnostic"}),
            )
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let (status, body) = req(&app, "GET", &format!("/v1/jobs/{rid}"), authed(&app), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["stage"], "failed");
        // GET (authenticated) carries the error; the registry stores it.
        assert_eq!(body["error"], "PHI-ADJACENT diagnostic");
    }

    #[tokio::test]
    async fn generation_event_maps_started_to_generating_doc() {
        let app = test_app().await;
        let rid = uuid::Uuid::new_v4().to_string();
        app._app
            .emit(
                "generation-progress",
                serde_json::json!({
                    "type": "synopsis",
                    "status": "started",
                    "recording_id": rid,
                    "progress": null
                }),
            )
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let (status, body) = req(&app, "GET", &format!("/v1/jobs/{rid}"), authed(&app), None).await;
        assert_eq!(status, StatusCode::OK, "status: {body}");
        assert_eq!(body["stage"], "generating_synopsis");
    }

    /// The registry consumes the EMITTERS' own structs (shared since the
    /// scaffold-dedup branch) — this pins that serializing what the
    /// pipeline/generation commands actually emit still deserializes and
    /// drives the registry. The hand-written-JSON tests above pin the raw
    /// wire shape for frontend compat; this one pins Rust-side compat, so
    /// an emitter field rename fails HERE instead of silently breaking
    /// mobile job staging in production.
    #[tokio::test]
    async fn emitter_struct_serializations_drive_the_registry() {
        let app = test_app().await;
        let rid = uuid::Uuid::new_v4().to_string();

        // Emit the structs themselves — exactly what the emitters do
        // (Tauri serializes the payload; payload() on the listener side is
        // the object JSON).
        app._app
            .emit(
                "pipeline-progress",
                &crate::commands::pipeline::PipelineProgress {
                    recording_id: rid.clone(),
                    stage: crate::job_stages::GENERATING_SOAP.to_string(),
                    error: None,
                },
            )
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let (status, body) = req(&app, "GET", &format!("/v1/jobs/{rid}"), authed(&app), None).await;
        assert_eq!(status, StatusCode::OK, "status: {body}");
        assert_eq!(body["stage"], "generating_soap");

        // progress: None is OMITTED from the JSON (skip_serializing_if) —
        // the mobile consumer must tolerate its absence (serde default).
        let generation_payload =
            serde_json::to_string(&crate::commands::generation::GenerationProgress {
                doc_type: "peer_discussion".into(),
                status: crate::job_stages::COMPLETED.into(),
                recording_id: rid.clone(),
                progress: None,
            })
            .expect("serialize generation progress");
        assert!(
            !generation_payload.contains("progress"),
            "None stats must be omitted: {generation_payload}"
        );
        app._app
            .emit(
                "generation-progress",
                &crate::commands::generation::GenerationProgress {
                    doc_type: "peer_discussion".into(),
                    status: crate::job_stages::COMPLETED.into(),
                    recording_id: rid.clone(),
                    progress: None,
                },
            )
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let (status, body) = req(&app, "GET", &format!("/v1/jobs/{rid}"), authed(&app), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["stage"], "completed");
    }

    /// The PUT cap is per-doc-type and 500_000 across the board. Synopsis
    /// specifically: it persists via the metadata patch path, so it must
    /// NOT be routed through the desktop column-cap fallback (50_000) — a
    /// 2026-09-07 regression did exactly that and would have rejected any
    /// legitimately long synopsis re-save from a mobile device.
    #[tokio::test]
    async fn document_put_caps_per_doc_type() {
        let app = test_app().await;
        let rid = seed_recording(&app, Some("Patient reports headache."));

        // 60_000-char synopsis — over the 50_000 fallback, under the cap.
        let long_synopsis = "x".repeat(60_000);
        let (status, body) = req(
            &app,
            "PUT",
            &format!("/v1/recordings/{rid}/documents/synopsis"),
            authed(&app),
            Some(json!({ "content": long_synopsis })),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT, "synopsis 60k: {body}");

        // 500_001-char soap — one over the cap, 413.
        let over = "y".repeat(500_001);
        let (status, body) = req(
            &app,
            "PUT",
            &format!("/v1/recordings/{rid}/documents/soap"),
            authed(&app),
            Some(json!({ "content": over.clone() })),
        )
        .await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "soap 500001: {body}");

        // 500_001-char synopsis — also capped at the blanket document cap.
        let (status, body) = req(
            &app,
            "PUT",
            &format!("/v1/recordings/{rid}/documents/synopsis"),
            authed(&app),
            Some(json!({ "content": over })),
        )
        .await;
        assert_eq!(
            status,
            StatusCode::PAYLOAD_TOO_LARGE,
            "synopsis 500001: {body}"
        );
    }

    #[tokio::test]
    async fn document_get_put_roundtrip_and_revision() {
        let app = test_app().await;
        let rid = seed_recording(&app, Some("Patient reports headache."));
        let uri = format!("/v1/recordings/{rid}/documents/soap_note_field_alias");
        let _ = uri; // placeholder to keep assertions explicit below
        let uri = format!("/v1/recordings/{rid}/documents/soap");
        let (status, body) = req(&app, "GET", &uri, authed(&app), None).await;
        assert_eq!(status, StatusCode::OK, "get: {body}");
        assert_eq!(body["doc_type"], "soap");
        assert_eq!(
            body["content"],
            serde_json::Value::Null,
            "not generated yet"
        );

        let (status, _) = req(
            &app,
            "PUT",
            &uri,
            authed(&app),
            Some(json!({"content": "S: edited on phone"})),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        // Read back + per-field sync revision present.
        let (status, body) = req(&app, "GET", &uri, authed(&app), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["content"], "S: edited on phone");
        assert!(
            body["updated_at"].is_string(),
            "field revision or row stamp must be set: {body}"
        );

        // Invalid doc type → 400; unknown recording → 404.
        let (status, _) = req(
            &app,
            "GET",
            &format!("/v1/recordings/{rid}/documents/nonsense"),
            authed(&app),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let ghost = uuid::Uuid::new_v4().to_string();
        let (status, _) = req(
            &app,
            "GET",
            &format!("/v1/recordings/{ghost}/documents/soap"),
            authed(&app),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn synopsis_document_roundtrip_via_metadata() {
        let app = test_app().await;
        let rid = seed_recording(&app, Some("Patient reports headache."));

        let uri = format!("/v1/recordings/{rid}/documents/synopsis");
        let (status, _) = req(
            &app,
            "PUT",
            &uri,
            authed(&app),
            Some(json!({"content": "Brief: tension headache, follow up 2 weeks."})),
        )
        .await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        let (status, body) = req(&app, "GET", &uri, authed(&app), None).await;
        assert_eq!(status, StatusCode::OK, "get: {body}");
        assert_eq!(
            body["content"],
            "Brief: tension headache, follow up 2 weeks."
        );
    }

    #[tokio::test]
    async fn export_returns_pdf_and_docx_bytes() {
        let app = test_app().await;
        let rid = seed_recording(&app, Some("Patient reports headache."));
        // Seed a SOAP note for export.
        {
            let uuid = uuid::Uuid::parse_str(&rid).unwrap();
            let conn = app.db.conn().expect("conn");
            let mut rec =
                medical_db::recordings::RecordingsRepo::get_by_id(&conn, &uuid).expect("recording");
            rec.soap_note = Some("S: Headache\nA: Tension\nP: Follow up".to_string());
            medical_db::recordings::RecordingsRepo::update(&conn, &rec).expect("update");
        }

        let pdf = req_bytes(
            &app,
            &format!("/v1/recordings/{rid}/export?format=pdf&doc_type=soap"),
            authed(&app),
        )
        .await;
        assert_eq!(pdf.0, StatusCode::OK);
        assert!(pdf.1.starts_with(b"%PDF-"), "PDF magic");
        assert!(pdf.1.len() > 500, "non-trivial PDF");

        let docx = req_bytes(
            &app,
            &format!("/v1/recordings/{rid}/export?format=docx&doc_type=soap"),
            authed(&app),
        )
        .await;
        assert_eq!(docx.0, StatusCode::OK);
        assert!(docx.1.starts_with(&[0x50, 0x4B]), "DOCX/ZIP magic");

        // Synopsis export via metadata.
        {
            let uuid = uuid::Uuid::parse_str(&rid).unwrap();
            let conn = app.db.conn().expect("conn");
            let mut rec =
                medical_db::recordings::RecordingsRepo::get_by_id(&conn, &uuid).expect("recording");
            rec.metadata = serde_json::json!({"synopsis": "Brief synopsis text."});
            medical_db::recordings::RecordingsRepo::update(&conn, &rec).expect("update");
        }
        let syn = req_bytes(
            &app,
            &format!("/v1/recordings/{rid}/export?format=pdf&doc_type=synopsis"),
            authed(&app),
        )
        .await;
        assert_eq!(syn.0, StatusCode::OK, "synopsis export: status");
        assert!(syn.1.starts_with(b"%PDF-"));

        // Missing content → 404, missing recording → 404, bad format → 400.
        let peer = req_bytes(
            &app,
            &format!("/v1/recordings/{rid}/export?format=pdf&doc_type=peer_discussion"),
            authed(&app),
        )
        .await;
        assert_eq!(peer.0, StatusCode::NOT_FOUND);
        let ghost = uuid::Uuid::new_v4().to_string();
        let (status, _) = req(
            &app,
            "GET",
            &format!("/v1/recordings/{ghost}/export?format=pdf&doc_type=soap"),
            authed(&app),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) = req(
            &app,
            "GET",
            &format!("/v1/recordings/{rid}/export?format=rtf&doc_type=soap"),
            authed(&app),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn tombstoned_recordings_are_invisible_to_documents_and_export() {
        // Soft-delete leaks PHI if get_by_id (no deleted_at projection) is
        // used unchecked: documents readable, exports render, PUT silently
        // resurrects the tombstoned row. All three must 404 instead.
        let app = test_app().await;
        let rid = seed_recording(&app, Some("Patient reports headache."));
        let uuid = uuid::Uuid::parse_str(&rid).unwrap();

        // Seed a SOAP note, then tombstone the row.
        {
            let conn = app.db.conn().expect("conn");
            let mut rec =
                medical_db::recordings::RecordingsRepo::get_by_id(&conn, &uuid).expect("recording");
            rec.soap_note = Some("S: Headache".to_string());
            medical_db::recordings::RecordingsRepo::update(&conn, &rec).expect("update");
            medical_db::recordings::RecordingsRepo::soft_delete(&conn, &uuid).expect("tombstone");
        }

        // GET document → 404 (not the seeded SOAP content).
        let (status, body) = req(
            &app,
            "GET",
            &format!("/v1/recordings/{rid}/documents/soap"),
            authed(&app),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "get: {body}");

        // PUT document → 404; the tombstoned row must gain nothing.
        let (status, _) = req(
            &app,
            "PUT",
            &format!("/v1/recordings/{rid}/documents/soap"),
            authed(&app),
            Some(json!({"content": "resurrection attempt"})),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        {
            let conn = app.db.conn().expect("conn");
            let rec = medical_db::recordings::RecordingsRepo::get_by_id(&conn, &uuid)
                .expect("row still exists (tombstoned)");
            assert_ne!(rec.soap_note, Some("resurrection attempt".to_string()));
        }

        // Export → 404 (content exists on the tombstoned row but must not
        // render for a paired device).
        let (status, _) = req(
            &app,
            "GET",
            &format!("/v1/recordings/{rid}/export?format=pdf&doc_type=soap"),
            authed(&app),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn devices_self_revoke_invalidates_bearer() {
        let app = test_app().await;
        // Sanity: token works.
        let (status, _) = req(&app, "GET", "/v1/vocabulary", authed(&app), None).await;
        assert_eq!(status, StatusCode::OK);

        let (status, _) = req(&app, "POST", "/v1/devices/self", authed(&app), None).await;
        assert_eq!(status, StatusCode::NO_CONTENT);

        // The same token now 401s everywhere (revocation is the boundary).
        let (status, _) = req(&app, "GET", "/v1/vocabulary", authed(&app), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn mobile_routes_require_auth() {
        // (method, uri) pairs — POST-only routes must be probed with POST;
        // axum answers 405 (not 401) for a wrong method before any handler.
        for (method, uri, body) in [
            ("POST", "/v1/recordings", Some(json!({"filename": "x.wav"}))),
            ("GET", "/v1/jobs/00000000-0000-0000-0000-000000000000", None),
            (
                "GET",
                "/v1/jobs/00000000-0000-0000-0000-000000000000/events",
                None,
            ),
            (
                "GET",
                "/v1/recordings/00000000-0000-0000-0000-000000000000/documents/soap",
                None,
            ),
            (
                "GET",
                "/v1/recordings/00000000-0000-0000-0000-000000000000/export?format=pdf&doc_type=soap",
                None,
            ),
            ("POST", "/v1/devices/self", None),
        ] {
            let app = test_app().await;
            let (status, _) = req(&app, method, uri, None, body).await;
            assert_eq!(status, StatusCode::UNAUTHORIZED, "no bearer on {uri}");
        }
    }
}

/// Direct tests for `validate_generate_request` — the runtime-generic
/// validation core of the Wry-only generate route. The HTTP route itself is
/// merged only on Wry (the generation commands are `AppHandle<Wry>`-typed),
/// so these exercise the core against the same `ApiState` the router uses:
/// doc-type parse, peer-discussion required fields, recording visibility
/// (tombstones are 404), and soap's audio-presence 409.
mod generate_validation_tests {
    use super::super::mobile::{DocType, GenerateRequest, validate_generate_request};
    use super::*;
    use medical_core::types::recording::Recording;
    use medical_db::recordings::RecordingsRepo;
    use uuid::Uuid;

    async fn validate(
        app: &TestApp,
        id: &str,
        doc_type: &str,
        req: &GenerateRequest,
    ) -> Result<DocType, StatusCode> {
        validate_generate_request(&app.state, id, doc_type, req).await
    }

    /// Seed a recording whose `audio_path` is `audio` (None = a path that
    /// does not exist on disk — the no-audio case).
    fn seed_with_audio(app: &TestApp, audio: Option<std::path::PathBuf>) -> String {
        let conn = app.db.conn().expect("conn");
        let path = audio.unwrap_or_else(|| app._tmp.path().join("no-such-audio.wav"));
        let rec = Recording::new("consult.wav", path);
        RecordingsRepo::insert(&conn, &rec).expect("seed insert");
        rec.id.to_string()
    }

    fn peer_req() -> GenerateRequest {
        GenerateRequest {
            physician_name: Some("Dr. Smith".into()),
            specialty: Some("Cardiology".into()),
            reason: Some("chest pain review".into()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn unknown_doc_type_and_malformed_uuid_are_400() {
        let app = test_app().await;
        let id = seed_with_audio(&app, None);
        let err = validate(&app, &id, "nonsense", &GenerateRequest::default())
            .await
            .expect_err("unknown doc type");
        assert_eq!(err, StatusCode::BAD_REQUEST);

        let err = validate(&app, "not-a-uuid", "soap", &GenerateRequest::default())
            .await
            .expect_err("malformed uuid");
        assert_eq!(err, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn peer_discussion_requires_all_three_fields_nonblank() {
        let app = test_app().await;
        let id = seed_with_audio(&app, None);

        // Sanity: the complete request passes.
        let doc = validate(&app, &id, "peer_discussion", &peer_req())
            .await
            .expect("complete peer request is valid");
        assert_eq!(doc, DocType::PeerDiscussion);

        for blank in [None, Some("   ")] {
            for field in ["physician_name", "specialty", "reason"] {
                let mut req = peer_req();
                match field {
                    "physician_name" => req.physician_name = blank.map(String::from),
                    "specialty" => req.specialty = blank.map(String::from),
                    _ => req.reason = blank.map(String::from),
                }
                let err = validate(&app, &id, "peer_discussion", &req)
                    .await
                    .unwrap_err();
                assert_eq!(
                    err,
                    StatusCode::BAD_REQUEST,
                    "{field} = {blank:?} must reject"
                );
            }
        }
    }

    #[tokio::test]
    async fn unknown_and_tombstoned_recordings_are_404() {
        let app = test_app().await;
        // Well-formed uuid that was never created.
        let missing = Uuid::new_v4().to_string();
        let err = validate(&app, &missing, "synopsis", &GenerateRequest::default())
            .await
            .expect_err("unknown recording");
        assert_eq!(err, StatusCode::NOT_FOUND);

        // Tombstoned rows are invisible (same visibility rule as the
        // document/export handlers). Scoped so the pooled connection is
        // released before `validate` asks the pool for its own.
        let id = seed_with_audio(&app, None);
        {
            let conn = app.db.conn().expect("conn");
            let uuid = Uuid::parse_str(&id).expect("uuid");
            RecordingsRepo::soft_delete(&conn, &uuid).expect("tombstone");
        }
        let err = validate(&app, &id, "synopsis", &GenerateRequest::default())
            .await
            .expect_err("tombstoned recording");
        assert_eq!(err, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn soap_without_an_audio_file_on_disk_is_409() {
        let app = test_app().await;
        let id = seed_with_audio(&app, None);
        let err = validate(&app, &id, "soap", &GenerateRequest::default())
            .await
            .expect_err("no audio");
        // 409, not 404: the recording exists and is visible — the request
        // is unprocessable because process_recording can only fail without
        // audio.
        assert_eq!(err, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn soap_with_audio_passes_and_other_docs_skip_the_audio_check() {
        let tmp = tempfile::tempdir().expect("tmp");
        let audio = tmp.path().join("consult.wav");
        std::fs::write(&audio, b"RIFF").expect("write audio");

        let app = test_app().await;
        let with_audio = seed_with_audio(&app, Some(audio));
        let doc = validate(&app, &with_audio, "soap", &GenerateRequest::default())
            .await
            .expect("audio present");
        assert_eq!(doc, DocType::Soap);

        // Only soap runs the transcribe pipeline — the doc generators work
        // from an existing SOAP note, so they must NOT demand audio.
        let no_audio = seed_with_audio(&app, None);
        let doc = validate(&app, &no_audio, "synopsis", &GenerateRequest::default())
            .await
            .expect("synopsis has no audio requirement");
        assert_eq!(doc, DocType::Synopsis);
    }
}

/// The sharing-stop contract for the mobile event forwarders: listeners
/// registered by `attach_event_forwarders` must stop feeding the registry
/// once `detach_event_forwarders` runs — otherwise every sharing
/// stop→start cycle leaks two listeners that hold the dead registry and
/// fire on every progress event until the app exits.
mod forwarder_lifecycle_tests {
    use super::*;
    use tauri::Emitter as _;
    use uuid::Uuid;

    #[tokio::test]
    async fn forwarders_stop_updating_the_registry_after_detach() {
        let app = tauri::test::mock_app();
        let jobs = Arc::new(super::super::mobile::JobRegistry::new());
        let ids = super::super::mobile::attach_event_forwarders(app.handle(), &jobs);
        let rid = Uuid::new_v4().to_string();

        // Same wire shape the generation commands emit.
        let _ = app.emit(
            "generation-progress",
            json!({"type": "soap", "status": "started", "recording_id": rid, "progress": null}),
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert_eq!(
            jobs.get(&rid).expect("forwarder drove the registry").stage,
            "generating_soap"
        );

        super::super::mobile::detach_event_forwarders(app.handle(), ids);

        let _ = app.emit(
            "generation-progress",
            json!({"type": "soap", "status": "completed", "recording_id": rid, "progress": null}),
        );
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        // The snapshot stays stale — the detached listener must not fire.
        assert_eq!(
            jobs.get(&rid).expect("snapshot still cached").stage,
            "generating_soap",
            "a detached forwarder must not update the registry"
        );
    }
}
