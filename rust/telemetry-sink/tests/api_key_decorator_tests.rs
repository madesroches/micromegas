use micromegas_telemetry_sink::api_key_decorator::ApiKeyRequestDecorator;
use micromegas_telemetry_sink::request_decorator::RequestDecorator;

#[tokio::test]
async fn test_api_key_decorator_adds_header() {
    let decorator = ApiKeyRequestDecorator::new("test-key-123".to_string());
    let mut request = reqwest::Client::new()
        .post("http://example.com")
        .build()
        .expect("build request");

    decorator.decorate(&mut request).await.expect("decorate");

    let auth_header = request.headers().get(reqwest::header::AUTHORIZATION);
    assert!(auth_header.is_some());
    assert_eq!(
        auth_header.expect("header").to_str().expect("to_str"),
        "Bearer test-key-123"
    );
}

#[tokio::test]
async fn test_api_key_decorator_with_explicit_key() {
    let decorator = ApiKeyRequestDecorator::new("explicit-key-456".to_string());

    let mut request = reqwest::Client::new()
        .post("http://example.com")
        .build()
        .expect("build request");

    decorator.decorate(&mut request).await.expect("decorate");

    let auth_header = request.headers().get(reqwest::header::AUTHORIZATION);
    assert_eq!(
        auth_header.expect("header").to_str().expect("to_str"),
        "Bearer explicit-key-456"
    );
}

#[tokio::test]
async fn test_api_key_decorator_multiple_requests() {
    let decorator = ApiKeyRequestDecorator::new("multi-key-789".to_string());

    for _ in 0..3 {
        let mut request = reqwest::Client::new()
            .post("http://example.com")
            .build()
            .expect("build request");

        decorator.decorate(&mut request).await.expect("decorate");

        let auth_header = request.headers().get(reqwest::header::AUTHORIZATION);
        assert_eq!(
            auth_header.expect("header").to_str().expect("to_str"),
            "Bearer multi-key-789"
        );
    }
}
