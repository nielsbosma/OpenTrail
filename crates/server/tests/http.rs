#![allow(clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{Request, Response, StatusCode, header};
use http_body_util::BodyExt;
use opentrail_core::testing::app;
use opentrail_server::{AppState, router};
use serde_json::{Value, json};
use tower::ServiceExt;

const PASSWORD: &str = "correct horse battery staple 42";
const ORIGIN: &str = "http://localhost:3000";

struct Client {
    router: axum::Router,
    ip: &'static str,
}

impl Client {
    fn rpc(&self, name: &str) -> axum::http::request::Builder {
        Request::post(format!("/api/rpc/{name}")).header(header::CONTENT_TYPE, "application/json").header("x-forwarded-for", self.ip)
    }

    async fn send(&self, request: Request<Body>) -> (Response<Body>, Value) {
        let response = self.router.clone().oneshot(request).await.unwrap();
        let (parts, body) = response.into_parts();
        let bytes = body.collect().await.unwrap().to_bytes();
        let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (Response::from_parts(parts, Body::empty()), json)
    }

    async fn call(&self, name: &str, body: Value, extra: &[(&str, &str)]) -> (Response<Body>, Value) {
        let mut builder = self.rpc(name);
        for (k, v) in extra {
            builder = builder.header(*k, *v);
        }
        self.send(builder.body(Body::from(body.to_string())).unwrap()).await
    }
}

fn cookie_of(response: &Response<Body>) -> String {
    let set = response.headers().get(header::SET_COOKIE).unwrap().to_str().unwrap();
    assert!(set.contains("HttpOnly") && set.contains("SameSite=Lax"), "{set}");
    set.split(';').next().unwrap().to_string()
}

#[tokio::test]
async fn sessions_csrf_rate_limits_and_api_keys() {
    let Some(app) = app().await else { return };
    let state = AppState::new(app.clone());
    let client = Client { router: router(state), ip: "203.0.113.7" };

    let (response, _) = client.call("account.me", json!({}), &[]).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let h = response.headers();
    assert!(h.get(header::CONTENT_SECURITY_POLICY).unwrap().to_str().unwrap().contains("frame-ancestors 'none'"));
    assert_eq!(h.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(), "nosniff");
    assert_eq!(h.get(header::REFERRER_POLICY).unwrap(), "no-referrer");
    assert!(h.get("x-request-id").is_some());

    let (response, _) = client.call("no.such_call", json!({}), &[]).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let setup = json!({ "setupCode": app.keys.setup_code(), "orgName": "Acme AB", "name": "Ada", "email": "ada@example.com", "password": PASSWORD });
    let (response, _) = client.call("setup.create_first_admin", setup, &[]).await;
    assert_eq!(response.status(), StatusCode::OK);

    let (response, body) = client.call("auth.login", json!({ "email": "ada@example.com", "password": PASSWORD }), &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body["status"], "signed_in");
    assert!(body.get("token").is_none(), "the session token only travels in the cookie");
    assert_eq!(response.headers().get(header::CACHE_CONTROL).unwrap(), "no-store");
    let cookie = cookie_of(&response);
    assert!(cookie.starts_with("opentrail_session="));

    let (response, me) = client.call("account.me", json!({}), &[("cookie", &cookie), ("origin", ORIGIN)]).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(me["email"], "ada@example.com");

    for origin in ["https://evil.example", "https://localhost:3000", "null"] {
        let (response, _) = client.call("account.me", json!({}), &[("cookie", &cookie), ("origin", origin)]).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "a cookie write from {origin} is refused");
    }
    let (response, _) = client.call("account.me", json!({}), &[("cookie", &cookie)]).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN, "no Origin and no Sec-Fetch-Site is refused");
    let (response, _) = client.call("account.me", json!({}), &[("cookie", &cookie), ("sec-fetch-site", "cross-site")]).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let (response, _) = client.call("account.me", json!({}), &[("cookie", &cookie), ("sec-fetch-site", "same-origin")]).await;
    assert_eq!(response.status(), StatusCode::OK);

    let key_input = json!({ "name": "CI", "permissions": ["users.read"] });
    let (response, key) = client.call("api_keys.create", key_input.clone(), &[("cookie", &cookie), ("origin", ORIGIN)]).await;
    assert_eq!(response.status(), StatusCode::OK, "{key}");
    let bearer = format!("Bearer {}", key["key"].as_str().unwrap());
    let (response, _) = client.call("users.list", json!({}), &[("authorization", &bearer)]).await;
    assert_eq!(response.status(), StatusCode::OK, "an API key needs no Origin: it is not ambient");
    let (response, _) = client.call("api_keys.create", key_input, &[("authorization", &bearer)]).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN, "keys cannot reach app-only calls");
    let (response, _) = client.call("account.me", json!({}), &[("authorization", "Bearer otk_not_a_real_key")]).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "a bad key is refused, not ignored");

    let auth: &[(&str, &str)] = &[("cookie", &cookie), ("origin", ORIGIN)];
    let forged = json!({ "group": "email", "values": { "fromAddress": "sign@example.com", "verifiedAt": "2020-01-01T00:00:00Z" } });
    let (response, _) = client.call("settings.update", forged, auth).await;
    assert_eq!(response.status(), StatusCode::OK);
    let (_, status) = client.call("setup.status", json!({}), auth).await;
    assert_eq!(status["emailConfigured"], false, "only a test email that went out marks email as working");
    let (response, _) = client.call("settings.send_test_email", json!({}), auth).await;
    assert_eq!(response.status(), StatusCode::OK);
    let (_, status) = client.call("setup.status", json!({}), auth).await;
    assert_eq!(status["emailConfigured"], true);
    let changed = json!({ "group": "email", "values": { "fromAddress": "other@example.com" } });
    client.call("settings.update", changed, auth).await;
    let (_, status) = client.call("setup.status", json!({}), auth).await;
    assert_eq!(status["emailConfigured"], false, "a new sender needs a new test");

    let url = json!({ "group": "email", "values": { "provider": "smtp", "smtpHost": "smtp://mail.example.com" } });
    let (response, _) = client.call("settings.update", url, auth).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST, "a URL is not a host name");
    let plain = json!({ "group": "email", "values": { "provider": "smtp", "smtpHost": "mail.example.com", "smtpSecurity": "none" } });
    let (response, _) = client.call("settings.update", plain, auth).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST, "unencrypted SMTP is refused");
    let smtp = json!({ "group": "email", "values": {
        "provider": "smtp", "smtpHost": "mail.example.com", "smtpPort": 465, "smtpSecurity": "tls", "smtpUsername": "opentrail", "smtpPassword": "hunter2"
    } });
    let (response, saved) = client.call("settings.update", smtp, auth).await;
    assert_eq!(response.status(), StatusCode::OK, "{saved}");
    assert_eq!(saved["values"]["smtpPassword"], true, "the password is never returned");
    assert_eq!(saved["values"]["smtpHost"], "mail.example.com");

    let (response, _) = client.call("auth.logout", json!({}), &[("cookie", &cookie), ("origin", ORIGIN)]).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get(header::SET_COOKIE).unwrap().to_str().unwrap().contains("Max-Age=0"));
    let (response, _) = client.call("account.me", json!({}), &[("cookie", &cookie), ("origin", ORIGIN)]).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "the session is gone server side too");

    let guesser = Client { router: client.router.clone(), ip: "198.51.100.9" };
    let mut statuses = Vec::new();
    for _ in 0..12 {
        let (response, _) = guesser.call("auth.login", json!({ "email": "ada@example.com", "password": "wrong" }), &[]).await;
        statuses.push(response.status());
    }
    assert_eq!(statuses.last(), Some(&StatusCode::TOO_MANY_REQUESTS));
    let (response, _) = client.call("auth.login", json!({ "email": "bo@example.com", "password": PASSWORD }), &[]).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "other addresses are not throttled");
    let (response, _) = client.call("auth.login", json!({ "email": "ada@example.com", "password": PASSWORD }), &[]).await;
    assert_eq!(response.status(), StatusCode::OK, "and one address cannot lock the owner out");

    let (response, body) = client.send(Request::get("/api/nothing-here").body(Body::empty()).unwrap()).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(body["kind"], "not_found");
    let (response, body) = client.send(Request::get("/api/health").body(Body::empty()).unwrap()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn rest_api_with_keys_and_openapi() {
    let Some(app) = app().await else { return };
    let client = Client { router: router(AppState::new(app.clone())), ip: "203.0.113.9" };
    let setup = json!({ "setupCode": app.keys.setup_code(), "orgName": "Acme AB", "name": "Ada", "email": "ada@example.com", "password": PASSWORD });
    client.call("setup.create_first_admin", setup, &[]).await;
    let (response, _) = client.call("auth.login", json!({ "email": "ada@example.com", "password": PASSWORD }), &[]).await;
    let cookie = cookie_of(&response);
    let session: [(&str, &str); 2] = [("cookie", &cookie), ("origin", ORIGIN)];
    let (_, key) = client.call("api_keys.create", json!({ "name": "CRM", "permissions": ["users.read", "audit.read"] }), &session).await;
    let bearer = format!("Bearer {}", key["key"].as_str().unwrap());
    let (_, narrow) = client.call("api_keys.create", json!({ "name": "Org only", "permissions": ["org.read"] }), &session).await;
    let narrow = format!("Bearer {}", narrow["key"].as_str().unwrap());

    let rest = |method: &str, path: &str, auth: &str, body: Option<Value>, idempotency: Option<&str>| {
        let mut b = Request::builder()
            .method(method)
            .uri(format!("/api/v1{path}"))
            .header("authorization", auth)
            .header("x-forwarded-for", "203.0.113.9");
        if let Some(k) = idempotency {
            b = b.header("idempotency-key", k);
        }
        match body {
            Some(body) => b.header(header::CONTENT_TYPE, "application/json").body(Body::from(body.to_string())).unwrap(),
            None => b.body(Body::empty()).unwrap(),
        }
    };

    let (_, manager) =
        client.call("api_keys.create", json!({ "name": "Ops", "permissions": ["api_keys.manage", "org.read"] }), &session).await;
    let manager_bearer = format!("Bearer {}", manager["key"].as_str().unwrap());
    let manager_path = format!("/api-keys/{}", manager["id"].as_str().unwrap());
    let (response, edited) = client
        .send(rest("PATCH", &manager_path, &manager_bearer, Some(json!({ "name": "Ops edited", "allowedIps": ["203.0.113.0/24"] })), None))
        .await;
    assert_eq!(response.status(), StatusCode::OK, "{edited}");
    assert_eq!(edited["allowedIps"], json!(["203.0.113.0/24"]));
    assert!(edited.get("key").is_none() && !edited.to_string().contains(manager["key"].as_str().unwrap()));
    let (response, _) = client
        .send(rest("PATCH", &manager_path, &manager_bearer, Some(json!({ "permissions": ["api_keys.manage", "settings.manage"] })), None))
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN, "a key cannot widen a key");
    let (response, _) = client.send(rest("PATCH", &manager_path, &manager_bearer, Some(json!({ "allowedIps": [] })), None)).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN, "a key cannot lift an IP allowlist");
    let (response, listed) = client.send(rest("GET", "/api-keys", &manager_bearer, None, None)).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(listed.as_array().unwrap().iter().any(|k| k["name"] == "Ops edited"));

    let (response, users) = client.send(rest("GET", "/users", &bearer, None, None)).await;
    assert_eq!(response.status(), StatusCode::OK, "{users}");
    assert_eq!(users.as_array().unwrap().len(), 1);
    let (response, entries) = client.send(rest("GET", "/audit?limit=5", &bearer, None, None)).await;
    assert_eq!(response.status(), StatusCode::OK, "{entries}");
    let (response, _) = client.send(rest("GET", "/audit?limit=lots", &bearer, None, None)).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let (response, _) = client.send(rest("GET", "/audit", &narrow, None, None)).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN, "a key without audit.read cannot read the log");
    let (response, _) = client.send(rest("GET", "/users", "Bearer nope", None, None)).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let (response, spec) = client.send(Request::get("/api/v1/openapi.json").body(Body::empty()).unwrap()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(spec["openapi"], "3.1.0");
    let text = spec.to_string();
    assert!(!text.contains("#/$defs/"), "every reference points into components");
    for (path, ops) in spec["paths"].as_object().unwrap() {
        for (method, op) in ops.as_object().unwrap() {
            assert!(op["operationId"].is_string(), "{method} {path}");
        }
    }
    let revoke = &spec["paths"]["/api-keys/{id}"]["delete"];
    assert_eq!(revoke["parameters"][0]["name"], "id");
    let refs: Vec<String> =
        text.match_indices("#/components/schemas/").map(|(i, _)| text[i + 21..].split('"').next().unwrap().to_string()).collect();
    for r in refs {
        assert!(spec["components"]["schemas"].get(&r).is_some(), "{r} is defined");
    }
}

#[test]
fn every_rest_path_parameter_is_an_input_field() {
    use opentrail_core::services::registry;
    let services: std::collections::HashMap<_, _> = registry().into_iter().map(|s| (s.meta().name, s)).collect();
    for route in opentrail_server::rest::ROUTES {
        let service = services.get(route.service).unwrap_or_else(|| panic!("{} is registered", route.service));
        let schema = serde_json::to_value(service.input_schema()).unwrap();
        for param in route.path.split('/').filter_map(|s| s.strip_prefix('{').and_then(|s| s.strip_suffix('}'))) {
            assert!(schema["properties"].get(param).is_some(), "{} {} has no field {param}", route.method, route.path);
        }
        for (field, _) in route.fixed {
            assert!(schema["properties"].get(*field).is_some(), "{} has no field {field}", route.service);
        }
    }
}

#[tokio::test]
async fn bodies_are_refused_before_they_are_read() {
    let Some(app) = app().await else { return };
    let client = Client { router: router(AppState::new(app.clone())), ip: "203.0.113.20" };
    let setup = json!({ "setupCode": app.keys.setup_code(), "orgName": "Acme AB", "name": "Ada", "email": "ada@example.com", "password": PASSWORD });
    client.call("setup.create_first_admin", setup, &[]).await;

    let big = vec![b'['; 3 * 1024 * 1024];
    let send_big = |name: &str| client.rpc(name).body(Body::from(big.clone())).unwrap();
    let (response, _) = client.send(send_big("users.list")).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED, "no credentials: the size does not matter");
    let (response, _) = client.send(send_big("setup.status")).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE, "a public call takes two megabytes at most");
}

#[tokio::test]
async fn public_calls_refuse_other_sites() {
    let Some(app) = app().await else { return };
    let client = Client { router: router(AppState::new(app.clone())), ip: "203.0.113.21" };
    let setup = json!({ "setupCode": app.keys.setup_code(), "orgName": "Acme AB", "name": "Ada", "email": "ada@example.com", "password": PASSWORD });
    client.call("setup.create_first_admin", setup, &[]).await;
    let login = json!({ "email": "ada@example.com", "password": PASSWORD });

    let form = Request::post("/api/rpc/auth.login")
        .header(header::CONTENT_TYPE, "text/plain")
        .header("x-forwarded-for", client.ip)
        .body(Body::from(login.to_string()))
        .unwrap();
    let (response, _) = client.send(form).await;
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let untyped = Request::post("/api/rpc/auth.login").header("x-forwarded-for", client.ip).body(Body::from(login.to_string())).unwrap();
    let (response, _) = client.send(untyped).await;
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    let (response, _) = client.call("auth.login", login.clone(), &[("sec-fetch-site", "cross-site")]).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let (response, _) = client.call("auth.login", login, &[("sec-fetch-site", "same-origin")]).await;
    assert_eq!(response.status(), StatusCode::OK);
    let empty = Request::post("/api/rpc/setup.status").header("x-forwarded-for", client.ip).body(Body::empty()).unwrap();
    let (response, _) = client.send(empty).await;
    assert_eq!(response.status(), StatusCode::OK, "no body needs no content type");
}
