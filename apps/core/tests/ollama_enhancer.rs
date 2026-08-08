#![allow(clippy::too_many_lines)]

mod downloader_support;

use std::time::Duration;

use downloader_support::{ScriptedHttpServer, ScriptedResponse};
use mediaflow_core::connectors::enhancer::model::{
    BaseIdentityHint, EnhancementInput, EnhancerConfigInput, EnhancerMediaKind,
};
use mediaflow_core::connectors::enhancer::ollama::{OllamaClient, OllamaClientOptions};
use mediaflow_core::connectors::enhancer::port::{EnhancerError, IdentificationEnhancer};

#[test]
fn configuration_accepts_only_local_network_origins_and_bounded_fields() {
    for url in [
        "http://localhost:11434",
        "http://127.0.0.1:11434",
        "http://10.1.2.3:11434",
        "http://172.16.1.2:11434",
        "http://192.168.1.2:11434",
        "http://169.254.1.2:11434",
        "http://[::1]:11434",
        "http://[fd00::1]:11434",
        "http://[fe80::1]:11434",
    ] {
        assert!(config(url, "qwen3:4b", 3_000).validate().is_ok(), "{url}");
    }
    for url in [
        "https://models.example.com",
        "http://8.8.8.8:11434",
        "http://[2001:4860:4860::8888]:11434",
        "http://0.0.0.0:11434",
        "ftp://127.0.0.1/model",
        "http://user@127.0.0.1:11434",
        "http://127.0.0.1:11434?token=secret",
        "http://127.0.0.1:11434/#fragment",
        "http://localhost.evil.test:11434",
    ] {
        assert!(config(url, "qwen3:4b", 3_000).validate().is_err(), "{url}");
    }
    assert!(
        config("http://127.0.0.1:11434", "", 3_000)
            .validate()
            .is_err()
    );
    assert!(
        config("http://127.0.0.1:11434", &"x".repeat(129), 3_000)
            .validate()
            .is_err()
    );
    assert!(
        config("http://127.0.0.1:11434", "qwen3:4b", 99)
            .validate()
            .is_err()
    );
}

#[tokio::test]
async fn probe_and_chat_use_fixed_bounded_protocol_and_return_strict_hints() {
    let hints_json = r#"{\"title\":\"The Show\",\"year\":2024,\"media_kind\":\"episode\",\"season\":1,\"episodes\":[2]}"#;
    let server = ScriptedHttpServer::spawn(vec![
        response(r#"{"models":[{"name":"qwen3:4b","model":"qwen3:4b","modified_at":"2026-07-24T00:00:00Z","size":123,"digest":"abc","details":{"format":"gguf"}}]}"#),
        response(format!(
            r#"{{"model":"qwen3:4b","created_at":"2026-07-24T00:00:00Z","message":{{"role":"assistant","content":"{hints_json}"}},"done":true,"done_reason":"stop","total_duration":1,"load_duration":1,"prompt_eval_count":1,"prompt_eval_duration":1,"eval_count":1,"eval_duration":1}}"#
        )),
    ])
    .await;
    let mut localhost_origin = server.origin.clone();
    localhost_origin.set_host(Some("localhost")).unwrap();
    let validated = config(localhost_origin.as_str(), "qwen3:4b", 2_000)
        .validate()
        .unwrap();
    let client = test_client(64 * 1024, Duration::from_secs(1));

    let probe = client.probe(validated.endpoint()).await.unwrap();
    assert_eq!(probe.adapter_version, "ollama-v1");
    assert!(probe.model_available);
    let input = EnhancementInput::new(
        "The.Show.S01E02.2024.1080p.mkv".to_owned(),
        vec!["The Show".to_owned(), "Season 1".to_owned()],
        BaseIdentityHint {
            normalized_title: "The Show".to_owned(),
            media_kind: EnhancerMediaKind::Episode,
            year: Some(2024),
            season: Some(1),
            episodes: vec![2],
        },
    )
    .unwrap();
    let hints = client.enhance(validated.endpoint(), &input).await.unwrap();
    assert_eq!(hints.title.as_deref(), Some("The Show"));
    assert_eq!(hints.media_kind, Some(EnhancerMediaKind::Episode));
    assert_eq!(hints.season, Some(1));
    assert_eq!(hints.episodes, vec![2]);

    let requests = server.requests();
    assert!(requests[0].starts_with("GET /api/tags HTTP/1.1"));
    assert!(requests[1].starts_with("POST /api/chat HTTP/1.1"));
    assert!(requests[1].contains("\"stream\":false"));
    assert!(requests[1].contains("\"format\""));
    assert!(requests[1].contains("The.Show.S01E02.2024.1080p.mkv"));
    assert!(requests[1].len() < 16 * 1024);
    let debug = format!("{client:?} {input:?} {hints:?}");
    assert!(!debug.contains("The.Show.S01E02"));
    assert!(!debug.contains("The Show"));
}

#[tokio::test]
async fn redirect_status_timeout_size_and_invalid_content_map_to_redacted_failures() {
    let cases = [
        (
            ScriptedResponse::new(
                "302 Found",
                &[("Location", "http://127.0.0.1:9/api/tags")],
                "",
            ),
            EnhancerError::InvalidResponse,
        ),
        (
            ScriptedResponse::new("429 Too Many Requests", &[], ""),
            EnhancerError::Overloaded,
        ),
        (
            ScriptedResponse::new("503 Service Unavailable", &[], "private failure body"),
            EnhancerError::Unavailable,
        ),
    ];
    for (response, expected) in cases {
        let server = ScriptedHttpServer::spawn(vec![response]).await;
        let validated = config(server.origin.as_str(), "qwen3:4b", 1_000)
            .validate()
            .unwrap();
        let error = test_client(64 * 1024, Duration::from_secs(1))
            .probe(validated.endpoint())
            .await
            .unwrap_err();
        assert_eq!(error, expected);
        assert!(!format!("{error:?}").contains("private failure body"));
    }

    let oversized = ScriptedHttpServer::spawn(vec![response(vec![b'x'; 1025])]).await;
    let validated = config(oversized.origin.as_str(), "qwen3:4b", 1_000)
        .validate()
        .unwrap();
    assert_eq!(
        test_client(1024, Duration::from_secs(1))
            .probe(validated.endpoint())
            .await,
        Err(EnhancerError::ResponseTooLarge)
    );
    let stalled = ScriptedHttpServer::spawn(vec![
        response(r#"{"models":[]}"#).delayed(Duration::from_millis(200)),
    ])
    .await;
    let validated = config(stalled.origin.as_str(), "qwen3:4b", 100)
        .validate()
        .unwrap();
    assert_eq!(
        test_client(1024, Duration::from_millis(100))
            .probe(validated.endpoint())
            .await,
        Err(EnhancerError::Timeout)
    );

    for content in [
        r#"{"title":"Movie","provider_id":"tmdb:1"}"#,
        r#"{"title":"Movie","extra":"command"}"#,
        r#"{"title":"Movie","media_kind":"movie","season":1,"episodes":[1]}"#,
    ] {
        let escaped = serde_json::to_string(content).unwrap();
        let server = ScriptedHttpServer::spawn(vec![response(format!(
            r#"{{"model":"qwen3:4b","message":{{"role":"assistant","content":{escaped}}},"done":true}}"#
        ))])
        .await;
        let validated = config(server.origin.as_str(), "qwen3:4b", 1_000)
            .validate()
            .unwrap();
        let input = input();
        assert_eq!(
            test_client(64 * 1024, Duration::from_secs(1))
                .enhance(validated.endpoint(), &input)
                .await,
            Err(EnhancerError::InvalidResponse)
        );
    }
}

#[test]
fn enhancement_input_bounds_basename_parents_and_base_hint_before_http() {
    assert!(
        EnhancementInput::new(
            "x".repeat(513),
            Vec::new(),
            BaseIdentityHint::movie("Movie".to_owned(), Some(2024)),
        )
        .is_err()
    );
    assert!(
        EnhancementInput::new(
            "/private/movie.mkv".to_owned(),
            Vec::new(),
            BaseIdentityHint::movie("Movie".to_owned(), Some(2024)),
        )
        .is_err()
    );
    assert!(
        EnhancementInput::new(
            "movie.mkv".to_owned(),
            vec!["a".to_owned(), "b".to_owned(), "c".to_owned()],
            BaseIdentityHint::movie("Movie".to_owned(), Some(2024)),
        )
        .is_err()
    );
    assert!(
        EnhancementInput::new(
            "movie.mkv".to_owned(),
            vec!["x".repeat(256)],
            BaseIdentityHint::movie("Movie".to_owned(), Some(2024)),
        )
        .is_err()
    );
    assert!(
        EnhancementInput::new(
            "movie.mkv".to_owned(),
            Vec::new(),
            BaseIdentityHint::movie("x".repeat(513), Some(2024)),
        )
        .is_err()
    );
}

fn input() -> EnhancementInput {
    EnhancementInput::new(
        "Movie.2024.mkv".to_owned(),
        vec!["Movies".to_owned()],
        BaseIdentityHint::movie("Movie".to_owned(), Some(2024)),
    )
    .unwrap()
}

fn config(base_url: &str, model: &str, timeout_ms: u32) -> EnhancerConfigInput {
    EnhancerConfigInput {
        enabled: true,
        base_url: base_url.to_owned(),
        model: model.to_owned(),
        timeout_ms,
    }
}

fn test_client(max_response_bytes: usize, request_timeout: Duration) -> OllamaClient {
    OllamaClient::new(OllamaClientOptions {
        connect_timeout: Duration::from_millis(100),
        request_timeout,
        max_response_bytes,
        max_concurrency: 2,
    })
    .unwrap()
}

fn response(body: impl AsRef<[u8]>) -> ScriptedResponse {
    ScriptedResponse::new("200 OK", &[("Content-Type", "application/json")], body)
}
