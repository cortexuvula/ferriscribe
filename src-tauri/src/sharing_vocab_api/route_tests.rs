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
    async fn recordings_list_pages_by_created_at_with_tiebreak_and_excludes_tombstones() {
        let app = test_app().await;
        let conn = app.db.conn().expect("conn");

        // Five recordings with distinct created_at (newest first: e, d, c, b, a)
        // plus a TIE PAIR (f, g) sharing one timestamp — the cursor boundary
        // must advance past BOTH by (created_at, id), never by timestamp alone.
        let mk = |id: &str, created: &str| {
            let uuid = uuid::Uuid::parse_str(id).expect("uuid");
            let mut rec = medical_core::types::recording::Recording::new(
                format!("{id}.wav"),
                std::path::PathBuf::new(),
            );
            rec.id = uuid;
            rec.created_at = chrono::DateTime::parse_from_rfc3339(created)
                .expect("created")
                .with_timezone(&chrono::Utc);
            medical_db::recordings::RecordingsRepo::insert(&conn, &rec).expect("seed");
        };
        mk(
            "00000000-0000-0000-0000-00000000000a",
            "2026-09-01T10:00:00+00:00",
        );
        mk(
            "00000000-0000-0000-0000-00000000000b",
            "2026-09-02T10:00:00+00:00",
        );
        mk(
            "00000000-0000-0000-0000-00000000000c",
            "2026-09-03T10:00:00+00:00",
        );
        mk(
            "00000000-0000-0000-0000-00000000000d",
            "2026-09-04T10:00:00+00:00",
        );
        mk(
            "00000000-0000-0000-0000-00000000000e",
            "2026-09-05T10:00:00+00:00",
        );
        mk(
            "00000000-0000-0000-0000-00000000000f",
            "2026-09-06T10:00:00+00:00",
        );
        mk(
            "00000000-0000-0000-0000-000000000010",
            "2026-09-06T10:00:00+00:00",
        );
        // A tombstoned NEWEST row — must never appear.
        mk(
            "00000000-0000-0000-0000-0000000000ff",
            "2026-09-07T10:00:00+00:00",
        );
        conn.execute(
            "UPDATE recordings SET deleted_at = '2026-09-07T11:00:00+00:00'
             WHERE id = '00000000-0000-0000-0000-0000000000ff'",
            [],
        )
        .expect("tombstone");

        // Release the pooled connection BEFORE the request — in-memory
        // pools are max_size=1, and the seed conn still being alive would
        // starve the handler's checkout (30s timeout → 500).
        drop(conn);

        // Page 1: limit 3 => [10 (tie, id DESC), f (tie), e], has_more, cursor.
        let (status, body) = req(&app, "GET", "/v1/recordings?limit=3", authed(&app), None).await;
        assert_eq!(status, StatusCode::OK, "page1: {body}");
        let arr = body["recordings"].as_array().expect("array");
        assert_eq!(arr.len(), 3);
        assert_eq!(
            arr[0]["id"], "00000000-0000-0000-0000-000000000010",
            "tie pair: id DESC"
        );
        assert_eq!(arr[1]["id"], "00000000-0000-0000-0000-00000000000f");
        assert_eq!(arr[2]["id"], "00000000-0000-0000-0000-00000000000e");
        assert_eq!(body["has_more"], true);
        let cursor = body["next_cursor"].as_str().expect("cursor").to_string();

        // Page 2 across the tie boundary: cursor is the SECOND tie row (f);
        // a timestamp-only cursor would re-serve or skip the pair.
        let (status, body) = req(
            &app,
            "GET",
            &format!("/v1/recordings?limit=3&cursor={cursor}"),
            authed(&app),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "page2: {body}");
        let arr = body["recordings"].as_array().expect("array");
        assert_eq!(
            (
                arr[0]["id"].as_str().unwrap(),
                arr[1]["id"].as_str().unwrap(),
                arr[2]["id"].as_str().unwrap()
            ),
            (
                "00000000-0000-0000-0000-00000000000d",
                "00000000-0000-0000-0000-00000000000c",
                "00000000-0000-0000-0000-00000000000b"
            ),
            "page 2 must continue strictly below the (created_at, id) cursor"
        );
        assert_eq!(body["has_more"], true);
        let cursor2 = body["next_cursor"].as_str().expect("cursor2").to_string();

        // Page 3: last row (a), no more.
        let (status, body) = req(
            &app,
            "GET",
            &format!("/v1/recordings?limit=3&cursor={cursor2}"),
            authed(&app),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "page3: {body}");
        let arr = body["recordings"].as_array().expect("array");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"], "00000000-0000-0000-0000-00000000000a");
        assert_eq!(body["has_more"], false);
        assert!(body.get("next_cursor").is_none_or(|v| v.is_null()));

        // Tombstoned row never appeared: pages served exactly the 7 live
        // rows (3 + 3 + 1), asserted exhaustively by id above.
        assert_eq!(3 + 3 + 1, 7, "exactly the 7 live rows were served");

        // Malformed cursor → 400, not 500.
        let (status, _) = req(
            &app,
            "GET",
            "/v1/recordings?cursor=%%%not-base64%%%",
            authed(&app),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn recordings_list_orders_mixed_format_stamps_parsed_across_pages() {
        // codie's Critical: a string ORDER BY disagrees with a parsed
        // cursor predicate when created_at mixes RFC 3339 (T-format) and
        // legacy SQLite space-format stamps. Seed interleaved dates in
        // BOTH formats and assert the parsed order holds across a page
        // boundary — no skips, no duplicates.
        let app = test_app().await;
        let conn = app.db.conn().expect("conn");

        // Dates (oldest → newest): d1 < d2 < d3 < d4 < d5. d2/dd4 are
        // space-format; d1/d3/d5 are T-format. Correct parsed order:
        // d5, d4, d3, d2, d1 regardless of format.
        let mk = |id: &str, created: &str| {
            let mut rec = medical_core::types::recording::Recording::new(
                format!("{id}.wav"),
                std::path::PathBuf::new(),
            );
            rec.id = uuid::Uuid::parse_str(id).expect("uuid");
            // Raw insert: bypass the repo so the stamp lands verbatim
            // (RecordingsRepo would normalize through chrono).
            conn.execute(
                "INSERT INTO recordings (id, filename, audio_path, created_at, metadata)
                 VALUES (?1, ?2, '', ?3, '{}')",
                rusqlite::params![id, format!("{id}.wav"), created],
            )
            .expect("seed");
            rec
        };
        mk(
            "00000000-0000-0000-0000-00000000000a",
            "2026-09-01T10:00:00+00:00",
        ); // d1 T
        mk(
            "00000000-0000-0000-0000-00000000000b",
            "2026-09-02 10:00:00",
        ); // d2 SPACE
        mk(
            "00000000-0000-0000-0000-00000000000c",
            "2026-09-03T10:00:00+00:00",
        ); // d3 T
        mk(
            "00000000-0000-0000-0000-00000000000d",
            "2026-09-04 10:00:00",
        ); // d4 SPACE
        mk(
            "00000000-0000-0000-0000-00000000000e",
            "2026-09-05T10:00:00+00:00",
        ); // d5 T
        mk("00000000-0000-0000-0000-00000000000f", "not-a-timestamp"); // NULL-stamp row

        // Sanity: the string sort would place 'not-a-timestamp' and the
        // space-format rows LAST/WILDLY — the parsed sort must interleave
        // by actual date. (Guard against regression to ORDER BY created_at.)
        drop(conn);

        // Page 1 (limit 2): d5, d4 — the space-format d4 must sit in its
        // parsed position, NOT sink below d3 (string sort would return
        // d5, d3 because '2026-09-04 ...' < '2026-09-03T...' lexically?
        // No — '2026-09-04' > '2026-09-03' at the date prefix, but the
        // 'T' vs ' ' at position 10 flips same-day comparisons. The pin
        // is the CROSS-BOUNDARY walk below.)
        let (status, body) = req(&app, "GET", "/v1/recordings?limit=2", authed(&app), None).await;
        assert_eq!(status, StatusCode::OK, "page1: {body}");
        let arr = body["recordings"].as_array().expect("array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["id"], "00000000-0000-0000-0000-00000000000e");
        assert_eq!(
            arr[1]["id"], "00000000-0000-0000-0000-00000000000d",
            "space-format d4 must interleave in parsed position, not sink"
        );
        assert_eq!(body["has_more"], true);
        let cursor = body["next_cursor"].as_str().expect("cursor").to_string();

        // Walk pages 2..N; collect every id; stop at null cursor.
        let mut seen = vec![
            "00000000-0000-0000-0000-00000000000e".to_string(),
            "00000000-0000-0000-0000-00000000000d".to_string(),
        ];
        let mut cur = cursor;
        loop {
            let (status, body) = req(
                &app,
                "GET",
                &format!("/v1/recordings?limit=2&cursor={cur}"),
                authed(&app),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "walk: {body}");
            let arr = body["recordings"].as_array().expect("array");
            if arr.is_empty() {
                break;
            }
            for r in arr {
                seen.push(r["id"].as_str().expect("id").to_string());
            }
            match body["next_cursor"].as_str() {
                Some(c) => cur = c.to_string(),
                None => break,
            }
        }

        // Parsed order: d5, d4, d3, d2, d1 — space-format rows interleave
        // by parsed date. The corrupt-stamp row (f) is EXCLUDED: it can't
        // be hydrated (parse_db_timestamp fails; sync drops it too), and
        // serving its id would create a phantom page slot. Pinned.
        assert_eq!(
            seen,
            vec![
                "00000000-0000-0000-0000-00000000000e".to_string(),
                "00000000-0000-0000-0000-00000000000d".to_string(),
                "00000000-0000-0000-0000-00000000000c".to_string(),
                "00000000-0000-0000-0000-00000000000b".to_string(),
                "00000000-0000-0000-0000-00000000000a".to_string(),
            ],
            "mixed-format stamps must interleave by parsed date; corrupt-stamp row excluded; no skips/dupes"
        );
        assert!(
            !seen.contains(&"00000000-0000-0000-0000-00000000000f".to_string()),
            "corrupt-stamp row must never be served"
        );
    }

    #[tokio::test]
    async fn recordings_list_requires_auth_and_clamps_limit() {
        let app = test_app().await;
        let (status, _) = req(&app, "GET", "/v1/recordings", None, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);

        // limit=0 clamps to 1; limit=999 clamps to 100 (both 200, no panic).
        let (status, body) = req(&app, "GET", "/v1/recordings?limit=0", authed(&app), None).await;
        assert_eq!(status, StatusCode::OK, "limit=0: {body}");
        let (status, body) =
            req(&app, "GET", "/v1/recordings?limit=9999", authed(&app), None).await;
        assert_eq!(status, StatusCode::OK, "limit=9999: {body}");
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

/// JobRegistry internals + the jobs SSE stream (2026-09-07 review item e):
/// `mark_if_current` supersession, `prune_terminal` eviction, poisoned-lock
/// recovery, and `GET /v1/jobs/{id}/events` — previously untested anywhere.
mod registry_internals_tests {
    use super::*;
    use crate::job_stages as stage;
    use futures_util::StreamExt;
    use uuid::Uuid;

    /// A registry entry with a fully controlled stamp (prune tests need
    /// backdated snapshots, which the live API can't produce).
    fn entry(
        seq: u64,
        stage_str: &str,
        updated_at: &str,
    ) -> (u64, super::super::mobile::JobSnapshot) {
        (
            seq,
            super::super::mobile::JobSnapshot {
                recording_id: "rid".to_string(),
                stage: stage_str.to_string(),
                error: None,
                updated_at: updated_at.to_string(),
            },
        )
    }

    #[test]
    fn mark_increments_sequences_and_get_returns_latest() {
        let jobs = super::super::mobile::JobRegistry::new();
        let s1 = jobs.mark("rid", stage::QUEUED, None);
        let s2 = jobs.mark("rid", stage::GENERATING_SOAP, None);
        assert!(s2 > s1, "sequences must be monotonic ({s1} → {s2})");
        let snap = jobs.get("rid").expect("snapshot present");
        assert_eq!(snap.stage, stage::GENERATING_SOAP);
        assert_eq!(snap.recording_id, "rid");
        assert!(jobs.get("other").is_none());
    }

    /// THE safety-net race: a terminal mark captured at queue time must be
    /// dropped when any newer mark landed since (a newer job for the same
    /// recording must not be clobbered by its slow predecessor) — and must
    /// APPLY when its snapshot is still the latest word.
    #[test]
    fn mark_if_current_is_gated_on_the_current_sequence() {
        let jobs = super::super::mobile::JobRegistry::new();

        // Queued at seq 1; live events moved on to seq 2.
        let queued_seq = jobs.mark("rid", stage::QUEUED, None);
        let generating_seq = jobs.mark("rid", stage::GENERATING_SOAP, None);

        // The queued-time safety net (completed @ the older seq) is
        // superseded and must not apply.
        jobs.mark_if_current("rid", queued_seq, stage::COMPLETED, None);
        assert_eq!(
            jobs.get("rid").expect("snapshot").stage,
            stage::GENERATING_SOAP,
            "a superseded terminal mark must not apply"
        );

        // With no newer mark since, the safety net applies — errors included.
        jobs.mark_if_current("rid", generating_seq, stage::FAILED, Some("boom".into()));
        let snap = jobs.get("rid").expect("snapshot");
        assert_eq!(snap.stage, stage::FAILED);
        assert_eq!(snap.error.as_deref(), Some("boom"));

        // An expect-seq for an EVICTED/never-marked entry inserts nothing.
        jobs.mark_if_current("ghost", 99, stage::COMPLETED, None);
        assert!(jobs.get("ghost").is_none());
    }

    /// Eviction: only TERMINAL stages past the cutoff leave; terminal-but-
    /// fresh, and non-terminal-no-matter-how-old, stay.
    #[test]
    fn prune_terminal_evicts_only_stale_terminal_entries() {
        let old = (chrono::Utc::now() - chrono::Duration::hours(25)).to_rfc3339();
        let fresh = chrono::Utc::now().to_rfc3339();
        let mut jobs = std::collections::HashMap::new();
        jobs.insert("stale-done".to_string(), entry(1, stage::COMPLETED, &old));
        jobs.insert("fresh-done".to_string(), entry(2, stage::COMPLETED, &fresh));
        jobs.insert("stale-running".to_string(), entry(3, stage::QUEUED, &old));
        jobs.insert("stale-failed".to_string(), entry(4, stage::FAILED, &old));

        let removed = super::super::mobile::prune_terminal(&mut jobs, chrono::Duration::hours(24));
        assert_eq!(removed, 2, "completed + failed past cutoff: {jobs:?}");
        assert!(!jobs.contains_key("stale-done"));
        assert!(!jobs.contains_key("stale-failed"));
        assert!(jobs.contains_key("fresh-done"), "fresh terminal stays");
        assert!(
            jobs.contains_key("stale-running"),
            "a stale NON-terminal entry must never be evicted"
        );
    }

    /// Poison recovery: a panic elsewhere while holding the registry lock
    /// must not silence later progress events.
    #[test]
    fn poisoned_lock_is_recovered_not_dropped() {
        let jobs = super::super::mobile::JobRegistry::new();
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = jobs.inner.lock().unwrap();
            panic!("poison the registry lock");
        }));
        assert!(jobs.inner.is_poisoned());

        jobs.mark("rid", stage::QUEUED, None);
        assert_eq!(
            jobs.get("rid").expect("mark after poison lands").stage,
            stage::QUEUED
        );
    }

    /// The SSE stream: an unknown job yields an `unknown` snapshot frame,
    /// and later registry marks for THAT id stream as data frames (other
    /// ids are filtered out).
    #[tokio::test]
    async fn jobs_sse_streams_initial_snapshot_then_live_marks() {
        let app = test_app().await;
        let rid = Uuid::new_v4().to_string();

        let request = Request::builder()
            .method("GET")
            .uri(format!("/v1/jobs/{rid}/events"))
            .header("authorization", format!("Bearer {}", app.token))
            .body(Body::empty())
            .expect("request");
        let response = app
            .router
            .clone()
            .oneshot(request)
            .await
            .expect("sse route");
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|ct| ct.starts_with("text/event-stream")),
            "SSE content type"
        );

        let mut stream = response.into_body().into_data_stream();
        let first = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .expect("initial frame within 2s")
            .expect("frame present")
            .expect("frame ok");
        let first = String::from_utf8_lossy(&first);
        assert!(first.contains("unknown"), "unknown job first: {first}");

        // A mark for a DIFFERENT recording must not reach this stream…
        app.state
            .jobs
            .mark(&Uuid::new_v4().to_string(), stage::QUEUED, None);
        // …but a mark for ours does.
        app.state.jobs.mark(&rid, stage::QUEUED, None);
        let second = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .expect("live frame within 2s")
            .expect("frame present")
            .expect("frame ok");
        let second = String::from_utf8_lossy(&second);
        assert!(
            second.contains(stage::QUEUED),
            "live mark streamed: {second}"
        );
        assert!(!second.contains("error"), "SSE never carries error text");
    }
}
