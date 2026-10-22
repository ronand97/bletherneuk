//! Design §20.11: web server security tests.

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use http_body_util::BodyExt;
use tower::ServiceExt;

use super::*;
use crate::service::testing::test_app;

struct Client {
    t: crate::service::testing::TestApp,
}

impl Client {
    fn new() -> Client {
        Client { t: test_app() }
    }

    fn host(&self) -> String {
        format!("127.0.0.1:{}", self.t.app.port())
    }

    fn cookie(&self) -> String {
        format!("bn_{}={}", self.t.app.port(), self.t.app.secrets.session)
    }

    fn get(&self, path: &str) -> axum::http::request::Builder {
        Request::get(path).header(header::HOST, self.host())
    }

    fn post(&self, path: &str) -> axum::http::request::Builder {
        Request::post(path)
            .header(header::HOST, self.host())
            .header(header::ORIGIN, self.t.app.origin())
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, self.cookie())
            .header("x-bletherneuk-key", self.t.app.secrets.write_key.clone())
    }

    async fn send(&self, req: Request<Body>) -> (StatusCode, axum::http::HeaderMap, String) {
        let resp = router(self.t.app.clone()).oneshot(req).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        (status, headers, String::from_utf8_lossy(&body).into_owned())
    }
}

fn code_of(url: &str) -> String {
    url.split("code=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_string()
}

#[tokio::test]
async fn reads_need_the_session_cookie() {
    let c = Client::new();
    let (s, _, _) = c
        .send(c.get("/api/board").body(Body::empty()).unwrap())
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _, body) = c
        .send(
            c.get("/api/board")
                .header(header::COOKIE, c.cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert!(body.contains("\"blocks\""));
}

#[tokio::test]
async fn wrong_host_is_refused() {
    let c = Client::new();
    for host in ["localhost", "evil.example:80", "127.0.0.1", "127.0.0.1:1"] {
        let req = Request::get("/api/board")
            .header(header::HOST, host)
            .header(header::COOKIE, c.cookie())
            .body(Body::empty())
            .unwrap();
        let (s, _, _) = c.send(req).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{host}");
    }
}

#[tokio::test]
async fn cross_site_and_same_site_requests_are_refused_even_with_the_cookie() {
    let c = Client::new();
    // A page on another 127.0.0.1 port arrives as same-site and carries our cookie.
    for site in ["cross-site", "same-site"] {
        let req = c
            .get("/api/board")
            .header(header::COOKIE, c.cookie())
            .header("sec-fetch-site", site)
            .body(Body::empty())
            .unwrap();
        let (s, _, _) = c.send(req).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{site}");
    }
    let req = c
        .get("/api/board")
        .header(header::COOKIE, c.cookie())
        .header("sec-fetch-site", "same-origin")
        .body(Body::empty())
        .unwrap();
    assert_eq!(c.send(req).await.0, StatusCode::OK);
}

#[tokio::test]
async fn posts_need_the_write_key_and_exact_origin() {
    let c = Client::new();
    let body = || Body::from("{\"title\":\"x\"}");
    let (s, _, _) = c
        .send(c.post("/api/blocks/1/rename").body(body()).unwrap())
        .await;
    assert_eq!(s, StatusCode::OK, "a well-formed request gets through");

    let no_key = Request::post("/api/blocks/1/rename")
        .header(header::HOST, c.host())
        .header(header::ORIGIN, c.t.app.origin())
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, c.cookie())
        .body(body())
        .unwrap();
    assert_eq!(c.send(no_key).await.0, StatusCode::FORBIDDEN);

    let wrong_key = c.post("/api/blocks/1/rename");
    let mut req = wrong_key.body(body()).unwrap();
    req.headers_mut()
        .insert("x-bletherneuk-key", "0".repeat(64).parse().unwrap());
    assert_eq!(c.send(req).await.0, StatusCode::FORBIDDEN);

    for origin in [
        "http://127.0.0.1:1",
        "http://localhost",
        "null",
        "https://example.com",
    ] {
        let mut req = c.post("/api/blocks/1/rename").body(body()).unwrap();
        req.headers_mut()
            .insert(header::ORIGIN, origin.parse().unwrap());
        assert_eq!(c.send(req).await.0, StatusCode::FORBIDDEN, "{origin}");
    }
    let mut req = c.post("/api/blocks/1/rename").body(body()).unwrap();
    req.headers_mut().remove(header::ORIGIN);
    assert_eq!(c.send(req).await.0, StatusCode::FORBIDDEN, "no origin");
}

#[tokio::test]
async fn preflight_gets_no_cors_headers() {
    let c = Client::new();
    let req = Request::options("/api/blocks/1/rename")
        .header(header::HOST, c.host())
        .header(header::ORIGIN, "http://127.0.0.1:1")
        .header("access-control-request-method", "POST")
        .header("access-control-request-headers", "x-bletherneuk-key")
        .body(Body::empty())
        .unwrap();
    let (s, h, _) = c.send(req).await;
    assert_eq!(s, StatusCode::METHOD_NOT_ALLOWED);
    assert!(h.keys().all(|k| !k.as_str().starts_with("access-control-")));
}

#[tokio::test]
async fn auth_codes_work_once_and_never_reappear() {
    let c = Client::new();
    let url = c.t.app.page_url("/panel");
    let code = code_of(&url);
    let path = url.trim_start_matches(&c.t.app.origin()).to_string();
    let (s, h, body) = c.send(c.get(&path).body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::OK);
    let cookie = h.get(header::SET_COOKIE).unwrap().to_str().unwrap();
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Strict"));
    assert!(cookie.contains("Path=/"));
    assert!(body.contains("url=/panel"));
    assert!(!body.contains(&code), "the hop never carries the code");
    assert_eq!(h.get(header::CACHE_CONTROL).unwrap(), "no-store");

    // Second use, with no cookie: refused.
    let (s, h, _) = c.send(c.get(&path).body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert!(h.get(header::SET_COOKIE).is_none());

    // Second use from a panel that already shares the cookie: let through to `next`.
    let (s, _, body) = c
        .send(
            c.get(&path)
                .header(header::COOKIE, c.cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    assert!(body.contains("url=/panel"));
}

#[tokio::test]
async fn expired_codes_are_refused() {
    let c = Client::new();
    let url = c.t.app.page_url("/");
    let code = code_of(&url);
    c.t.app
        .codes
        .lock()
        .unwrap()
        .get_mut(&code)
        .unwrap()
        .expires = std::time::Instant::now() - std::time::Duration::from_secs(1);
    let path = url.trim_start_matches(&c.t.app.origin()).to_string();
    let (s, h, _) = c.send(c.get(&path).body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert!(h.get(header::SET_COOKIE).is_none());
}

#[tokio::test]
async fn auth_next_cannot_point_elsewhere() {
    let c = Client::new();
    for next in [
        "https://evil.example/",
        "//evil.example",
        "/api/board",
        "javascript:alert(1)",
    ] {
        let path = format!(
            "/auth?code=nope&next={}",
            url::form_urlencoded::byte_serialize(next.as_bytes()).collect::<String>()
        );
        let (_, _, body) = c
            .send(
                c.get(&path)
                    .header(header::COOKIE, c.cookie())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await;
        assert!(body.contains("url=/\""), "{next}: {body}");
    }
}

#[tokio::test]
async fn every_response_carries_the_security_headers() {
    let c = Client::new();
    for req in [
        c.get("/api/board")
            .header(header::COOKIE, c.cookie())
            .body(Body::empty())
            .unwrap(),
        c.get("/nope")
            .header(header::COOKIE, c.cookie())
            .body(Body::empty())
            .unwrap(),
        c.get("/api/board").body(Body::empty()).unwrap(),
    ] {
        let (_, h, _) = c.send(req).await;
        assert_eq!(h.get(header::CONTENT_SECURITY_POLICY).unwrap(), CSP);
        assert_eq!(h.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(), "nosniff");
        assert_eq!(h.get(header::REFERRER_POLICY).unwrap(), "no-referrer");
        assert_eq!(h.get(header::X_FRAME_OPTIONS).unwrap(), "DENY");
        assert!(h.get(header::ACCESS_CONTROL_ALLOW_ORIGIN).is_none());
    }
}

#[tokio::test]
async fn pages_carry_the_write_key_only_when_signed_in() {
    let c = Client::new();
    let (s, _, body) = c.send(c.get("/panel").body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert!(!body.contains(&c.t.app.secrets.write_key));
    let (s, _, body) = c
        .send(
            c.get("/panel")
                .header(header::COOKIE, c.cookie())
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    if body.contains("<head>") || body.contains("bn-page") {
        assert!(body.contains("content=\"panel\""));
        assert!(body.contains(&c.t.app.secrets.write_key));
    }
}

#[tokio::test]
async fn the_api_takes_ids_not_paths() {
    let c = Client::new();
    for path in [
        "/api/files/..%2F..%2Fetc%2Fpasswd/open",
        "/api/chats/not-a-uuid/open",
    ] {
        let (s, _, _) = c.send(c.post(path).body(Body::from("{}")).unwrap()).await;
        assert_ne!(s, StatusCode::OK, "{path}");
    }
}
