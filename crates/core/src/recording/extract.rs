//! Turns a batch of rrweb events into rows agents can query: pages, interactions, console, network,
//! errors, vitals and custom events. It reads the stream and never judges it.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use super::mask::clean_url;

const TEXT: usize = 4000;
const STACK: usize = 8000;
const NAME: usize = 200;
const BODY: usize = 10 * 1024;
const PROPS: usize = 4 * 1024;

/// rrweb's event and source numbers.
mod rr {
    pub const FULL_SNAPSHOT: i64 = 2;
    pub const INCREMENTAL: i64 = 3;
    pub const META: i64 = 4;
    pub const CUSTOM: i64 = 5;
    pub const PLUGIN: i64 = 6;
    pub const MOUSE_INTERACTION: i64 = 2;
    pub const SCROLL: i64 = 3;
    pub const INPUT: i64 = 5;
    pub const CLICK: i64 = 2;
    pub const CONTEXT_MENU: i64 = 3;
    pub const DBL_CLICK: i64 = 4;
}

pub struct Page {
    pub ts: DateTime<Utc>,
    pub url: String,
    pub title: Option<String>,
}

pub struct Interaction {
    pub ts: DateTime<Utc>,
    pub url: Option<String>,
    pub kind: &'static str,
    pub node_id: Option<i32>,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub input_length: Option<i32>,
}

#[derive(Clone, Default)]
pub struct Element {
    pub role: Option<String>,
    pub name: Option<String>,
    pub testid: Option<String>,
    pub component: Option<String>,
}

pub struct Console {
    pub ts: DateTime<Utc>,
    pub level: &'static str,
    pub message: String,
    pub stack: Option<String>,
}

pub struct Network {
    pub ts: DateTime<Utc>,
    pub initiator: &'static str,
    pub method: String,
    pub url: String,
    pub status: i32,
    pub duration_ms: Option<i32>,
    pub bytes: Option<i64>,
    pub request_body: Option<String>,
    pub response_body: Option<String>,
}

pub struct Error {
    pub ts: DateTime<Utc>,
    pub kind: &'static str,
    pub message: String,
    pub stack: Option<String>,
    pub url: Option<String>,
}

pub struct Vital {
    pub ts: DateTime<Utc>,
    pub url: Option<String>,
    pub name: &'static str,
    pub value: f64,
}

pub struct Custom {
    pub ts: DateTime<Utc>,
    pub name: String,
    pub props: Option<Value>,
}

#[derive(Default)]
pub struct Extracted {
    pub pages: Vec<Page>,
    pub interactions: Vec<Interaction>,
    pub elements: BTreeMap<i32, Element>,
    pub console: Vec<Console>,
    pub network: Vec<Network>,
    pub errors: Vec<Error>,
    pub vitals: Vec<Vital>,
    pub custom: Vec<Custom>,
    pub user_ref: Option<String>,
    pub screen: Option<(i32, i32)>,
}

pub fn ts_of(event: &Value) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp_millis(event.get("timestamp")?.as_i64()?)
}

fn text(value: Option<&Value>, max: usize) -> Option<String> {
    let s = match value? {
        Value::String(s) => s.clone(),
        Value::Null => return None,
        other => other.to_string(),
    };
    let s = s.trim();
    (!s.is_empty()).then(|| truncate(s, max))
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

fn int(value: Option<&Value>) -> Option<i32> {
    value?.as_f64().filter(|f| f.is_finite()).map(|f| f.round().clamp(i32::MIN as f64, i32::MAX as f64) as i32)
}

/// Reads one batch in order. `url` is the page the window was on before the batch. Request and
/// response bodies are moved out of the events: they are stored apart, and only when the project
/// captures bodies.
pub fn extract(events: &mut [Value], mut url: Option<String>) -> Extracted {
    let mut out = Extracted::default();
    let mut last_input: BTreeMap<i32, usize> = BTreeMap::new();
    let mut last_scroll: BTreeMap<i32, usize> = BTreeMap::new();
    for event in events.iter_mut() {
        let Some(ts) = ts_of(event) else { continue };
        let kind = event.get("type").and_then(Value::as_i64).unwrap_or(-1);
        let Some(data) = event.get_mut("data").and_then(Value::as_object_mut) else { continue };
        match kind {
            rr::META => {
                if let Some(Value::String(href)) = data.get_mut("href") {
                    *href = clean_url(href);
                }
                if let (Some(w), Some(h)) = (int(data.get("width")), int(data.get("height"))) {
                    out.screen.get_or_insert((w, h));
                }
            }
            rr::FULL_SNAPSHOT => {}
            rr::INCREMENTAL => incremental(data, ts, &url, &mut out, &mut last_input, &mut last_scroll),
            rr::PLUGIN => {
                if data.get("plugin").and_then(Value::as_str) == Some("rrweb/console@1")
                    && let Some(payload) = data.get("payload")
                {
                    out.console.push(console(payload, ts));
                }
            }
            rr::CUSTOM => {
                let tag = data.get("tag").and_then(Value::as_str).unwrap_or_default().to_string();
                let Some(payload) = data.get_mut("payload").and_then(Value::as_object_mut) else { continue };
                custom(&tag, payload, ts, &mut url, &mut out);
            }
            _ => {}
        }
    }
    out
}

fn incremental(
    data: &Map<String, Value>,
    ts: DateTime<Utc>,
    url: &Option<String>,
    out: &mut Extracted,
    last_input: &mut BTreeMap<i32, usize>,
    last_scroll: &mut BTreeMap<i32, usize>,
) {
    let node_id = int(data.get("id"));
    let base = |kind| Interaction { ts, url: url.clone(), kind, node_id, x: None, y: None, input_length: None };
    match data.get("source").and_then(Value::as_i64) {
        Some(rr::MOUSE_INTERACTION) => {
            let kind = match data.get("type").and_then(Value::as_i64) {
                Some(rr::CLICK) => "click",
                Some(rr::DBL_CLICK) => "dblclick",
                Some(rr::CONTEXT_MENU) => "contextmenu",
                _ => return,
            };
            out.interactions.push(Interaction { x: int(data.get("x")), y: int(data.get("y")), ..base(kind) });
        }
        // Typing sends one event per key. One row per field and batch says the same.
        Some(rr::INPUT) => {
            let length = data.get("text").and_then(Value::as_str).map(|t| t.chars().count() as i32);
            let row = Interaction { input_length: length, ..base("input") };
            upsert(&mut out.interactions, last_input, node_id, row);
        }
        Some(rr::SCROLL) => {
            let row = Interaction { x: int(data.get("x")), y: int(data.get("y")), ..base("scroll") };
            upsert(&mut out.interactions, last_scroll, node_id, row);
        }
        _ => {}
    }
}

fn upsert(rows: &mut Vec<Interaction>, last: &mut BTreeMap<i32, usize>, node_id: Option<i32>, row: Interaction) {
    let Some(id) = node_id else { return rows.push(row) };
    match last.get(&id) {
        Some(&i) => rows[i] = Interaction { ts: rows[i].ts, ..row },
        None => {
            last.insert(id, rows.len());
            rows.push(row);
        }
    }
}

fn console(payload: &Value, ts: DateTime<Utc>) -> Console {
    let level = match payload.get("level").and_then(Value::as_str) {
        Some("info") => "info",
        Some("warn") => "warn",
        Some("error" | "assert") => "error",
        Some("debug" | "trace") => "debug",
        _ => "log",
    };
    let parts: Vec<String> = payload
        .get("payload")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .map(|v| match v {
                    // The console plugin sends each argument JSON-encoded; a string arrives quoted.
                    Value::String(s) => serde_json::from_str::<String>(s).unwrap_or_else(|_| s.clone()),
                    other => other.to_string(),
                })
                .collect()
        })
        .unwrap_or_default();
    let stack = payload.get("trace").and_then(Value::as_array).map(|t| t.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"));
    Console { ts, level, message: truncate(&parts.join(" "), TEXT), stack: stack.filter(|s| !s.is_empty()).map(|s| truncate(&s, STACK)) }
}

/// The SDK's own events, all tagged `trail/...`.
fn custom(tag: &str, p: &mut Map<String, Value>, ts: DateTime<Utc>, url: &mut Option<String>, out: &mut Extracted) {
    let clean = |p: &mut Map<String, Value>, field: &str| -> Option<String> {
        let cleaned = text(p.get(field), 2048).map(|u| clean_url(&u));
        if let Some(u) = &cleaned {
            p.insert(field.into(), Value::String(u.clone()));
        }
        cleaned
    };
    match tag {
        "trail/page" => {
            if let Some(page) = clean(p, "url") {
                *url = Some(page.clone());
                out.pages.push(Page { ts, url: page, title: text(p.get("title"), NAME) });
            }
        }
        "trail/el" => {
            if let Some(id) = int(p.get("id")) {
                out.elements.insert(
                    id,
                    Element {
                        role: text(p.get("role"), 50),
                        name: text(p.get("name"), NAME),
                        testid: text(p.get("testid"), NAME),
                        component: text(p.get("component"), NAME),
                    },
                );
            }
        }
        "trail/net" => {
            let Some(request_url) = clean(p, "url") else { return };
            let initiator = match p.get("initiator").and_then(Value::as_str) {
                Some("xhr") => "xhr",
                Some("resource") => "resource",
                _ => "fetch",
            };
            let request_body = p.remove("requestBody").and_then(|v| text(Some(&v), BODY));
            let response_body = p.remove("responseBody").and_then(|v| text(Some(&v), BODY));
            out.network.push(Network {
                ts,
                initiator,
                method: text(p.get("method"), 16).unwrap_or_else(|| "GET".into()).to_ascii_uppercase(),
                url: request_url,
                status: int(p.get("status")).unwrap_or(0).clamp(0, 999),
                duration_ms: int(p.get("durationMs")).map(|d| d.max(0)),
                bytes: p.get("bytes").and_then(Value::as_i64).filter(|b| *b >= 0),
                request_body,
                response_body,
            });
        }
        "trail/error" => {
            let kind = if p.get("kind").and_then(Value::as_str) == Some("unhandledrejection") { "unhandledrejection" } else { "error" };
            out.errors.push(Error {
                ts,
                kind,
                message: text(p.get("message"), TEXT).unwrap_or_else(|| "(no message)".into()),
                stack: text(p.get("stack"), STACK),
                url: url.clone(),
            });
        }
        "trail/vital" => {
            let name = match p.get("name").and_then(Value::as_str) {
                Some("LCP") => "LCP",
                Some("INP") => "INP",
                Some("CLS") => "CLS",
                Some("FCP") => "FCP",
                Some("TTFB") => "TTFB",
                _ => return,
            };
            if let Some(value) = p.get("value").and_then(Value::as_f64).filter(|v| v.is_finite()) {
                out.vitals.push(Vital { ts, url: url.clone(), name, value });
            }
        }
        "trail/custom" => {
            let Some(name) = text(p.get("name"), NAME) else { return };
            let props = p.get("props").filter(|v| v.is_object() && v.to_string().len() <= PROPS).cloned();
            out.custom.push(Custom { ts, name, props });
        }
        "trail/identify" => {
            out.user_ref = text(p.get("userRef"), NAME);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_batch_becomes_rows() {
        let mut events = vec![
            json!({ "type": 4, "timestamp": 1000, "data": { "href": "https://a.test/?token=x", "width": 1280, "height": 720 } }),
            json!({ "type": 5, "timestamp": 1001, "data": { "tag": "trail/page", "payload": { "url": "https://a.test/cart?session=s", "title": "Cart" } } }),
            json!({ "type": 5, "timestamp": 1002, "data": { "tag": "trail/el", "payload": { "id": 7, "role": "button", "name": "Pay", "testid": "pay" } } }),
            json!({ "type": 3, "timestamp": 1003, "data": { "source": 2, "type": 2, "id": 7, "x": 10.4, "y": 20 } }),
            json!({ "type": 3, "timestamp": 1004, "data": { "source": 5, "id": 9, "text": "*" } }),
            json!({ "type": 3, "timestamp": 1005, "data": { "source": 5, "id": 9, "text": "****" } }),
            json!({ "type": 6, "timestamp": 1006, "data": { "plugin": "rrweb/console@1", "payload": { "level": "warn", "trace": ["at x (a.js:1:1)"], "payload": ["\"low stock\"", "3"] } } }),
            json!({ "type": 5, "timestamp": 1007, "data": { "tag": "trail/net", "payload": { "initiator": "fetch", "method": "post", "url": "/api/pay?key=k", "status": 500, "durationMs": 120, "requestBody": "{\"a\":1}" } } }),
            json!({ "type": 5, "timestamp": 1008, "data": { "tag": "trail/error", "payload": { "kind": "error", "message": "boom", "stack": "Error: boom" } } }),
            json!({ "type": 5, "timestamp": 1009, "data": { "tag": "trail/vital", "payload": { "name": "LCP", "value": 1234.5 } } }),
            json!({ "type": 5, "timestamp": 1010, "data": { "tag": "trail/custom", "payload": { "name": "checkout", "props": { "step": 2 } } } }),
            json!({ "type": 5, "timestamp": 1011, "data": { "tag": "trail/identify", "payload": { "userRef": "u-42" } } }),
        ];
        let x = extract(&mut events, None);
        assert_eq!(events[0]["data"]["href"], "https://a.test/?token=****");
        assert_eq!(x.screen, Some((1280, 720)));
        assert_eq!(x.pages[0].url, "https://a.test/cart?session=****");
        assert_eq!(x.elements[&7].name.as_deref(), Some("Pay"));
        assert_eq!(x.interactions.len(), 2, "one click and one coalesced input");
        assert_eq!((x.interactions[0].kind, x.interactions[0].x), ("click", Some(10)));
        assert_eq!((x.interactions[1].kind, x.interactions[1].input_length), ("input", Some(4)));
        assert_eq!(x.interactions[1].url.as_deref(), Some("https://a.test/cart?session=****"));
        assert_eq!((x.console[0].level, x.console[0].message.as_str()), ("warn", "low stock 3"));
        let net = &x.network[0];
        assert_eq!((net.method.as_str(), net.url.as_str(), net.status), ("POST", "/api/pay?key=****", 500));
        assert_eq!(net.request_body.as_deref(), Some("{\"a\":1}"));
        assert!(events[7]["data"]["payload"].get("requestBody").is_none(), "bodies leave the replay");
        assert_eq!(x.errors[0].url.as_deref(), Some("https://a.test/cart?session=****"));
        assert_eq!((x.vitals[0].name, x.vitals[0].value), ("LCP", 1234.5));
        assert_eq!(x.custom[0].props, Some(json!({ "step": 2 })));
        assert_eq!(x.user_ref.as_deref(), Some("u-42"));
    }
}
