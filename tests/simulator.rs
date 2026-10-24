use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use les::{Destination, Les, SendRequest, server};
use tower::ServiceExt;

fn request() -> SendRequest {
    SendRequest {
        from: "sender@example.com".into(),
        destination: Destination {
            to_addresses: vec!["to@example.net".into()],
            ..Default::default()
        },
        subject: "日本語 subject".into(),
        text: Some("hello".into()),
        html: None,
    }
}
#[test]
fn validation_is_atomic_and_domain_verification_works() {
    let les = Les::in_memory().unwrap();
    assert!(matches!(
        les.send(request(), 10),
        Err(les::Error::Rejected(_))
    ));
    les.verify_identity("example.com").unwrap();
    let mut bad = request();
    bad.destination.cc_addresses = vec!["invalid".into()];
    assert!(les.send(bad, 11).is_err());
    let mut bad = request();
    bad.destination.to_addresses = vec!["to@example.net".into(); 51];
    assert!(les.send(bad, 12).is_err());
    let mut bad = request();
    bad.text = None;
    assert!(les.send(bad, 13).is_err());
    let id = les.send(request(), 42).unwrap();
    let messages = les.messages(0, 100).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].id, id);
    assert_eq!(messages[0].created_at_ms, 42);
    les.delete_identity("example.com").unwrap();
    assert!(les.send(request(), 43).is_err());
}
#[test]
fn persistent_store_survives_reopening_and_pages() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("les.sqlite");
    let id = {
        let les = Les::open(&path).unwrap();
        les.verify_identity("sender@example.com").unwrap();
        les.send(request(), 100).unwrap()
    };
    let les = Les::open(&path).unwrap();
    assert_eq!(les.identities().unwrap(), ["sender@example.com"]);
    assert_eq!(les.messages(0, 1).unwrap()[0].id, id);
    assert!(les.messages(1, 1).unwrap().is_empty());
}
async fn call(app: axum::Router, uri: &str, body: String) -> (StatusCode, String) {
    let response = app
        .oneshot(Request::post(uri).body(Body::from(body)).unwrap())
        .await
        .unwrap();
    assert!(response.headers().contains_key("x-amzn-requestid"));
    let status = response.status();
    (
        status,
        String::from_utf8(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap(),
    )
}
#[tokio::test]
async fn query_and_rest_share_state_and_use_protocol_errors() {
    let app = server::router(Les::in_memory().unwrap());
    let (status, body) = call(
        app.clone(),
        "/",
        "Action=VerifyEmailIdentity&EmailAddress=sender%40example.com".into(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("VerifyEmailIdentityResponse"));
    let input = serde_json::json!({"FromEmailAddress":"sender@example.com", "Destination":{"ToAddresses":["to@example.net"]}, "Content":{"Simple":{"Subject":{"Data":"hello"},"Body":{"Text":{"Data":"test"}}}}});
    let (status, body) = call(app.clone(), "/v2/email/outbound-emails", input.to_string()).await;
    assert_eq!(status, StatusCode::OK);
    assert!(serde_json::from_str::<serde_json::Value>(&body).unwrap()["MessageId"].is_string());
    let fields = [
        ("Action", "SendEmail"),
        ("Source", "sender@example.com"),
        ("Destination.ToAddresses.member.1", "to@example.net"),
        ("Message.Subject.Data", "a < b & c"),
        ("Message.Body.Text.Data", "body"),
    ];
    let (status, body) = call(
        app.clone(),
        "/",
        serde_urlencoded::to_string(fields).unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("<MessageId>"));
    let (status, body) = call(app.clone(), "/", "Action=SendRawEmail".into()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("<ErrorResponse"));
    let (status, body) = call(app.clone(), "/v2/email/outbound-emails", "{}".into()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("BadRequestException"));
    let response = app
        .oneshot(Request::get("/_les/messages").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let data: serde_json::Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(data["messages"].as_array().unwrap().len(), 2);
    assert_eq!(data["messages"][1]["request"]["subject"], "a < b & c");
}
#[tokio::test]
async fn unsupported_content_is_not_silently_accepted() {
    let app = server::router(Les::in_memory().unwrap());
    let (status, _) = call(app, "/v2/email/outbound-emails", serde_json::json!({"FromEmailAddress":"sender@example.com","Destination":{"ToAddresses":["to@example.net"]},"Content":{"Raw":{"Data":"aGVsbG8="}}}).to_string()).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[test]
fn mailbox_filters_before_pagination_and_includes_cc_bcc() {
    let les = Les::in_memory().unwrap();
    les.verify_identity("example.com").unwrap();
    let first = les.send(request(), 0).unwrap();
    let mut other = request();
    other.from = "other@example.com".into();
    other.destination.to_addresses = vec!["other@example.net".into()];
    other.destination.cc_addresses = vec!["cc@example.net".into()];
    other.destination.bcc_addresses = vec!["bcc@example.net".into(), "cc@example.net".into()];
    let last = les.send(other, 1000).unwrap();
    assert_eq!(
        les.mailbox(false, "sender@example.com", 0, 20).unwrap()[0].id,
        first
    );
    assert_eq!(
        les.mailbox(true, "to@example.net", 0, 20).unwrap()[0].id,
        first
    );
    for address in ["cc@example.net", "bcc@example.net"] {
        let messages = les.mailbox(true, address, 0, 20).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].id, last);
    }
    assert_eq!(les.mailbox(true, "", 0, 1).unwrap()[0].id, last);
    assert_eq!(les.mailbox(true, "", 1, 1).unwrap()[0].id, first);
    assert!(
        les.mailbox(false, "missing@example.net", 0, 20)
            .unwrap()
            .is_empty()
    );
    assert!(les.message("missing").unwrap().is_none());
}
async fn get_page(app: axum::Router, uri: &str) -> (StatusCode, axum::http::HeaderMap, String) {
    let response = app
        .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = String::from_utf8(
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    (status, headers, body)
}
#[tokio::test]
async fn mailbox_ui_escapes_content_and_isolates_html_preview() {
    let les = Les::in_memory().unwrap();
    les.verify_identity("example.com").unwrap();
    let mut req = request();
    req.subject = "<script>alert(1)</script>".into();
    req.text = Some("<img src=x onerror=alert(1)>".into());
    req.html = Some("<h1>Hello</h1><script>alert(1)</script>".into());
    let id = les.send(req, 0).unwrap();
    let app = server::router(les);
    let (status, headers, _) = get_page(app.clone(), "/").await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(headers["location"], "/_les/mail");
    for path in [
        "/_les/mail?folder=inbox".into(),
        "/_les/mail?folder=sent".into(),
        format!("/_les/mail/{id}"),
    ] {
        let (status, headers, body) = get_page(app.clone(), &path).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers["cache-control"], "no-store");
        assert!(body.contains("&lt;script&gt;"));
        assert!(!body.contains("<script>"));
        assert!(body.contains("1970-01-01 00:00:00"));
    }
    let (_, headers, body) = get_page(app.clone(), &format!("/_les/mail/{id}/preview")).await;
    assert!(
        headers["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("sandbox; default-src 'none'")
    );
    assert!(body.contains("<h1>Hello</h1>"));
    assert_eq!(
        get_page(app.clone(), "/_les/mail/missing").await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get_page(app.clone(), "/_les/mail?folder=unknown").await.0,
        StatusCode::BAD_REQUEST
    );
    let (_, _, body) = get_page(app, "/_les/mail?address=missing%40example.net").await;
    assert!(body.contains("該当するメールはありません"));
}

#[tokio::test]
async fn v2_duplicate_identity_returns_modeled_error_and_v1_remains_idempotent() {
    let app = server::router(Les::in_memory().unwrap());
    for name in ["sender@example.com", "example.org"] {
        let payload = serde_json::json!({"EmailIdentity": name}).to_string();
        assert_eq!(
            call(app.clone(), "/v2/email/identities", payload.clone())
                .await
                .0,
            StatusCode::OK
        );
        let response = app
            .clone()
            .oneshot(
                Request::post("/v2/email/identities")
                    .body(Body::from(payload))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response.headers()["x-amzn-errortype"],
            "AlreadyExistsException"
        );
        assert!(response.headers().contains_key("x-amzn-requestid"));
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap())
                .unwrap();
        assert_eq!(body["__type"], "AlreadyExistsException");
    }
    for _ in 0..2 {
        assert_eq!(
            call(
                app.clone(),
                "/",
                "Action=VerifyEmailIdentity&EmailAddress=sender%40example.com".into()
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    let response = app
        .clone()
        .oneshot(
            Request::delete("/v2/email/identities/sender@example.com")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        call(
            app,
            "/v2/email/identities",
            serde_json::json!({"EmailIdentity":"sender@example.com"}).to_string()
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn preview_keeps_long_body_but_removes_navigation_and_active_content() {
    let les = Les::in_memory().unwrap();
    les.verify_identity("example.com").unwrap();
    let mut req = request();
    let html = format!(
        r#"<meta http-equiv="refresh" content="0;url=https://example.org"><base href="https://example.org"><a href="https://example.org" tabindex="0">Link text</a><a href="/health">Local link</a><script>alert(1)</script><form action="/health"><button>Submit</button></form><iframe src="https://example.org"></iframe>{}<p>END OF LONG MESSAGE</p>"#,
        "<p>Long message line</p>".repeat(200)
    );
    req.html = Some(html.clone());
    let id = les.send(req, 0).unwrap();
    let app = server::router(les);
    let (_, _, detail) = get_page(app.clone(), &format!("/_les/mail/{id}")).await;
    assert!(detail.contains("sandbox tabindex=\"0\""));
    assert!(!detail.contains("pointer-events:none"));
    assert!(detail.contains("&lt;meta http-equiv=")); // original source retained
    let (status, headers, preview) = get_page(app, &format!("/_les/mail/{id}/preview")).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        headers["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("sandbox; default-src 'none'")
    );
    assert!(preview.contains("Link text"));
    assert!(preview.contains("Local link"));
    assert!(preview.contains("END OF LONG MESSAGE"));
    assert_eq!(preview.matches("Long message line").count(), 200);
    for forbidden in [
        "<a ",
        "<meta http-equiv",
        "<base",
        "<script",
        "<form",
        "<button",
        "<iframe",
        "href=",
    ] {
        assert!(
            !preview.contains(forbidden),
            "Unexpected active markup: {forbidden}"
        );
    }
}
