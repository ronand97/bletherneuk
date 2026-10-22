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

    fn key(&self) -> String {
        self.t.app.secrets.write_key.clone()
    }

    /// A read as a page makes it: with the key header.
    fn get(&self, path: &str) -> axum::http::request::Builder {
        Request::get(path)
            .header(header::HOST, self.host())
            .header("x-bletherneuk-key", self.key())
    }

    fn bare_get(&self, path: &str) -> axum::http::request::Builder {
        Request::get(path).header(header::HOST, self.host())
    }

    fn post(&self, path: &str) -> axum::http::request::Builder {
        Request::post(path)
            .header(header::HOST, self.host())
            .header(header::ORIGIN, self.t.app.origin())
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-bletherneuk-key", self.key())
    }

    async fn send(&self, req: Request<Body>) -> (StatusCode, axum::http::HeaderMap, String) {
        let resp = router(self.t.app.clone()).oneshot(req).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        (status, headers, String::from_utf8_lossy(&body).into_owned())
    }

    fn auth_path(&self, next: &str) -> String {
        let url = self.t.app.page_url(next);
        url.trim_start_matches(&self.t.app.origin()).to_string()
    }
}

#[tokio::test]
async fn reads_need_the_write_key() {
    let c = Client::new();
    let (s, _, _) = c
        .send(c.bare_get("/api/board").body(Body::empty()).unwrap())
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let mut wrong = c.get("/api/board").body(Body::empty()).unwrap();
    wrong
        .headers_mut()
        .insert("x-bletherneuk-key", "0".repeat(64).parse().unwrap());
    assert_eq!(c.send(wrong).await.0, StatusCode::UNAUTHORIZED);
    let (s, _, body) = c
        .send(c.get("/api/board").body(Body::empty()).unwrap())
        .await;
    assert_eq!(s, StatusCode::OK);
    assert!(body.contains("\"blocks\""));
}

#[tokio::test]
async fn cookies_are_never_set_or_honoured() {
    let c = Client::new();
    let (_, h, _) = c
        .send(c.bare_get(&c.auth_path("/")).body(Body::empty()).unwrap())
        .await;
    assert!(h.get(header::SET_COOKIE).is_none());
    // A cookie, whatever it holds, opens nothing.
    let req = c
        .bare_get("/api/board")
        .header(header::COOKIE, format!("bn_{}={}", c.t.app.port(), c.key()))
        .body(Body::empty())
        .unwrap();
    assert_eq!(c.send(req).await.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn wrong_host_is_refused() {
    let c = Client::new();
    for host in ["localhost", "evil.example:80", "127.0.0.1", "127.0.0.1:1"] {
        let mut req = c.get("/api/board").body(Body::empty()).unwrap();
        req.headers_mut()
            .insert(header::HOST, host.parse().unwrap());
        assert_eq!(c.send(req).await.0, StatusCode::FORBIDDEN, "{host}");
    }
}

#[tokio::test]
async fn cross_site_and_same_site_requests_are_refused_even_with_the_key() {
    let c = Client::new();
    // A page on another 127.0.0.1 port arrives as same-site.
    for site in ["cross-site", "same-site"] {
        let req = c
            .get("/api/board")
            .header("sec-fetch-site", site)
            .body(Body::empty())
            .unwrap();
        assert_eq!(c.send(req).await.0, StatusCode::FORBIDDEN, "{site}");
    }
    let req = c
        .get("/api/board")
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

    let mut no_key = c.post("/api/blocks/1/rename").body(body()).unwrap();
    no_key.headers_mut().remove("x-bletherneuk-key");
    assert_eq!(c.send(no_key).await.0, StatusCode::UNAUTHORIZED);

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
async fn a_code_opens_its_page_once() {
    let c = Client::new();
    let path = c.auth_path("/changes?block=3");
    let (s, h, body) = c.send(c.bare_get(&path).body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(h.get(header::CACHE_CONTROL).unwrap(), "no-store");
    if body.contains("bn-page") {
        assert!(body.contains("content=\"changes\""));
        assert!(body.contains("content=\"/changes?block=3\""));
        assert!(body.contains(&c.key()));
    }
    let (s, _, body) = c.send(c.bare_get(&path).body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "a second use is refused");
    assert!(!body.contains(&c.key()));
}

#[tokio::test]
async fn a_panel_code_serves_every_toolbelt_until_it_expires() {
    let c = Client::new();
    let url = c.t.app.page_url("/panel");
    let path = url.trim_start_matches(&c.t.app.origin()).to_string();
    for _ in 0..3 {
        let (s, _, _) = c.send(c.bare_get(&path).body(Body::empty()).unwrap()).await;
        assert_eq!(s, StatusCode::OK);
    }
    let code = url
        .split("code=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_string();
    c.t.app
        .codes
        .lock()
        .unwrap()
        .get_mut(&code)
        .unwrap()
        .expires = std::time::Instant::now() - std::time::Duration::from_secs(1);
    let (s, _, body) = c.send(c.bare_get(&path).body(Body::empty()).unwrap()).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert!(!body.contains(&c.key()));
}

#[tokio::test]
async fn expired_and_unknown_codes_never_reveal_the_key() {
    let c = Client::new();
    let url = c.t.app.page_url("/");
    let code = url
        .split("code=")
        .nth(1)
        .unwrap()
        .split('&')
        .next()
        .unwrap()
        .to_string();
    c.t.app
        .codes
        .lock()
        .unwrap()
        .get_mut(&code)
        .unwrap()
        .expires = std::time::Instant::now() - std::time::Duration::from_secs(1);
    let path = url.trim_start_matches(&c.t.app.origin()).to_string();
    for p in [path.as_str(), "/auth?code=nope&next=/", "/auth"] {
        let (s, _, body) = c.send(c.bare_get(p).body(Body::empty()).unwrap()).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{p}");
        assert!(!body.contains(&c.key()), "{p}");
    }
}

#[tokio::test]
async fn plain_page_paths_never_carry_the_key() {
    let c = Client::new();
    for p in ["/", "/panel", "/changes?block=1"] {
        let (s, _, body) = c.send(c.bare_get(p).body(Body::empty()).unwrap()).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED, "{p}");
        assert!(!body.contains(&c.key()), "{p}");
    }
}

#[tokio::test]
async fn every_response_carries_the_security_headers() {
    let c = Client::new();
    for req in [
        c.get("/api/board").body(Body::empty()).unwrap(),
        c.get("/nope").body(Body::empty()).unwrap(),
        c.bare_get("/api/board").body(Body::empty()).unwrap(),
        c.bare_get("/panel").body(Body::empty()).unwrap(),
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
