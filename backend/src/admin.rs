use crate::auth::{check_auth, require_admin, AuthUser};
use crate::db::{self, now};
use crate::proxy;
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};
use std::sync::Arc;
use std::time::Duration;

fn row_channel(row: &sqlx::sqlite::SqliteRow) -> Value {
    json!({
        "id": row.get::<i64, _>("id"),
        "name": row.get::<String, _>("name"),
        "website": row.get::<String, _>("website"),
        "base_url": row.get::<String, _>("base_url"),
        "base_url_anthropic": row.get::<String, _>("base_url_anthropic"),
        "api_key": row.get::<String, _>("api_key"),
        "models": row.get::<String, _>("models"),
        "disabled_models": row.get::<String, _>("disabled_models"),
        "enabled": row.get::<i64, _>("enabled"),
        "created_at": row.get::<i64, _>("created_at"),
    })
}

fn row_token(row: &sqlx::sqlite::SqliteRow, show_key: bool, owner: Option<&str>) -> Value {
    let mut v = json!({
        "id": row.get::<i64, _>("id"),
        "name": row.get::<String, _>("name"),
        "enabled": row.get::<i64, _>("enabled"),
        "created_at": row.get::<i64, _>("created_at"),
        "accessed_at": row.get::<i64, _>("accessed_at"),
        "rpm_limit": row.get::<i64, _>("rpm_limit"),
        "daily_token_limit": row.get::<i64, _>("daily_token_limit"),
        "user_id": row.get::<Option<i64>, _>("user_id"),
    });
    if let Some(o) = owner {
        v["owner"] = json!(o);
    }
    if show_key {
        v["key"] = json!(row.get::<String, _>("key"));
    }
    v
}

/// All token-name values that belong to the given user, plus a sentinel when
/// the user is admin (we use a special flag in the SQL instead so the
/// query stays a single roundtrip — this helper exists only for log/usage
/// filtering where the SQL needs an explicit list).
async fn token_names_for_user(
    pool: &sqlx::SqlitePool,
    user_id: i64,
    include_orphans: bool,
) -> Vec<String> {
    let q = if include_orphans {
        "SELECT name FROM tokens WHERE user_id = ? OR user_id IS NULL"
    } else {
        "SELECT name FROM tokens WHERE user_id = ?"
    };
    sqlx::query_scalar(q)
        .bind(user_id)
        .fetch_all(pool)
        .await
        .unwrap_or_default()
}

/// GET /api/models — every enabled channel's model list, grouped by channel.
/// Admin-only: the channel view exposes infrastructure details.
pub async fn list_channel_models(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    require_admin(&state, &headers)?;
    let rows = sqlx::query("SELECT name, models FROM channels WHERE enabled=1 ORDER BY id ASC")
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
    /// Optional upstream's official website, shown read-only in the UI.
    /// Never used for routing; purely informational.
    #[serde(default)]
    pub website: String,
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
}
fn default_true() -> bool {
    true
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

/// Body for `POST /api/channels/:id/models` — set the enabled + disabled
/// model lists on a single channel without touching any other column.
/// Used by the models-management page so it never accidentally clobbers
/// website / base_url / api_key. The proxy only reads `models` for routing;
/// `disabled_models` is kept around so disabled cards survive a refresh.
#[derive(Deserialize)]
pub struct UpdateChannelModelsReq {
    pub models: String,
    pub disabled_models: String,
}

pub async fn list_channels(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    require_admin(&state, &headers)?;
    let rows = sqlx::query("SELECT * FROM channels ORDER BY id DESC")
        .fetch_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({
        "channels": rows.iter().map(row_channel).collect::<Vec<_>>()
    })))
}

pub async fn create_channel(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ChannelReq>,
) -> Result<Json<Value>, StatusCode> {
    require_admin(&state, &headers)?;
    if req.base_url.trim().is_empty() && req.base_url_anthropic.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    sqlx::query("INSERT INTO channels (name, website, base_url, base_url_anthropic, api_key, models, enabled, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(&req.name)
        .bind(req.website.trim())
        .bind(req.base_url.trim_end_matches('/'))
        .bind(req.base_url_anthropic.trim_end_matches('/'))
        .bind(&req.api_key)
        .bind(req.models.unwrap_or_default())
        .bind(req.enabled as i64)
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
    require_admin(&state, &headers)?;
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
    sqlx::query("UPDATE channels SET name=?, website=?, base_url=?, base_url_anthropic=?, api_key=?, models=?, enabled=? WHERE id=?")
        .bind(&req.name)
        .bind(req.website.trim())
        .bind(req.base_url.trim_end_matches('/'))
        .bind(req.base_url_anthropic.trim_end_matches('/'))
        .bind(&req.api_key)
        .bind(&models)
        .bind(req.enabled as i64)
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
    require_admin(&state, &headers)?;
    sqlx::query("DELETE FROM channels WHERE id=?")
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(json!({ "ok": true })))
}

/// POST /api/channels/:id/models — set the models list on a single channel
/// without touching any other column. The full-PUT `update_channel` makes
/// it too easy for the models page to clobber website / base_url / api_key
/// when it only meant to flip a model switch.
pub async fn update_channel_models(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(req): Json<UpdateChannelModelsReq>,
) -> Result<Json<Value>, StatusCode> {
    require_admin(&state, &headers)?;
    sqlx::query("UPDATE channels SET models=?, disabled_models=? WHERE id=?")
        .bind(&req.models)
        .bind(&req.disabled_models)
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
    if let Err(s) = require_admin(&state, &headers) {
        return (s, Json(json!({ "error": "unauthorized" }))).into_response();
    }
    // try each configured base URL: OpenAI style (Bearer) then Anthropic
    // style (x-api-key), return the first successful response
    let mut last_err = String::new();
    // Same budget as the model test below, for the same reason: this is a
    // click-and-wait admin action, and the shared client's 600s would leave
    // the dialog spinning for ten minutes if an upstream hangs.
    let timeout = model_test_timeout(&state.pool).await;
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
        match r.timeout(timeout).send().await {
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
            // Distinguish a timeout from a connect failure: "the upstream is
            // slow" and "the upstream is unreachable" call for different
            // admin reactions, and reqwest's `Display` alone doesn't say
            // which happened.
            Err(e) if e.is_timeout() => {
                last_err = format!("{} -> 请求超时（> {}s）", base, timeout.as_secs());
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
            return (StatusCode::BAD_GATEWAY, Json(json!({ "error": msg }))).into_response();
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
    require_admin(&state, &headers)?;
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
    let timeout = model_test_timeout(&state.pool).await;

    let mut protocols = serde_json::Map::new();

    // OpenAI-compatible attempt
    if !openai_base.is_empty() {
        let url = format!("{}/chat/completions", openai_base);
        let r = state
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", req.api_key))
            .json(&minimal_body);
        protocols.insert("openai".to_string(), probe(r, timeout).await);
    }

    // Anthropic-compatible attempt
    if !anthropic_base.is_empty() {
        let url = format!("{}/v1/messages", anthropic_base);
        let r = state
            .http
            .post(&url)
            .header("x-api-key", &req.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&minimal_body);
        protocols.insert("anthropic".to_string(), probe(r, timeout).await);
    }

    let any_ok = protocols
        .values()
        .any(|v| v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false));
    Ok(Json(json!({
        "ok": any_ok,
        "protocols": Value::Object(protocols),
    })))
}

/// Fallback when the `model_test_timeout_secs` row is missing or unusable.
const DEFAULT_TEST_PROBE_TIMEOUT_SECS: u64 = 10;

/// How long a single upstream click-and-wait action may take before it's
/// called a failure. Shared by `test_model` and `fetch_models` — both are
/// admin-UI actions that block on one upstream, so one setting covers both.
///
/// Read per request rather than baked into a const so a slow upstream can be
/// given more headroom without a rebuild. The `settings` row is optional and
/// falls back to [`DEFAULT_TEST_PROBE_TIMEOUT_SECS`].
///
/// The shared client in `AppState` allows 600s, which is right for a real
/// relay but useless for a click-and-wait UI: a hung upstream would leave the
/// card spinning for ten minutes.
async fn model_test_timeout(pool: &SqlitePool) -> Duration {
    let secs = db::get_setting(pool, "model_test_timeout_secs")
        .await
        .ok()
        .flatten()
        .and_then(|v| v.parse::<u64>().ok())
        // 0 would time out every probe instantly — treat it as "unset"
        // rather than as a request to disable testing.
        .filter(|&s| s > 0)
        .unwrap_or(DEFAULT_TEST_PROBE_TIMEOUT_SECS);
    Duration::from_secs(secs)
}

/// Fire one minimal request and describe the outcome for the model card:
/// `{ ok, ms }` on success, `{ ok: false, ms, error }` otherwise.
///
/// `ms` is wall-clock from send to response headers read, so the UI can show a
/// latency next to the checkmark instead of a bare "Available".
async fn probe(r: reqwest::RequestBuilder, timeout: Duration) -> Value {
    let started = std::time::Instant::now();
    let outcome = match r.timeout(timeout).send().await {
        Ok(resp) if resp.status().is_success() => Ok(()),
        Ok(resp) => {
            let st = resp.status().as_u16();
            let raw = resp.text().await.unwrap_or_default();
            Err(format!("HTTP {}: {}", st, extract_error_msg(&raw)))
        }
        Err(e) if e.is_timeout() => Err(format!("请求超时（> {}s）", timeout.as_secs())),
        Err(e) => Err(format!("请求失败: {}", e)),
    };
    let ms = started.elapsed().as_millis();
    match outcome {
        Ok(()) => json!({ "ok": true, "ms": ms }),
        Err(e) => json!({ "ok": false, "ms": ms, "error": e }),
    }
}

/// Pull a human-readable error message out of an upstream's JSON error body,
/// falling back to a truncated raw string when the schema is unknown.
///
/// Shared with the relay path (`proxy.rs`) so the circuit breaker records
/// the upstream's own wording — `HTTP 429: Rate limit exceeded` — rather
/// than a bare status code.
pub(crate) fn extract_error_msg(raw: &str) -> String {
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
    /// admin-only: assign to a specific user instead of the caller
    #[serde(default)]
    pub user_id: Option<i64>,
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

/// Verify the caller is allowed to manage this token. Admins can manage any;
/// regular users only their own.
async fn authorize_token(
    state: &AppState,
    user: AuthUser,
    token_id: i64,
) -> Result<i64, StatusCode> {
    let owner: Option<i64> = sqlx::query_scalar("SELECT user_id FROM tokens WHERE id = ?")
        .bind(token_id)
        .fetch_optional(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .flatten();
    match owner {
        None => Err(StatusCode::NOT_FOUND),
        Some(uid) if user.is_admin || uid == user.id => Ok(uid),
        Some(_) => Err(StatusCode::FORBIDDEN),
    }
}

pub async fn list_tokens(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    let user = check_auth(&state, &headers)?;
    let rows = if user.is_admin {
        sqlx::query(
            "SELECT t.*, u.username AS owner_name
             FROM tokens t LEFT JOIN users u ON t.user_id = u.id
             ORDER BY t.id DESC",
        )
        .fetch_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    } else {
        sqlx::query(
            "SELECT t.*, u.username AS owner_name
             FROM tokens t LEFT JOIN users u ON t.user_id = u.id
             WHERE t.user_id = ?
             ORDER BY t.id DESC",
        )
        .bind(user.id)
        .fetch_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    };
    let tokens: Vec<Value> = rows
        .iter()
        .map(|r| {
            let owner = r.try_get::<Option<String>, _>("owner_name").ok().flatten();
            row_token(r, true, owner.as_deref())
        })
        .collect();
    Ok(Json(json!({ "tokens": tokens })))
}

pub async fn create_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<TokenReq>,
) -> Result<Json<Value>, StatusCode> {
    let user = check_auth(&state, &headers)?;
    let owner_id = if let Some(uid) = req.user_id {
        if !user.is_admin {
            return Err(StatusCode::FORBIDDEN);
        }
        uid
    } else {
        user.id
    };
    let key = format!("sk-{}", uuid::Uuid::new_v4().simple());
    sqlx::query("INSERT INTO tokens (name, key, enabled, created_at, rpm_limit, daily_token_limit, user_id) VALUES (?, ?, ?, ?, ?, ?, ?)")
        .bind(&req.name)
        .bind(&key)
        .bind(req.enabled as i64)
        .bind(now())
        .bind(req.rpm_limit.max(0))
        .bind(req.daily_token_limit.max(0))
        .bind(owner_id)
        .execute(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let row = sqlx::query(
        "SELECT t.*, u.username AS owner_name
         FROM tokens t LEFT JOIN users u ON t.user_id = u.id
         WHERE t.key = ?",
    )
    .bind(&key)
    .fetch_one(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let owner = row
        .try_get::<Option<String>, _>("owner_name")
        .ok()
        .flatten();
    Ok(Json(row_token(&row, true, owner.as_deref())))
}

pub async fn toggle_token(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
    Json(req): Json<TokenUpdateReq>,
) -> Result<Json<Value>, StatusCode> {
    let user = check_auth(&state, &headers)?;
    authorize_token(&state, user, id).await?;
    sqlx::query("UPDATE tokens SET enabled=?, rpm_limit=?, daily_token_limit=? WHERE id=?")
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
    let user = check_auth(&state, &headers)?;
    authorize_token(&state, user, id).await?;
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
            TargetEntry::Obj { channel, model } => {
                (channel.trim().to_string(), model.trim().to_string())
            }
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
    let entries: Vec<TargetEntry> = serde_json::from_str(targets).unwrap_or_default();
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
    require_admin(&state, &headers)?;
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
    require_admin(&state, &headers)?;
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
        Err(ref e) if crate::db::is_unique_violation(e) => {
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
    require_admin(&state, &headers)?;
    let targets = clean_targets(&req.targets);
    if req.alias.trim().is_empty() || targets.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let res =
        sqlx::query("UPDATE model_mappings SET alias=?, target_model=?, targets=? WHERE id=?")
            .bind(req.alias.trim())
            .bind(targets[0].1.clone())
            .bind(encode_targets(&targets))
            .bind(id)
            .execute(&state.pool)
            .await;
    match res {
        Ok(_) => Ok(Json(json!({ "ok": true }))),
        Err(ref e) if crate::db::is_unique_violation(e) => Err(StatusCode::CONFLICT),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

pub async fn delete_mapping(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    require_admin(&state, &headers)?;
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
    /// Time window in hours. Default 1h — the log list is a debugging view,
    /// and "what just happened" is the overwhelmingly common question.
    /// `0` means no window (show everything).
    #[serde(default = "default_range_hours")]
    pub range: i64,
    // Optional filters, all combined with AND. The four text ones are
    // case-insensitive substring matches so the user can paste a fragment of an
    // IP or a model family; `status` is an exact match on the status code.
    /// Substring of `client_ip`. Empty string means "no filter" — the UI sends
    /// its inputs verbatim, so an untouched box arrives as "".
    #[serde(default)]
    pub ip: String,
    /// Substring of `token_name`.
    #[serde(default)]
    pub token: String,
    /// Substring of the requested model (see `COALESCE` in the WHERE clause —
    /// rows predating migration 0009 have it empty and keep `model`).
    #[serde(default)]
    pub model: String,
    /// Substring of the forwarded upstream model, with the same fallback.
    #[serde(default)]
    pub upstream_model: String,
    /// Exact status code. `Option` rather than `i64` because `0` is not a
    /// status code but *is* what an empty input parses to.
    #[serde(default)]
    pub status: Option<i64>,
}
fn default_page() -> i64 {
    1
}
fn default_size() -> i64 {
    20
}
fn default_range_hours() -> i64 {
    1
}

/// One `?` in the log list's WHERE clause, tagged with the type sqlx has to
/// bind it as. The clause mixes TEXT columns (token names, LIKE fragments) with
/// INTEGER ones (the window bound, the status code), so a single homogeneous
/// bind vec can't carry them — and since the count and page queries share the
/// clause, they share this vec too.
enum LogBind {
    Text(String),
    Int(i64),
}

/// Apply the clause's binds to a query. A macro rather than a function because
/// sqlx's `Query` and `QueryScalar` are unrelated types with structurally
/// identical `bind` methods — no trait bound unifies them, and the log list
/// needs both (COUNT and SELECT share one WHERE clause).
macro_rules! bind_clause {
    ($query:expr, $binds:expr) => {{
        let mut q = $query;
        for b in $binds {
            q = match b {
                LogBind::Text(s) => q.bind(s.as_str()),
                LogBind::Int(i) => q.bind(*i),
            };
        }
        q
    }};
}

/// Escape the LIKE wildcards in a user-supplied fragment, so filtering by
/// `100%` matches a literal percent sign rather than turning the `%` into a
/// wildcard. Paired with `ESCAPE '\'` at the call site.
fn like_fragment(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// The four text filter params and the column each one matches, in the order
/// `PageQuery` declares them. Shared by the WHERE builder and the
/// `/api/logs/filter-options` handler so a dropdown can only ever offer values
/// the corresponding filter is able to match — including the `COALESCE`
/// fallbacks, so the offered models are the ones that column really holds.
const LOG_FILTER_COLUMNS: [(&str, &str); 4] = [
    ("ip", "client_ip"),
    ("token", "token_name"),
    ("model", "COALESCE(NULLIF(request_model,''), model)"),
    (
        "upstream_model",
        "COALESCE(NULLIF(upstream_model,''), model)",
    ),
];

/// One filter's value from the query, blank-filtered. `None` means "this box is
/// empty, contribute no clause".
fn log_filter_value<'a>(q: &'a PageQuery, key: &str) -> Option<&'a str> {
    let raw = match key {
        "ip" => q.ip.trim(),
        "token" => q.token.trim(),
        "model" => q.model.trim(),
        "upstream_model" => q.upstream_model.trim(),
        _ => return None,
    };
    (!raw.is_empty()).then_some(raw)
}

/// Build the log list's WHERE clause and its binds: visibility scope, time
/// window, and the caller's filters. Shared by `list_logs` and
/// `log_filter_options` so the dropdowns and the list can never disagree about
/// what "the current filters" means.
///
/// `skip` drops one filter from the clause. The options endpoint uses it to
/// compute each dropdown independently — otherwise narrowing by token would
/// leave the token dropdown showing only the value already picked, with no way
/// to change it.
fn log_where_clause(
    user: &AuthUser,
    allowed: &[String],
    range: i64,
    q: &PageQuery,
    skip: Option<&str>,
) -> (String, Vec<LogBind>) {
    let since = if range > 0 {
        Some(crate::db::now() - range.min(24 * 365) * 3600)
    } else {
        None
    };
    // Admin sees every log; a non-admin is limited to their own tokens. An
    // empty `allowed` collapses to `1=0` because `token IN ()` is a syntax
    // error in SQLite.
    let mut where_parts: Vec<String> = Vec::new();
    // One entry per `?`, in clause order — see `bind_clause`.
    let mut binds: Vec<LogBind> = Vec::new();
    if !user.is_admin {
        if allowed.is_empty() {
            where_parts.push("1=0".to_string());
        } else {
            let ph = std::iter::repeat_n("?", allowed.len())
                .collect::<Vec<_>>()
                .join(",");
            where_parts.push(format!("token_name IN ({ph})"));
            binds.extend(allowed.iter().cloned().map(LogBind::Text));
        }
    }
    if let Some(s) = since {
        where_parts.push("created_at >= ?".to_string());
        binds.push(LogBind::Int(s));
    }
    // The user's filters, ANDed onto the visibility + window clauses.
    for (key, column) in LOG_FILTER_COLUMNS {
        if Some(key) == skip {
            continue;
        }
        if let Some(needle) = log_filter_value(q, key) {
            where_parts.push(format!("{column} LIKE ? ESCAPE '\\'"));
            binds.push(LogBind::Text(format!("%{}%", like_fragment(needle))));
        }
    }
    if let Some(status) = q.status {
        where_parts.push("status_code = ?".to_string());
        binds.push(LogBind::Int(status));
    }
    let where_sql = if where_parts.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", where_parts.join(" AND "))
    };
    (where_sql, binds)
}

pub async fn list_logs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<PageQuery>,
) -> Result<Json<Value>, StatusCode> {
    let user = check_auth(&state, &headers)?;
    let offset = (q.page - 1).max(0) * q.size;
    let allowed = token_names_for_user(&state.pool, user.id, user.is_admin).await;

    // One visibility clause, shared by the count and the page query — if they
    // ever diverge the total stops matching the rows (and for non-admins, a
    // mismatched count leaks the existence of other users' logs). The time
    // window belongs in that same clause: the total in the pager has to be the
    // total *within the selected range*.
    let (where_sql, binds) = log_where_clause(&user, &allowed, q.range, &q, None);

    let count_sql = format!("SELECT COUNT(*) FROM logs{where_sql}");
    let total: i64 = bind_clause!(sqlx::query_scalar(&count_sql), &binds)
        .fetch_one(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let page_sql = format!("SELECT * FROM logs{where_sql} ORDER BY id DESC LIMIT ? OFFSET ?");
    let rows = bind_clause!(sqlx::query(&page_sql), &binds)
        .bind(q.size)
        .bind(offset)
        .fetch_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    // The list shows a "failed N times" badge whose tooltip lists the failed
    // hops. Fetch every attempt for the visible page in one query and group in
    // Rust — a per-row lookup would be N+1 on the hottest endpoint.
    let ids: Vec<i64> = rows.iter().map(|r| r.get::<i64, _>("id")).collect();
    let mut failed_by_log: std::collections::HashMap<i64, Vec<Value>> =
        std::collections::HashMap::new();
    if !ids.is_empty() {
        let ph = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT log_id, upstream_model, channel_name, status_code, error \
             FROM log_attempts WHERE ok=0 AND skipped=0 AND log_id IN ({ph}) ORDER BY seq ASC"
        );
        let mut query = sqlx::query(&sql);
        for id in &ids {
            query = query.bind(id);
        }
        let attempt_rows = query
            .fetch_all(&state.pool)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        for a in attempt_rows {
            failed_by_log
                .entry(a.get::<i64, _>("log_id"))
                .or_default()
                .push(json!({
                    "upstream_model": a.get::<String, _>("upstream_model"),
                    "channel_name": a.get::<String, _>("channel_name"),
                    "status_code": a.get::<i64, _>("status_code"),
                    "error": a.get::<String, _>("error"),
                }));
        }
    }
    let logs: Vec<Value> = rows
        .iter()
        .map(|r| {
            let upstream_model = r.get::<String, _>("upstream_model");
            let upstream_model = if upstream_model.is_empty() {
                r.get::<String, _>("model")
            } else {
                upstream_model
            };
            let id = r.get::<i64, _>("id");
            let failed_count: i64 = r.try_get("failed_count").unwrap_or(0);
            json!({
                "id": id,
                "token_name": r.get::<String, _>("token_name"),
                "request_model": r.get::<String, _>("request_model"),
                "upstream_model": upstream_model,
                "channel_name": r.get::<String, _>("channel_name"),
                "status_code": r.get::<i64, _>("status_code"),
                "prompt_tokens": r.get::<i64, _>("prompt_tokens"),
                "completion_tokens": r.get::<i64, _>("completion_tokens"),
                "total_tokens": r.get::<i64, _>("total_tokens"),
                "failed_count": failed_count,
                "failed_attempts": failed_by_log.get(&id).cloned().unwrap_or_default(),
                "created_at": r.get::<i64, _>("created_at"),
                "client_ip": r.get::<String, _>("client_ip"),
                "user_agent": r.get::<String, _>("user_agent"),
            })
        })
        .collect();
    Ok(Json(json!({ "logs": logs, "total": total })))
}

/// How many distinct value combinations `/api/logs/filter-options` reads before
/// giving up. The dropdowns are built from the combinations actually present
/// under the current filters, and the cap keeps an all-time window over a
/// busy month from materializing hundreds of thousands of rows. Past the cap
/// the lists are simply shorter — every value offered still matches.
const LOG_OPTION_ROW_CAP: usize = 20_000;

/// GET /api/logs/filter-options — the values each of the five log filters can
/// be set to, so the UI can offer dropdowns instead of free-text boxes.
///
/// Takes the same query string as `/api/logs` (it reuses `PageQuery`) and
/// answers "given the filters applied *except this one*, what could I pick?"
/// — the facet rule, so narrowing by token doesn't leave the token dropdown
/// holding only the value already chosen. `page`/`size` are accepted and
/// ignored; sharing the struct is cheaper than maintaining a second parser
/// that could drift.
///
/// Scoped to the caller's visibility exactly like the list, so a non-admin
/// can't enumerate other users' token names or model names through it.
pub async fn log_filter_options(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<PageQuery>,
) -> Result<Json<Value>, StatusCode> {
    let user = check_auth(&state, &headers)?;
    let allowed = token_names_for_user(&state.pool, user.id, user.is_admin).await;

    // Five clauses, five DISTINCT scans. Each dropdown has to be computed
    // without its own filter, and a single grouped pass can't answer that —
    // the row sets differ per facet.
    let mut out = serde_json::Map::new();
    for (key, column) in LOG_FILTER_COLUMNS {
        let (where_sql, binds) = log_where_clause(&user, &allowed, q.range, &q, Some(key));
        let sql = format!(
            "SELECT DISTINCT {column} AS v FROM logs{where_sql} ORDER BY v LIMIT {LOG_OPTION_ROW_CAP}"
        );
        let values: Vec<String> = bind_clause!(sqlx::query_scalar(&sql), &binds)
            .fetch_all(&state.pool)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .into_iter()
            // Legacy rows carry '' in the split model columns; the COALESCE
            // handles those for matching, but there's no empty option to offer.
            .filter(|v: &String| !v.is_empty())
            .collect();
        out.insert(key.to_string(), json!(values));
    }

    // Status codes are a small closed set, so exact-match filtering makes the
    // facets indistinguishable — the code's own filter is skipped like the
    // others'.
    let (where_sql, binds) = log_where_clause(&user, &allowed, q.range, &q, Some("status"));
    let status_sql =
        format!("SELECT DISTINCT status_code FROM logs{where_sql} ORDER BY status_code LIMIT 100");
    let statuses: Vec<i64> = bind_clause!(sqlx::query_scalar(&status_sql), &binds)
        .fetch_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    out.insert("status".to_string(), json!(statuses));

    Ok(Json(Value::Object(out)))
}

/// Load a `logs` row on behalf of `user`, applying the same ownership rule as
/// `list_logs`: non-admins only see logs against tokens they own.
///
/// A row the caller may not see is a `404`, not a `403` — a 403 would confirm
/// the id exists, which is itself a leak across tenants.
async fn load_log_for_user(
    pool: &sqlx::SqlitePool,
    user: &AuthUser,
    id: i64,
) -> Result<sqlx::sqlite::SqliteRow, StatusCode> {
    let row = sqlx::query("SELECT * FROM logs WHERE id=?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    if !user.is_admin {
        let owner = sqlx::query_scalar::<_, i64>(
            "SELECT COALESCE((SELECT user_id FROM tokens WHERE name = ?), 0)",
        )
        .bind(row.get::<String, _>("token_name"))
        .fetch_one(pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if owner != user.id {
            return Err(StatusCode::NOT_FOUND);
        }
    }
    Ok(row)
}

/// GET /api/logs/:id — full metadata for a single request, used by the
/// log detail page. Same user-scoping as `list_logs`: non-admins can only
/// see logs against tokens they own.
pub async fn get_log(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    let user = check_auth(&state, &headers)?;
    let row = load_log_for_user(&state.pool, &user, id).await?;
    // `upstream_model` is empty on rows written before 0014; `model` has
    // always held the post-mapping name, so fall back to it.
    let upstream_model = {
        let m = row.get::<String, _>("upstream_model");
        if m.is_empty() {
            row.get::<String, _>("model")
        } else {
            m
        }
    };
    // The relay chain behind this request. One row per upstream attempt, in
    // the order they happened — empty for logs written before 0015.
    let attempt_rows = sqlx::query(
        "SELECT seq, upstream_model, channel_name, status_code, error, latency_ms, convert, ok, skipped \
         FROM log_attempts WHERE log_id=? ORDER BY seq ASC",
    )
    .bind(id)
    .fetch_all(&state.pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let attempts: Vec<Value> = attempt_rows
        .iter()
        .map(|a| {
            json!({
                "seq": a.get::<i64, _>("seq"),
                "upstream_model": a.get::<String, _>("upstream_model"),
                "channel_name": a.get::<String, _>("channel_name"),
                "status_code": a.get::<i64, _>("status_code"),
                "error": a.get::<String, _>("error"),
                "latency_ms": a.get::<i64, _>("latency_ms"),
                "convert": a.get::<String, _>("convert"),
                "ok": a.get::<i64, _>("ok") != 0,
                "skipped": a.get::<i64, _>("skipped") != 0,
            })
        })
        .collect();
    Ok(Json(json!({
        "log": {
            "id": row.get::<i64, _>("id"),
            "token_name": row.get::<String, _>("token_name"),
            "channel_name": row.get::<String, _>("channel_name"),
            "status_code": row.get::<i64, _>("status_code"),
            "prompt_tokens": row.get::<i64, _>("prompt_tokens"),
            "completion_tokens": row.get::<i64, _>("completion_tokens"),
            "total_tokens": row.get::<i64, _>("total_tokens"),
            "cache_read_tokens": row.get::<i64, _>("cache_read_tokens"),
            "cache_creation_tokens": row.get::<i64, _>("cache_creation_tokens"),
            "reasoning_tokens": row.get::<i64, _>("reasoning_tokens"),
            "request_model": row.get::<String, _>("request_model"),
            "upstream_model": upstream_model,
            "latency_ms": row.get::<i64, _>("latency_ms"),
            "stream": row.get::<i64, _>("stream") != 0,
            "protocol": row.get::<String, _>("protocol"),
            "convert": row.get::<String, _>("convert"),
            "error": row.get::<String, _>("error"),
            "failed_count": row.try_get("failed_count").unwrap_or(0),
            "attempts": attempts,
            "created_at": row.get::<i64, _>("created_at"),
            "client_ip": row.get::<String, _>("client_ip"),
            "user_agent": row.get::<String, _>("user_agent"),
        }
    })))
}

/// GET /api/logs/:id/debug — the captured upstream response body for a
/// request, as written by the relay's debug capture.
///
/// Deliberately its own endpoint rather than a field on `get_log`: the body
/// can be a couple hundred KB, and `get_log` is the hot path the log list
/// and detail page both hit on every render.
///
/// Authorization is identical to `get_log` (including the 404-not-403 rule),
/// so this can't be used to read other tenants' traffic. `available: false`
/// with a 200 means "you may see this log, but nothing was captured" — a
/// success that succeeded, or a failure whose upstream sent no body. The two
/// cases are deliberately distinguishable from the 404.
pub async fn get_log_debug(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Json<Value>, StatusCode> {
    let user = check_auth(&state, &headers)?;
    load_log_for_user(&state.pool, &user, id).await?;
    let Some(file) = proxy::read_debug_log(&state.pool, id).await else {
        return Ok(Json(json!({
            "log_id": id,
            "available": false,
        })));
    };
    // Lossy: an upstream can reply with binary or with a truncated UTF-8
    // sequence. The body is returned as a JSON *string* so the SPA never has
    // to guess a content type, and replacement chars are visible rather than
    // silently decoding into something that looks valid.
    Ok(Json(json!({
        "log_id": id,
        "available": true,
        "bytes": file.bytes.len(),
        "truncated": file.truncated,
        "body": String::from_utf8_lossy(&file.bytes),
    })))
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
    let user = check_auth(&state, &headers)?;
    let days = q.range.clamp(1, 90);
    let since = crate::db::now() - days * 86400;
    let allowed = token_names_for_user(&state.pool, user.id, user.is_admin).await;

    // A predicate that limits to the caller's tokens, or empty when admin.
    // Empty `allowed` for non-admin collapses to no rows.
    fn restrict_clause(allowed: &[String]) -> String {
        if allowed.is_empty() {
            return " WHERE 1=0".to_string();
        }
        let ph = std::iter::repeat_n("?", allowed.len())
            .collect::<Vec<_>>()
            .join(",");
        format!(" WHERE token_name IN ({})", ph)
    }
    let clause = if user.is_admin {
        String::new()
    } else {
        restrict_clause(&allowed)
    };
    let user_binds = if user.is_admin { Vec::new() } else { allowed };

    // by_day (GROUP BY day bucket)
    //
    // Since 0015 a `logs` row is one client request, so `COUNT(*)` is the
    // request count the page always claimed to show. It used to over-count by
    // the number of failed hops per request. Token sums are unaffected: only
    // the winning attempt carries usage, and a failed hop always had none.
    // The per-channel breakdown likewise now attributes tokens to the channel
    // that actually served the request rather than to every channel tried.
    let by_day_sql = format!(
        "SELECT (created_at / 86400) * 86400 AS k,
                COUNT(*) AS reqs,
                SUM(prompt_tokens) AS p,
                SUM(completion_tokens) AS c,
                SUM(total_tokens) AS t
         FROM logs{0}{1} created_at >= ? GROUP BY k ORDER BY k",
        clause,
        if clause.is_empty() { " WHERE" } else { " AND" }
    );
    let mut day_query = sqlx::query(&by_day_sql);
    for n in &user_binds {
        day_query = day_query.bind(n);
    }
    day_query = day_query.bind(since);
    let day_rows = day_query
        .fetch_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // by token / model / channel — separate queries (SQLite has no GROUPING SETS)
    let group_sql = |col: &str| -> String {
        format!(
            "SELECT {0} AS k, COUNT(*) AS reqs,
                SUM(prompt_tokens) AS p,
                SUM(completion_tokens) AS c,
                SUM(total_tokens) AS t
             FROM logs{1}{2} created_at >= ? GROUP BY {0} ORDER BY t DESC",
            col,
            clause,
            if clause.is_empty() { " WHERE" } else { " AND" }
        )
    };
    let sql = group_sql("token_name");
    let mut q = sqlx::query(&sql);
    for n in &user_binds {
        q = q.bind(n);
    }
    q = q.bind(since);
    let token_rows = q
        .fetch_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let sql = group_sql("model");
    let mut q = sqlx::query(&sql);
    for n in &user_binds {
        q = q.bind(n);
    }
    q = q.bind(since);
    let model_rows = q
        .fetch_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let sql = group_sql("channel_name");
    let mut q = sqlx::query(&sql);
    for n in &user_binds {
        q = q.bind(n);
    }
    q = q.bind(since);
    let channel_rows = q
        .fetch_all(&state.pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // totals
    let totals_sql = format!(
        "SELECT COUNT(*) AS reqs, COALESCE(SUM(prompt_tokens),0) AS p,
                COALESCE(SUM(completion_tokens),0) AS c,
                COALESCE(SUM(total_tokens),0) AS t
         FROM logs{0}{1} created_at >= ?",
        clause,
        if clause.is_empty() { " WHERE" } else { " AND" }
    );
    let mut totals_query = sqlx::query(&totals_sql);
    for n in &user_binds {
        totals_query = totals_query.bind(n);
    }
    totals_query = totals_query.bind(since);
    let totals = totals_query
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

// `into_response` is brought in scope for fetch_models above; using a
// qualified import keeps the helper file narrower.
use axum::response::IntoResponse;
