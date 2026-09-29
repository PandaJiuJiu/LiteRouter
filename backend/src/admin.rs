use crate::auth::check_admin;
use crate::db::now;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::Row;
use std::sync::Arc;

fn row_channel(row: &sqlx::sqlite::SqliteRow) -> Value {
    json!({
        "id": row.get::<i64, _>("id"),
        "name": row.get::<String, _>("name"),
        "base_url": row.get::<String, _>("base_url"),
        "base_url_anthropic": row.get::<String, _>("base_url_anthropic"),
        "api_key": row.get::<String, _>("api_key"),
        "models": row.get::<String, _>("models"),
        "enabled": row.get::<i64, _>("enabled"),
        "kind": row.get::<String, _>("kind"),
        "created_at": row.get::<i64, _>("created_at"),
    })
}

fn row_token(row: &sqlx::sqlite::SqliteRow, show_key: bool) -> Value {
    let mut v = json!({
        "id": row.get::<i64, _>("id"),
        "name": row.get::<String, _>("name"),
        "enabled": row.get::<i64, _>("enabled"),
        "created_at": row.get::<i64, _>("created_at"),
        "accessed_at": row.get::<i64, _>("accessed_at"),
        "rpm_limit": row.get::<i64, _>("rpm_limit"),
        "daily_token_limit": row.get::<i64, _>("daily_token_limit"),
    });
    if show_key {
        v["key"] = json!(row.get::<String, _>("key"));
    }
    v
}

/// GET /api/models — every enabled channel's model list, grouped by channel,
/// so the routing UI can offer a picker instead of free-text input.
pub async fn list_channel_models(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    check_admin(&state, &headers)?;
    let rows = sqlx::query(
        "SELECT name, models FROM channels WHERE enabled=1 ORDER BY id ASC",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let channels: Vec<Value> = rows
        .iter()
        .map(|r| {
            let models: Vec<String> = r
                .get::<String, _>("models")
                .split(',')
                .map(|m| m.trim().to_string())
                .filter(|m| !m.is_empty() && m != "*")
                .collect();
            json!({ "name": r.get::<String, _>("name"), "models": models })
        })
        .filter(|c| !c["models"].as_array().unwrap().is_empty())
        .collect();
    Ok(Json(json!({ "channels": channels })))
}

// ---------- channels ----------

#[derive(Deserialize)]
pub struct ChannelReq {
    pub name: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub base_url_anthropic: String,
    pub api_key: String,
    /// optional: omit to keep the current value on update (model selection
    /// is managed on the models page)
    #[serde(default)]
    pub models: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 'external' (default, serves relay traffic) or 'internal' (excluded
    /// from routing)
    #[serde(default = "default_external")]
    pub kind: Option<String>,
}
fn default_true() -> bool {
    true
}
fn default_external() -> Option<String> {
    Some("external".to_string())
}

#[derive(Deserialize)]
pub struct FetchModelsReq {
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub base_url_anthropic: String,
    pub api_key: String,
}

#[derive(Deserialize)]
pub struct TestModelReq {
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub base_url_anthropic: String,
    pub api_key: String,
    pub model: String,
}

pub async fn list_channels(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    check_admin(&state, &headers)?;
    let rows = sqlx::query("SELECT * FROM channels ORDER BY id DESC")
        .fetch_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({
        "channels": rows.iter().map(row_channel).collect::<Vec<_>>()
    })))
}

/// Only 'internal' is special; anything else (or missing) means 'external'.
fn normalize_kind(kind: Option<&str>) -> String {
    if kind == Some("internal") {
        "internal".to_string()
    } else {
        "external".to_string()
    }
}

pub async fn create_channel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ChannelReq>,
) -> Result<Json<Value>, StatusCode> {
    check_admin(&state, &headers)?;
    if req.base_url.trim().is_empty() && req.base_url_anthropic.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let kind = normalize_kind(req.kind.as_deref());
    sqlx::query("INSERT INTO channels (name, base_url, base_url_anthropic, api_key, models, enabled, kind, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(&req.name)
        .bind(req.base_url.trim_end_matches('/'))
        .bind(req.base_url_anthropic.trim_end_matches('/'))
        .bind(&req.api_key)
        .bind(req.models.unwrap_or_default())
        .bind(req.enabled as i64)
        .bind(&kind)
        .bind(now())
        .execute(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn update_channel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(req): Json<ChannelReq>,
) -> Result<Json<Value>, StatusCode> {
    check_admin(&state, &headers)?;
    if req.base_url.trim().is_empty() && req.base_url_anthropic.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    // keep existing models when the request omits the field
    let models = match req.models {
        Some(m) => m,
        None => sqlx::query("SELECT models FROM channels WHERE id=?")
            .bind(id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .map(|r| r.get::<String, _>("models"))
            .unwrap_or_default(),
    };
    // keep existing kind when the request omits it (older clients)
    let kind = match req.kind {
        Some(ref k) => normalize_kind(Some(k)),
        None => sqlx::query("SELECT kind FROM channels WHERE id=?")
            .bind(id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .map(|r| r.get::<String, _>("kind"))
            .unwrap_or_else(|| "external".to_string()),
    };
    sqlx::query("UPDATE channels SET name=?, base_url=?, base_url_anthropic=?, api_key=?, models=?, enabled=?, kind=? WHERE id=?")
        .bind(&req.name)
        .bind(req.base_url.trim_end_matches('/'))
        .bind(req.base_url_anthropic.trim_end_matches('/'))
        .bind(&req.api_key)
        .bind(&models)
        .bind(req.enabled as i64)
        .bind(&kind)
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn delete_channel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    check_admin(&state, &headers)?;
    sqlx::query("DELETE FROM channels WHERE id=?")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "ok": true })))
}

/// POST /api/channels/fetch-models — fetch model list from an upstream
/// (works on an unsaved channel form: takes base_url + api_key directly).
pub async fn fetch_models(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<FetchModelsReq>,
) -> axum::response::Response {
    if let Err(s) = check_admin(&state, &headers) {
        return (s, Json(json!({ "error": "unauthorized" }))).into_response();
    }
    // try each configured base URL: OpenAI style (Bearer) then Anthropic
    // style (x-api-key), return the first successful response
    let mut last_err = String::new();
    // upstream definitely has no /models endpoint if either attempt returns
    // 404 (or 405). Some providers (e.g. Volcano Ark) return 401 for unknown
    // paths with valid auth, so 401 alone isn't conclusive — only 404/405 are.
    let mut unsupported_endpoint = false;
    let attempts: Vec<(&str, &str)> = vec![
        (req.base_url.trim_end_matches('/'), "bearer"),
        (req.base_url_anthropic.trim_end_matches('/'), "x-api-key"),
    ];
    let mut success = None;
    for (base, style) in attempts {
        if base.is_empty() {
            continue;
        }
        let mut r = state.http.get(format!("{}/models", base));
        if style == "x-api-key" {
            r = r.header("x-api-key", &req.api_key);
        } else {
            r = r.header("Authorization", format!("Bearer {}", req.api_key));
        }
        match r.send().await {
            Ok(resp) if resp.status().is_success() => {
                success = Some(resp);
                break;
            }
            Ok(resp) => {
                let status = resp.status().as_u16();
                last_err = format!("{} -> HTTP {}", base, status);
                if status == 404 || status == 405 {
                    unsupported_endpoint = true;
                }
            }
            Err(e) => last_err = format!("{} -> {}", base, e),
        }
    }
    let resp = match success {
        Some(r) => r,
        None => {
            let msg = if unsupported_endpoint {
                // upstream simply doesn't expose a list-models endpoint
                // (e.g. Volcano Ark, many Chinese providers) — tell the user
                // to use the manual model input instead
                "上游不提供获取模型列表的接口，请使用「手动添加」功能".to_string()
            } else {
                format!("获取模型列表失败（{}）", last_err)
            };
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": msg })),
            )
                .into_response()
        }
    };
    let body: Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "上游返回的不是有效 JSON" })),
            )
                .into_response()
        }
    };
    let models: Vec<String> = body
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|i| i.as_str()))
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();
    Json(json!({ "models": models })).into_response()
}

/// POST /api/channels/test-model — send a minimal request to the upstream to
/// check whether `model` is actually reachable. Tests both the OpenAI-style
/// and the Anthropic-style URL independently, and returns per-protocol results
/// so the UI can show which protocol(s) work for this model.
pub async fn test_model(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<TestModelReq>,
) -> Result<Json<Value>, StatusCode> {
    check_admin(&state, &headers)?;
    if req.model.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let openai_base = req.base_url.trim_end_matches('/');
    let anthropic_base = req.base_url_anthropic.trim_end_matches('/');
    let minimal_body = json!({
        "model": req.model,
        "max_tokens": 1,
        "messages": [{"role": "user", "content": "."}],
    });

    let mut protocols = serde_json::Map::new();

    // OpenAI-compatible attempt
    if !openai_base.is_empty() {
        let url = format!("{}/chat/completions", openai_base);
        let r = state.http
            .post(&url)
            .header("Authorization", format!("Bearer {}", req.api_key))
            .json(&minimal_body);
        let entry = match r.send().await {
            Ok(resp) if resp.status().is_success() => json!({ "ok": true }),
            Ok(resp) => {
                let st = resp.status().as_u16();
                let raw = resp.text().await.unwrap_or_default();
                json!({ "ok": false, "error": format!("HTTP {}: {}", st, extract_error_msg(&raw)) })
            }
            Err(e) => json!({ "ok": false, "error": format!("请求失败: {}", e) }),
        };
        protocols.insert("openai".to_string(), entry);
    }

    // Anthropic-compatible attempt
    if !anthropic_base.is_empty() {
        let url = format!("{}/v1/messages", anthropic_base);
        let r = state.http
            .post(&url)
            .header("x-api-key", &req.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&minimal_body);
        let entry = match r.send().await {
            Ok(resp) if resp.status().is_success() => json!({ "ok": true }),
            Ok(resp) => {
                let st = resp.status().as_u16();
                let raw = resp.text().await.unwrap_or_default();
                json!({ "ok": false, "error": format!("HTTP {}: {}", st, extract_error_msg(&raw)) })
            }
            Err(e) => json!({ "ok": false, "error": format!("请求失败: {}", e) }),
        };
        protocols.insert("anthropic".to_string(), entry);
    }

    let any_ok = protocols
        .values()
        .any(|v| v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false));
    Ok(Json(json!({
        "ok": any_ok,
        "protocols": Value::Object(protocols),
    })))
}

/// Pull a human-readable error message out of an upstream's JSON error body,
/// falling back to a truncated raw string when the schema is unknown.
fn extract_error_msg(raw: &str) -> String {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|v| {
            v.get("error").and_then(|e| {
                e.get("message")
                    .and_then(|m| m.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| e.as_str().map(|s| s.to_string()))
            })
        })
        .unwrap_or_else(|| raw.chars().take(160).collect())
}

// ---------- tokens ----------

#[derive(Deserialize)]
pub struct TokenReq {
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// requests per minute; 0 = unlimited
    #[serde(default)]
    pub rpm_limit: i64,
    /// total tokens per UTC day; 0 = unlimited
    #[serde(default)]
    pub daily_token_limit: i64,
}

/// PATCH-able fields for an existing token (everything except name + key)
#[derive(Deserialize)]
pub struct TokenUpdateReq {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub rpm_limit: i64,
    #[serde(default)]
    pub daily_token_limit: i64,
}

pub async fn list_tokens(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    check_admin(&state, &headers)?;
    let rows = sqlx::query("SELECT * FROM tokens ORDER BY id DESC")
        .fetch_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({
        "tokens": rows.iter().map(|r| row_token(r, true)).collect::<Vec<_>>()
    })))
}

pub async fn create_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<TokenReq>,
) -> Result<Json<Value>, StatusCode> {
    check_admin(&state, &headers)?;
    let key = format!("sk-{}", uuid::Uuid::new_v4().simple());
    sqlx::query("INSERT INTO tokens (name, key, enabled, created_at, rpm_limit, daily_token_limit) VALUES (?, ?, ?, ?, ?, ?)")
        .bind(&req.name)
        .bind(&key)
        .bind(req.enabled as i64)
        .bind(now())
        .bind(req.rpm_limit.max(0))
        .bind(req.daily_token_limit.max(0))
        .execute(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let row = sqlx::query("SELECT * FROM tokens WHERE key=?")
        .bind(&key)
        .fetch_one(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(row_token(&row, true)))
}

pub async fn toggle_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(req): Json<TokenUpdateReq>,
) -> Result<Json<Value>, StatusCode> {
    check_admin(&state, &headers)?;
    sqlx::query(
        "UPDATE tokens SET enabled=?, rpm_limit=?, daily_token_limit=? WHERE id=?",
    )
    .bind(req.enabled as i64)
    .bind(req.rpm_limit.max(0))
    .bind(req.daily_token_limit.max(0))
    .bind(id)
    .execute(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "ok": true })))
}

pub async fn delete_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    check_admin(&state, &headers)?;
    sqlx::query("DELETE FROM tokens WHERE id=?")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "ok": true })))
}

// ---------- model mappings ----------

#[derive(Deserialize)]
pub struct MappingReq {
    pub alias: String,
    /// ordered list of routing targets, tried in array order
    pub targets: Vec<TargetEntry>,
}

/// One routing target: an upstream model, optionally pinned to a channel.
/// Stored in the `targets` column as a JSON array of these objects. Legacy
/// rows (plain model-name strings) are accepted and read as unpinned.
#[derive(Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TargetEntry {
    Obj {
        /// empty = any channel serving this model
        #[serde(default)]
        channel: String,
        /// upstream model id; "*" means every model on the pinned channel
        model: String,
    },
    Str(String),
}

impl TargetEntry {
    fn normalize(entry: TargetEntry, out: &mut Vec<(String, String)>) {
        let (channel, model) = match entry {
            TargetEntry::Obj { channel, model } => (channel.trim().to_string(), model.trim().to_string()),
            TargetEntry::Str(s) => (String::new(), s.trim().to_string()),
        };
        // a wildcard needs a channel to expand against
        if model.is_empty() || (model == "*" && channel.is_empty()) {
            return;
        }
        if !out.iter().any(|(c, m)| c == &channel && m == &model) {
            out.push((channel, model));
        }
    }
}

/// Parse the JSON-array `targets` column into ordered (channel, model)
/// pairs; fall back to the legacy single `target_model` column when the row
/// predates the migration. An empty channel means "any channel".
pub fn parse_targets(targets: &str, fallback: &str) -> Vec<(String, String)> {
    let entries: Vec<TargetEntry> =
        serde_json::from_str(targets).unwrap_or_default();
    let mut out: Vec<(String, String)> = Vec::new();
    for e in entries {
        TargetEntry::normalize(e, &mut out);
    }
    if out.is_empty() && !fallback.trim().is_empty() {
        out.push((String::new(), fallback.trim().to_string()));
    }
    out
}

/// Serialize normalized targets back into the stored JSON form.
fn encode_targets(targets: &[(String, String)]) -> String {
    let entries: Vec<TargetEntry> = targets
        .iter()
        .map(|(channel, model)| TargetEntry::Obj {
            channel: channel.clone(),
            model: model.clone(),
        })
        .collect();
    serde_json::to_string(&entries).unwrap_or_default()
}

fn clean_targets(raw: &[TargetEntry]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for e in raw {
        TargetEntry::normalize(e.clone(), &mut out);
    }
    out
}

pub async fn list_mappings(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    check_admin(&state, &headers)?;
    let rows = sqlx::query("SELECT * FROM model_mappings ORDER BY id ASC")
        .fetch_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mappings: Vec<Value> = rows
        .iter()
        .map(|r| {
            let targets = parse_targets(
                &r.get::<String, _>("targets"),
                &r.get::<String, _>("target_model"),
            );
            json!({
                "id": r.get::<i64, _>("id"),
                "alias": r.get::<String, _>("alias"),
                "targets": targets.iter().map(|(c, m)| json!({ "channel": c, "model": m })).collect::<Vec<_>>(),
                "created_at": r.get::<i64, _>("created_at"),
            })
        })
        .collect();
    Ok(Json(json!({ "mappings": mappings })))
}

pub async fn create_mapping(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<MappingReq>,
) -> Result<Json<Value>, StatusCode> {
    check_admin(&state, &headers)?;
    let targets = clean_targets(&req.targets);
    if req.alias.trim().is_empty() || targets.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    // first target's model also goes into the legacy column so old reads still work
    let res = sqlx::query(
        "INSERT INTO model_mappings (alias, target_model, targets, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(req.alias.trim())
    .bind(targets[0].1.clone())
    .bind(encode_targets(&targets))
    .bind(now())
    .execute(&state.pool)
    .await;
    match res {
        Ok(_) => Ok(Json(json!({ "ok": true }))),
        Err(sqlx::Error::Database(e)) if e.code().as_deref() == Some("20602") => {
            // SQLITE_CONSTRAINT_UNIQUE — alias already exists
            Err(StatusCode::CONFLICT)
        }
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn update_mapping(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(req): Json<MappingReq>,
) -> Result<Json<Value>, StatusCode> {
    check_admin(&state, &headers)?;
    let targets = clean_targets(&req.targets);
    if req.alias.trim().is_empty() || targets.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let res = sqlx::query(
        "UPDATE model_mappings SET alias=?, target_model=?, targets=? WHERE id=?",
    )
    .bind(req.alias.trim())
    .bind(targets[0].1.clone())
    .bind(encode_targets(&targets))
    .bind(id)
    .execute(&state.pool)
    .await;
    match res {
        Ok(_) => Ok(Json(json!({ "ok": true }))),
        Err(sqlx::Error::Database(e)) if e.code().as_deref() == Some("20602") => {
            Err(StatusCode::CONFLICT)
        }
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn delete_mapping(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    check_admin(&state, &headers)?;
    sqlx::query("DELETE FROM model_mappings WHERE id=?")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "ok": true })))
}

// ---------- logs ----------

#[derive(Deserialize)]
pub struct PageQuery {
    #[serde(default = "default_page")]
    pub page: i64,
    #[serde(default = "default_size")]
    pub size: i64,
}
fn default_page() -> i64 {
    1
}
fn default_size() -> i64 {
    50
}

pub async fn list_logs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<PageQuery>,
) -> Result<Json<Value>, StatusCode> {
    check_admin(&state, &headers)?;
    let offset = (q.page - 1).max(0) * q.size;
    let rows = sqlx::query("SELECT * FROM logs ORDER BY id DESC LIMIT ? OFFSET ?")
        .bind(q.size)
        .bind(offset)
        .fetch_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let logs: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "token_name": r.get::<String, _>("token_name"),
                "model": r.get::<String, _>("model"),
                "request_model": r.get::<String, _>("request_model"),
                "channel_name": r.get::<String, _>("channel_name"),
                "status_code": r.get::<i64, _>("status_code"),
                "prompt_tokens": r.get::<i64, _>("prompt_tokens"),
                "completion_tokens": r.get::<i64, _>("completion_tokens"),
                "total_tokens": r.get::<i64, _>("total_tokens"),
                "created_at": r.get::<i64, _>("created_at"),
            })
        })
        .collect();
    Ok(Json(json!({ "logs": logs })))
}

/// GET /api/usage?range=7d — aggregated token usage & request counts,
/// broken down by day / token / model / channel.
#[derive(Deserialize)]
pub struct UsageQuery {
    #[serde(default = "default_range")]
    pub range: i64,
}
fn default_range() -> i64 {
    7
}

pub async fn usage(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<UsageQuery>,
) -> Result<Json<Value>, StatusCode> {
    check_admin(&state, &headers)?;
    let days = q.range.max(1).min(90);
    let since = crate::db::now() - days * 86400;

    // by day
    let day_rows = sqlx::query(
        "SELECT
            (created_at / 86400) * 86400 AS k,
            COUNT(*) AS reqs,
            SUM(prompt_tokens) AS p,
            SUM(completion_tokens) AS c,
            SUM(total_tokens) AS t
         FROM logs WHERE created_at >= ? GROUP BY k ORDER BY k",
    )
    .bind(since)
    .fetch_all(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // by token / model / channel: separate queries (SQLite has no GROUPING SETS)
    let group_q = |col: &str| {
        format!(
            "SELECT {0} AS k, COUNT(*) AS reqs,
                SUM(prompt_tokens) AS p,
                SUM(completion_tokens) AS c,
                SUM(total_tokens) AS t
             FROM logs WHERE created_at >= ? GROUP BY {0} ORDER BY t DESC",
            col
        )
    };
    let token_rows = sqlx::query(&group_q("token_name"))
        .bind(since)
        .fetch_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let model_rows = sqlx::query(&group_q("model"))
        .bind(since)
        .fetch_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let channel_rows = sqlx::query(&group_q("channel_name"))
        .bind(since)
        .fetch_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let totals = sqlx::query(
        "SELECT COUNT(*) AS reqs, COALESCE(SUM(prompt_tokens),0) AS p,
                COALESCE(SUM(completion_tokens),0) AS c,
                COALESCE(SUM(total_tokens),0) AS t
         FROM logs WHERE created_at >= ?",
    )
    .bind(since)
    .fetch_one(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    fn map_group(rows: Vec<sqlx::sqlite::SqliteRow>) -> Vec<Value> {
        rows.into_iter()
            .map(|r| {
                json!({
                    "key": r.get::<String, _>("k"),
                    "requests": r.get::<i64, _>("reqs"),
                    "prompt_tokens": r.get::<i64, _>("p"),
                    "completion_tokens": r.get::<i64, _>("c"),
                    "total_tokens": r.get::<i64, _>("t"),
                })
            })
            .collect()
    }

    // by_day uses unix-day timestamp as the key (integer), not a string
    let by_day: Vec<Value> = day_rows
        .into_iter()
        .map(|r| {
            json!({
                "day": r.get::<i64, _>("k"),
                "requests": r.get::<i64, _>("reqs"),
                "prompt_tokens": r.get::<i64, _>("p"),
                "completion_tokens": r.get::<i64, _>("c"),
                "total_tokens": r.get::<i64, _>("t"),
            })
        })
        .collect();

    Ok(Json(json!({
        "range_days": days,
        "totals": {
            "requests": totals.get::<i64, _>("reqs"),
            "prompt_tokens": totals.get::<i64, _>("p"),
            "completion_tokens": totals.get::<i64, _>("c"),
            "total_tokens": totals.get::<i64, _>("t"),
        },
        "by_day": by_day,
        "by_token": map_group(token_rows),
        "by_model": map_group(model_rows),
        "by_channel": map_group(channel_rows),
    })))
}
