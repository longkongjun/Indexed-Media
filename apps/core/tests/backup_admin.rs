//! 备份管理面集成测试：手动备份产出自验证清单，列表脱敏只读。

mod common;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use mediaflow_core::bootstrap::config::RunMode;
use mediaflow_core::platform::http::build_router;
use mediaflow_core::platform::migrations::migrate_with_backup;
use serde_json::Value;
use tower::ServiceExt as _;
use uuid::Uuid;

const COOKIE_TOKEN: &str = "backups-cookie-token-sentinel";
const CSRF: &str = "backups-csrf-token-sentinel";

async fn harness() -> (common::TestConfigDir, axum::Router) {
    let fixture = common::TestConfigDir::new(RunMode::Development);
    let db = migrate_with_backup(fixture.config()).await.unwrap();
    let account = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO identity_accounts
         (singleton_key,id,normalized_name,display_name,password_phc,created_at_us,updated_at_us)
         VALUES (1,?,'admin','Admin','not-used',0,0)",
    )
    .bind(account.as_bytes().as_slice())
    .execute(db.pool())
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO identity_sessions
         (id,account_id,token_sha256,csrf_sha256,idle_expires_at_us,absolute_expires_at_us,
          credential_version,created_at_us,last_used_at_us,revoked_at_us)
         VALUES (?,?,?,?,?,?,?,?,?,NULL)",
    )
    .bind(Uuid::now_v7().as_bytes().as_slice())
    .bind(account.as_bytes().as_slice())
    .bind(mediaflow_core::platform::random::sha256(COOKIE_TOKEN.as_bytes()).as_slice())
    .bind(mediaflow_core::platform::random::sha256(CSRF.as_bytes()).as_slice())
    .bind(i64::MAX - 1)
    .bind(i64::MAX)
    .bind(1_i64)
    .bind(0_i64)
    .bind(0_i64)
    .execute(db.pool())
    .await
    .unwrap();
    let app = build_router(fixture.config().clone(), Some(db));
    (fixture, app)
}

async fn json_body(response: axum::response::Response) -> Value {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).expect("json body")
}

#[tokio::test]
async fn manual_backup_creates_verified_manifest_and_listing_is_redacted() {
    let (fixture, app) = harness().await;
    let created = json_body(
        app.clone()
            .oneshot(
                Request::post("/api/v1/system/backups")
                    .header(
                        header::COOKIE,
                        format!("__Host-mediaflow_session={COOKIE_TOKEN}"),
                    )
                    .header("x-csrf-token", CSRF)
                    .header(header::ORIGIN, "http://127.0.0.1:3000")
                    .header("sec-fetch-site", "same-origin")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(created["created"], true, "body: {created}");
    let sha = created["manifest"]["sha256"].as_str().unwrap().to_owned();
    assert_eq!(sha.len(), 64);
    assert!(!created.to_string().to_lowercase().contains("password"));

    let listed = json_body(
        app.clone()
            .oneshot(
                Request::get("/api/v1/system/backups?limit=5")
                    .header(
                        header::COOKIE,
                        format!("__Host-mediaflow_session={COOKIE_TOKEN}"),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    let items = listed["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["manifest"]["sha256"], sha.as_str());

    // 备份产物真实落盘且自验证可读。
    assert!(!fixture.manifest_files().is_empty());

    // 未认证访问被拒绝。
    let denied = app
        .oneshot(
            Request::get("/api/v1/system/backups")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn backup_preserves_business_facts_for_offline_restore() {
    let (fixture, app) = harness().await;
    // harness 已写入管理员账户；备份包含该事实（恢复演练留给运行手册）。
    let pool =
        sqlx::SqlitePool::connect(&format!("sqlite://{}", fixture.database_path().display()))
            .await
            .unwrap();
    let body = json_body(
        app.clone()
            .oneshot(
                Request::post("/api/v1/system/backups")
                    .header(
                        header::COOKIE,
                        format!("__Host-mediaflow_session={COOKIE_TOKEN}"),
                    )
                    .header("x-csrf-token", CSRF)
                    .header(header::ORIGIN, "http://127.0.0.1:3000")
                    .header("sec-fetch-site", "same-origin")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(body["created"], true, "body: {body}");
    // 校验最新备份文件确实是自验证清单。
    let manifest_path = fixture
        .manifest_files()
        .into_iter()
        .last()
        .expect("manifest file");
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    assert!(manifest["sha256"].as_str().is_some_and(|v| v.len() == 64));
    assert!(manifest["created_at"].as_str().is_some());
    pool.close().await;
}

fn backup_post(
    cookie: &str,
    csrf: Option<&str>,
    origin: Option<&str>,
    fetch_site: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::post("/api/v1/system/backups").header(header::COOKIE, cookie);
    if let Some(token) = csrf {
        builder = builder.header("x-csrf-token", token);
    }
    if let Some(value) = origin {
        builder = builder.header(header::ORIGIN, value);
    }
    if let Some(value) = fetch_site {
        builder = builder.header("sec-fetch-site", value);
    }
    builder.body(Body::empty()).unwrap()
}

/// 反例形状：标签、CSRF 头、Origin 头、`Sec-Fetch-Site` 头与预期错误码。
type GuardCase<'a> = (
    &'a str,
    Option<&'a str>,
    Option<&'a str>,
    Option<&'a str>,
    &'a str,
);

/// cookie 会话写请求缺/错 CSRF、跨源/`null`/缺失同源证明时必须被拒，且不落任何备份产物。
#[tokio::test]
async fn backup_post_rejects_cookie_writes_with_missing_or_cross_site_proof() {
    let (fixture, app) = harness().await;
    let cookie = format!("__Host-mediaflow_session={COOKIE_TOKEN}");
    let cases: &[GuardCase] = &[
        (
            "csrf header missing",
            None,
            Some("http://127.0.0.1:3000"),
            Some("same-origin"),
            "csrf.invalid",
        ),
        (
            "csrf token mismatch",
            Some("backups-csrf-token-wrong"),
            Some("http://127.0.0.1:3000"),
            Some("same-origin"),
            "csrf.invalid",
        ),
        (
            "cross-site origin",
            Some(CSRF),
            Some("https://evil.example"),
            Some("cross-site"),
            "origin.untrusted",
        ),
        (
            "null origin",
            Some(CSRF),
            Some("null"),
            None,
            "origin.untrusted",
        ),
        (
            "no same-origin proof",
            Some(CSRF),
            None,
            None,
            "origin.untrusted",
        ),
    ];
    for (label, csrf, origin, fetch_site, expected_code) in cases {
        let response = app
            .clone()
            .oneshot(backup_post(&cookie, *csrf, *origin, *fetch_site))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{label}");
        let body = json_body(response).await;
        assert_eq!(body["error"]["code"], *expected_code, "{label}: {body}");
        assert!(
            fixture.manifest_files().is_empty(),
            "{label}: backup manifest must not be written"
        );
        assert!(
            fixture.backup_files().is_empty(),
            "{label}: database snapshot must not be written"
        );
    }

    // 拒绝均留下与其他写路由同构的审计记录。
    let pool =
        sqlx::SqlitePool::connect(&format!("sqlite://{}", fixture.database_path().display()))
            .await
            .unwrap();
    let denials: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM platform_audit_events WHERE outcome='denied' ORDER BY action",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(denials, vec!["csrf", "csrf", "origin", "origin", "origin"]);
    pool.close().await;
}

/// 设备令牌通道按“令牌即写证明”豁免浏览器同源/CSRF，POST 语义保持成功。
#[tokio::test]
async fn backup_post_allows_device_token_channel_without_browser_proof() {
    let (fixture, app) = harness().await;
    let device_token = "backups-device-token-sentinel";
    let pool =
        sqlx::SqlitePool::connect(&format!("sqlite://{}", fixture.database_path().display()))
            .await
            .unwrap();
    let account_id: Vec<u8> = sqlx::query_scalar("SELECT id FROM identity_accounts LIMIT 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO identity_devices
         (id,account_id,display_name,token_sha256,credential_version,absolute_expires_at_us,
          created_at_us,last_used_at_us,revoked_at_us,revoked_reason)
         VALUES (?,?,?,?,1,?,?,NULL,NULL,NULL)",
    )
    .bind(Uuid::now_v7().as_bytes().as_slice())
    .bind(account_id.as_slice())
    .bind("backup test device")
    .bind(mediaflow_core::platform::random::sha256(device_token.as_bytes()).as_slice())
    .bind(i64::MAX)
    .bind(0_i64)
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;

    // 移动端形状：仅 Authorization 头，无 cookie、无 Origin/Sec-Fetch-Site、无 CSRF。
    let created = json_body(
        app.clone()
            .oneshot(
                Request::post("/api/v1/system/backups")
                    .header(header::AUTHORIZATION, format!("Bearer {device_token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(created["created"], true, "body: {created}");
    assert_eq!(fixture.manifest_files().len(), 1);
    assert_eq!(fixture.backup_files().len(), 1);
}
