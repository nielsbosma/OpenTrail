use axum::http::Method;
use serde_json::{Map, Value, json};

use crate::AppState;
use crate::rest::ROUTES;

const INTRO: &str = "Every call takes an API key as `Authorization: Bearer <key>`. Create keys under Account, API keys; \
a key can do what its scopes allow and never more than its owner's role.\n\n\
Writes accept an `Idempotency-Key` header: a retry with the same key and body gets the first response back \
(with `Idempotent-Replayed: true`) instead of acting twice. Keys are kept for a day.\n\n\
Errors are `{ \"error\": \"<message>\", \"kind\": \"<kind>\" }` with the matching HTTP status.";

/// Moves a schema's `$defs` into `components.schemas` and points its references there.
fn absorb(mut schema: Value, components: &mut Map<String, Value>) -> Value {
    if let Some(defs) = schema.as_object_mut().and_then(|o| o.remove("$defs")).and_then(|d| d.as_object().cloned()) {
        for (name, def) in defs {
            let def = absorb(def, components);
            components.entry(name).or_insert(def);
        }
    }
    if let Some(o) = schema.as_object_mut() {
        o.remove("$schema");
    }
    rewrite_refs(&mut schema);
    schema
}

fn rewrite_refs(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(r)) = map.get_mut("$ref")
                && let Some(name) = r.strip_prefix("#/$defs/")
            {
                *r = format!("#/components/schemas/{name}");
            }
            map.values_mut().for_each(rewrite_refs);
        }
        Value::Array(items) => items.iter_mut().for_each(rewrite_refs),
        _ => {}
    }
}

fn path_params(path: &str) -> Vec<&str> {
    path.split('/').filter_map(|s| s.strip_prefix('{').and_then(|s| s.strip_suffix('}'))).collect()
}

fn error_response(description: &str) -> Value {
    json!({ "description": description, "content": { "application/json": { "schema": { "$ref": "#/components/schemas/Error" } } } })
}

pub fn document(state: &AppState, server: &str) -> Value {
    let mut components = Map::new();
    components.insert(
        "Error".into(),
        json!({ "type": "object", "required": ["error", "kind"], "properties": {
            "error": { "type": "string" },
            "kind": { "type": "string", "enum": ["bad_request", "unauthorized", "forbidden", "not_found", "conflict", "too_many_requests", "unavailable", "internal"] }
        } }),
    );
    let mut paths: Map<String, Value> = Map::new();

    for route in ROUTES {
        let Some(service) = state.services.get(route.service) else { continue };
        let meta = service.meta();
        let input = absorb(serde_json::to_value(service.input_schema()).unwrap_or_default(), &mut components);
        let output = absorb(serde_json::to_value(service.output_schema()).unwrap_or_default(), &mut components);
        let in_path = path_params(route.path);
        let fixed: Vec<&str> = route.fixed.iter().map(|(k, _)| *k).collect();
        let properties = input["properties"].as_object().cloned().unwrap_or_default();
        let required: Vec<String> =
            input["required"].as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();

        let mut parameters: Vec<Value> = in_path
            .iter()
            .map(|p| json!({ "name": p, "in": "path", "required": true, "schema": properties.get(*p).cloned().unwrap_or(json!({ "type": "string" })) }))
            .collect();
        let mut operation = json!({
            "operationId": meta.tool_name(),
            "summary": meta.description,
            "tags": [route.path.split('/').nth(1).unwrap_or("general")],
        });
        let rest: Map<String, Value> =
            properties.into_iter().filter(|(k, _)| !in_path.contains(&k.as_str()) && !fixed.contains(&k.as_str())).collect();
        if route.method == Method::GET {
            for (name, schema) in &rest {
                let description = schema.get("description").cloned().unwrap_or(Value::Null);
                parameters.push(json!({ "name": name, "in": "query", "required": required.contains(name), "schema": schema, "description": description }));
            }
        } else if !rest.is_empty() {
            let body_required: Vec<&String> = required.iter().filter(|r| rest.contains_key(*r)).collect();
            let mut body = input.clone();
            body["properties"] = Value::Object(rest);
            body["required"] = json!(body_required);
            if let Some(o) = body.as_object_mut() {
                o.remove("title");
            }
            operation["requestBody"] = json!({ "required": true, "content": { "application/json": { "schema": body } } });
        }
        if let Some(params) = (!parameters.is_empty()).then_some(parameters) {
            operation["parameters"] = Value::Array(params);
        }
        let ok = if route.creates { "201" } else { "200" };
        let mut responses = Map::new();
        responses.insert(ok.into(), json!({ "description": "OK", "content": { "application/json": { "schema": output } } }));
        responses.insert("4XX".into(), error_response("The request was refused; see `kind`"));
        operation["responses"] = Value::Object(responses);
        paths.entry(route.path.to_string()).or_insert_with(|| json!({}))[route.method.as_str().to_lowercase()] = operation;
    }

    json!({
        "openapi": "3.1.0",
        "info": { "title": "OpenTrail API", "version": opentrail_core::env::app_version(), "description": INTRO },
        "servers": [{ "url": server }],
        "security": [{ "apiKey": [] }],
        "paths": paths,
        "components": {
            "schemas": components,
            "securitySchemes": { "apiKey": { "type": "http", "scheme": "bearer", "description": "An API key from Account, API keys" } }
        }
    })
}
