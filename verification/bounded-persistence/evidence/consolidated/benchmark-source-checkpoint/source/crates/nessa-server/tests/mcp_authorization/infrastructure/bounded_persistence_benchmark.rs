//! Manual composed OAuth benchmark over the production library; no real network.
use nessa_server::mcp_authorization::application::{
    AuthorizationOwner, AuthorizeAnswer, CallbackQuery, OAuthResponse,
};
use nessa_server::mcp_authorization::infrastructure::{
    FileAuthorizationAudit, FileRecords, MemoryAuthorization, ScriptedCallback,
};
use std::{io::Write, sync::Arc, time::Instant};
use uuid::Uuid;

fn contains_token(directory: &std::path::Path, token: &str) -> bool {
    let mut pending = vec![directory.to_path_buf()];
    while let Some(path) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            if bytes
                .windows(token.len())
                .any(|window| window == token.as_bytes())
            {
                return true;
            }
        }
    }
    false
}

#[test]
#[ignore = "manual local composed OAuth benchmark; scripted HTTP/callbacks, raw CSV"]
fn benchmark_scripted_authorize_callback_refresh() {
    let output =
        std::env::var("NESSA_627_BENCH_OUT").unwrap_or_else(|_| "/tmp/627-oauth-worker.csv".into());
    let samples: usize = std::env::var("NESSA_627_BENCH_SAMPLES")
        .ok()
        .map(|value| value.parse().unwrap())
        .unwrap_or(1000);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(8)
        .enable_time()
        .build()
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let rows = runtime.block_on(async {
        let mut lanes = Vec::new();
        for lane in 0..4 {
            let path = directory.path().join(format!("lane-{lane}"));
            let records = Arc::new(FileRecords::new(path.join("records")));
            let audit = Arc::new(FileAuthorizationAudit::new(path.join("audit.jsonl")));
            let memory = Arc::new(MemoryAuthorization::new());
            let owner = Arc::new(AuthorizationOwner::new(records, audit, memory.clone(), Arc::new(ScriptedCallback::pending()), memory.clone(), memory.clone(), memory.clone(), memory.clone(), true));
            lanes.push(tokio::spawn(async move {
                let mut rows = Vec::new();
                // Ten discarded complete workflows warm each independent lane.
                for sample in 0..(samples / 4 + 10) {
                    let server = Uuid::new_v4();
                    memory.set_resource(server, "https://mcp.example/mcp").await;
                    let response = |status, body: &str| OAuthResponse { status, body: body.into(), www_authenticate: None };
                    memory.push_route("https://mcp.example/mcp", Ok(OAuthResponse { status: 401, body: String::new(), www_authenticate: Some("Bearer resource_metadata=\"https://mcp.example/.well-known/oauth-protected-resource\"".into()) })).await;
                    memory.push_route("https://mcp.example/.well-known/oauth-protected-resource", Ok(response(200, r#"{"resource":"https://mcp.example/mcp","authorization_servers":["https://as.example"],"scopes_supported":["mcp"]}"#))).await;
                    memory.push_route("https://as.example/.well-known/oauth-authorization-server", Ok(response(200, r#"{"issuer":"https://as.example","authorization_endpoint":"https://as.example/authorize","token_endpoint":"https://as.example/token","registration_endpoint":"https://as.example/register","revocation_endpoint":"https://as.example/revoke","code_challenge_methods_supported":["S256"]}"#))).await;
                    memory.push_route("https://as.example/register", Ok(response(201, r#"{"client_id":"client"}"#))).await;
                    memory.push_route("https://as.example/token", Ok(response(200, r#"{"access_token":"bench-access","refresh_token":"bench-refresh","expires_in":60}"#))).await;
                    memory.push_route("https://as.example/token", Ok(response(200, r#"{"access_token":"bench-replacement","refresh_token":"bench-refresh","expires_in":60}"#))).await;
                    let start = Instant::now();
                    let answer = owner.authorize(server, "docs", "https://mcp.example/mcp").await;
                    let AuthorizeAnswer::PendingConsent { consent_url, .. } = answer else { panic!("scripted consent failed: {answer:?}"); };
                    let state = consent_url.split("state=").nth(1).unwrap().split('&').next().unwrap().to_owned();
                    assert!(matches!(owner.complete_callback(server, CallbackQuery { state, code: Some("code".into()), denied: false }).await, AuthorizeAnswer::Ready { generation: 1 }));
                    assert_eq!(owner.bearer(server).await.unwrap().unwrap().generation, 1);
                    assert_eq!(owner.rejected(server, "Bearer").await.unwrap().unwrap().generation, 2);
                    if sample >= 10 { rows.push((lane, sample - 10, start.elapsed().as_nanos())); }
                }
                rows
            }));
        }
        let mut rows = Vec::new();
        for lane in lanes { rows.extend(lane.await.unwrap()); }
        rows
    });
    let mut raw = std::fs::File::create(output).unwrap();
    writeln!(raw, "record_instances,audit_instances,concurrency,scripted_network_ms,operation,lane,sample,elapsed_ns").unwrap();
    for (lane, sample, elapsed) in rows {
        writeln!(
            raw,
            "4,4,4,0,authorize_callback_bearer_refresh,{lane},{sample},{elapsed}"
        )
        .unwrap();
    }
    for lane in 0..4 {
        let lines =
            std::fs::read_to_string(directory.path().join(format!("lane-{lane}/audit.jsonl")))
                .unwrap();
        assert!(lines
            .lines()
            .all(|line| serde_json::from_str::<serde_json::Value>(line).is_ok()));
        assert!(!contains_token(directory.path(), "bench-access"));
        assert!(!contains_token(directory.path(), "bench-replacement"));
    }
}
