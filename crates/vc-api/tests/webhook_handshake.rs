use serde_json::json;

#[tokio::test]
async fn a_webhook_accepts_empty_and_plain_text_401_bodies() {
    for refusal_body in ["", "invalid signature"] {
        use axum::response::IntoResponse;
        let seed = [7u8; 32];
        let public = ed25519_dalek::SigningKey::from_bytes(&seed)
            .verifying_key()
            .to_bytes();
        let handler = move |headers: axum::http::HeaderMap, body: String| async move {
            let signature = headers["x-signature-ed25519"].to_str().unwrap();
            let timestamp = headers["x-signature-timestamp"].to_str().unwrap();
            if vc_api::discord::verify_signature(&public, signature, timestamp, body.as_bytes()) {
                axum::Json(json!({"type":1})).into_response()
            } else {
                (axum::http::StatusCode::UNAUTHORIZED, refusal_body).into_response()
            }
        };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = axum::Router::new().route("/hook", axum::routing::post(handler));
        let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let result = vc_api::notification::verify(
            &vc_api::notification::Direct::default(),
            &format!("http://{addr}/hook"),
            &seed,
            123,
        )
        .await;
        task.abort();
        assert_eq!(result, vc_api::notification::Handshake::Passed);
    }
}
