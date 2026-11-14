use super::*;
#[tokio::test]
async fn desktop_health_has_exact_generation_and_regular_health_has_none() {
    let endpoint =
        EndpointIdentity::new("8bebd14e-56d3-4bc7-9fc1-c79bed671ce4".into(), 122).unwrap();
    let plain = handle_http_health(Some(Extension(endpoint.clone())), None).await;
    assert_eq!(plain.status(), StatusCode::OK);
    assert_eq!(
        plain.headers()["x-nessa-endpoint-instance"],
        endpoint.instance()
    );
    assert_eq!(plain.headers()["x-nessa-endpoint-process-id"], "122");
    for name in [
        "x-nessa-runtime-fingerprint",
        "x-nessa-runtime-instance",
        "x-nessa-process-id",
        "x-nessa-service-generation",
    ] {
        assert!(!plain.headers().contains_key(name));
    }
    let fingerprint = "a".repeat(64);
    let desktop = handle_http_health(
        Some(Extension(endpoint)),
        Some(Extension(
            RunningRuntime::new(
                fingerprint.clone(),
                "b4a38c5b-cf70-4d90-9059-7d9a3a51c658".into(),
                123,
                "c".repeat(64),
            )
            .unwrap(),
        )),
    )
    .await;
    assert_eq!(
        desktop.headers()["x-nessa-runtime-fingerprint"],
        fingerprint
    );
    assert_eq!(
        desktop.headers()["x-nessa-runtime-instance"],
        "b4a38c5b-cf70-4d90-9059-7d9a3a51c658"
    );
    assert_eq!(desktop.headers()["x-nessa-process-id"], "123");
    assert_eq!(
        desktop.headers()["x-nessa-service-generation"],
        "c".repeat(64)
    );
}
