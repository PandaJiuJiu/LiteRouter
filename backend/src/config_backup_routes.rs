//! HTTP surface for encrypted config backup / restore.
//!
//! Three endpoints, deliberately split so the SPA can show the user what
//! would happen before anything is written:
//!
//!   POST /api/config/export          → an encrypted .lrbak download
//!   POST /api/config/import/preview  → a per-row plan, nothing written
//!   POST /api/config/import/commit   → applies the plan the user approved
//!
//! The two-phase import is the point. A backup is the kind of file an admin
//! clicks "import" on twice by accident, and a same-name channel can mean
//! either "this is the same channel, refresh it" or "this is a *different*
//! channel that happens to share a label". Those are opposite intents, so
//! the choice can't be inferred — [`ConflictAction`] is the user's call and
//! is made per row.
//!
//! All three endpoints are admin-only. An export contains `channels.api_key`
//! and `tokens.key` in the clear inside the ciphertext; handing it to a
//! non-admin would defeat the point of the passphrase.

use crate::auth::{check_auth, AuthUser};
use crate::config_backup::{BackupPayload, ChannelRow, MappingRow, MappingTarget, TokenRow};
use crate::db::now;
use crate::mappings::{encode_targets, parse_targets};
use crate::state::AppState;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::Row;
use std::sync::Arc;

/// The three coverable sections. Kept as constants so the SPA, the export
/// writer, and the import validator all agree on the exact spelling — a
/// typo'd section name would otherwise silently import nothing.
pub const SECTION_CHANNELS: &str = "channels";
pub const SECTION_TOKENS: &str = "tokens";
pub const SECTION_MAPPINGS: &str = "mappings";

const ALL_SECTIONS: [&str; 3] = [SECTION_CHANNELS, SECTION_TOKENS, SECTION_MAPPINGS];

/// A user decision for one conflicting row. `Overwrite` replaces the local
/// row's fields with the backup's. `Skip` leaves the local row alone.
/// `KeepBoth` inserts the backup's row under a suffixed name (`name_1`,
/// `name_2`, … the first free one), leaving the local row untouched.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictAction {
    Overwrite,
    Skip,
    KeepBoth,
}

impl ConflictAction {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "overwrite" => Some(Self::Overwrite),
            "skip" => Some(Self::Skip),
            "keep_both" => Some(Self::KeepBoth),
            _ => None,
        }
    }
}

// ---------- shared helpers ----------

/// A table + column pair used to probe for name availability.
struct TableCol(&'static str, &'static str);

/// Find the first `_n` suffix (n = 1..9999) that produces a name absent from
/// `table.column`. Falls back to `base_<timestamp>` if all suffixes are taken.
async fn free_name(pool: &sqlx::SqlitePool, base: &str, tc: TableCol) -> String {
    for n in 1..10_000 {
        let candidate = format!("{base}_{n}");
        let taken: i64 = sqlx::query_scalar(format!("SELECT COUNT(*) FROM {} WHERE {}=?", tc.0, tc.1).as_str())
            .bind(&candidate)
            .fetch_one(pool)
            .await
            .unwrap_or(1);
        if taken == 0 {
            return candidate;
        }
    }
    format!("{base}_{}", now())
}

/// Build a 400 with a JSON `{error: ...}` body.
fn bad_request(msg: impl Into<String>) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg.into() }))).into_response()
}

/// Validate the requested section list against [`ALL_SECTIONS`].
#[allow(clippy::result_large_err)]
fn selected_sections(raw: &[String]) -> Result<Vec<String>, Response> {
    if raw.is_empty() {
        return Err(bad_request("至少选择一项要导出的内容"));
    }
    for s in raw {
        if !ALL_SECTIONS.contains(&s.as_str()) {
            return Err(bad_request(format!("未知的导出内容：{s}")));
        }
    }
    let mut out: Vec<String> = Vec::new();
    for s in raw {
        if !out.contains(s) {
            out.push(s.clone());
        }
    }
    Ok(out)
}

/// Require a passphrase. Empty or all-whitespace is rejected.
#[allow(clippy::result_large_err)]
fn require_passphrase(p: &str) -> Result<(), Response> {
    if p.trim().is_empty() {
        return Err(bad_request("必须设置一个密码来加密备份文件"));
    }
    if p.len() < 8 {
        return Err(bad_request("加密密码至少 8 位"));
    }
    Ok(())
}

// ---------- export ----------

#[derive(Deserialize)]
pub struct ExportReq {
    /// Which sections to include. All three by default on the SPA side, but
    /// the API requires an explicit non-empty list.
    pub sections: Vec<String>,
    pub passphrase: String,
}

/// POST /api/config/export — admin only. Returns the encrypted file as an
/// octet-stream attachment. The SPA turns this into a download; nothing is
/// written to the server.
pub async fn export_config(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ExportReq>,
) -> Result<Response, Response> {
    require_admin_err(&state, &headers)?;
    let sections = selected_sections(&req.sections)?;
    require_passphrase(&req.passphrase)?;

    let mut payload = BackupPayload {
        created_at: now(),
        sections: sections.clone(),
        ..Default::default()
    };

    for s in &sections {
        match s.as_str() {
            SECTION_CHANNELS => payload.channels = read_channels(&state).await,
            SECTION_TOKENS => payload.tokens = read_tokens(&state).await,
            SECTION_MAPPINGS => payload.mappings = read_mappings(&state).await,
            _ => unreachable!("selected_sections already validated"),
        }
    }

    let blob = crate::config_backup::encrypt(&req.passphrase, &payload);
    // Stamp the filename with the creation date so a folder of backups is
    // self-describing without opening anything.
    let stamp = crate::config_backup::format_stamp(payload.created_at);
    let filename = format!("literouter-backup-{stamp}.lrbak");
    let mut response = blob.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{filename}\"")) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(response)
}

#[allow(clippy::result_large_err)]
fn require_admin_err(state: &AppState, headers: &HeaderMap) -> Result<AuthUser, Response> {
    match check_auth(state, headers) {
        Ok(u) if u.is_admin => Ok(u),
        _ => Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "需要管理员权限" })),
        )
            .into_response()),
    }
}

async fn read_channels(state: &AppState) -> Vec<ChannelRow> {
    let rows = sqlx::query(
        "SELECT name, website, base_url, base_url_anthropic, api_key, models, enabled, created_at \
         FROM channels ORDER BY id ASC",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();
    rows.iter()
        .map(|r| ChannelRow {
            name: r.get("name"),
            website: r.try_get("website").unwrap_or_default(),
            base_url: r.get("base_url"),
            base_url_anthropic: r.try_get("base_url_anthropic").unwrap_or_default(),
            api_key: r.get("api_key"),
            models: r.get("models"),
            enabled: r.get::<i64, _>("enabled") != 0,
            created_at: r.get("created_at"),
        })
        .collect()
}

async fn read_tokens(state: &AppState) -> Vec<TokenRow> {
    let rows = sqlx::query(
        "SELECT t.name, t.key, t.enabled, t.rpm_limit, t.daily_token_limit, t.created_at, \
                COALESCE(u.username, '') AS username \
         FROM tokens t LEFT JOIN users u ON t.user_id = u.id \
         ORDER BY t.id ASC",
    )
    .fetch_all(&state.pool)
    .await
    .unwrap_or_default();
    rows.iter()
        .map(|r| TokenRow {
            name: r.get("name"),
            key: r.get("key"),
            enabled: r.get::<i64, _>("enabled") != 0,
            rpm_limit: r.try_get("rpm_limit").unwrap_or(0),
            daily_token_limit: r.try_get("daily_token_limit").unwrap_or(0),
            username: r.get("username"),
            created_at: r.get("created_at"),
        })
        .collect()
}

async fn read_mappings(state: &AppState) -> Vec<MappingRow> {
    let rows = sqlx::query("SELECT * FROM model_mappings ORDER BY id ASC")
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();
    rows.iter()
        .map(|r| {
            let targets = parse_targets(
                &r.try_get::<String, _>("targets").unwrap_or_default(),
                &r.try_get::<String, _>("target_model").unwrap_or_default(),
            );
            MappingRow {
                alias: r.get("alias"),
                targets: targets
                    .into_iter()
                    .map(|(channel, model)| MappingTarget { channel, model })
                    .collect(),
                created_at: r.get("created_at"),
            }
        })
        .collect()
}

// ---------- import preview ----------

#[derive(Deserialize)]
pub struct ImportFileReq {
    /// Raw bytes of the `.lrbak` file, as uploaded.
    #[serde(rename = "file")]
    pub file: String,
    pub passphrase: String,
}

/// One row of the import plan. `conflict: false` means the name is free —
/// it will be created no matter what. `conflict: true` means the user must
/// pick an action before commit; the plan we return does NOT guess.
fn plan_row(kind: &str, name: &str, incoming: &Value, conflict: bool, detail: Value) -> Value {
    json!({
        "kind": kind,
        "name": name,
        "conflict": conflict,
        "incoming": incoming,
        "detail": detail,
    })
}

/// POST /api/config/import/preview — admin only. Decrypts the file and
/// reports, per row, whether the name is free or collides. Writes nothing.
pub async fn preview_import(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ImportFileReq>,
) -> Result<Json<Value>, Response> {
    require_admin_err(&state, &headers)?;
    require_passphrase(&req.passphrase)?;
    let payload = decrypt_or_400(&req.passphrase, &req.file)?;

    let mut sections = json!({});

    // channels
    let mut chan_rows = Vec::new();
    for c in &payload.channels {
        let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM channels WHERE name=?")
            .bind(&c.name)
            .fetch_optional(&state.pool)
            .await
            .unwrap_or(None);
        chan_rows.push(plan_row(
            "channel",
            &c.name,
            &json!({
                "base_url": c.base_url,
                "base_url_anthropic": c.base_url_anthropic,
                "models": c.models,
                "enabled": c.enabled,
            }),
            existing.is_some(),
            json!({ "existing_id": existing }),
        ));
    }
    sections["channels"] = json!(chan_rows);

    // tokens
    let mut tok_rows = Vec::new();
    for t in &payload.tokens {
        let existing: Option<i64> =
            sqlx::query_scalar("SELECT id FROM tokens WHERE name=? LIMIT 1")
                .bind(&t.name)
                .fetch_optional(&state.pool)
                .await
                .unwrap_or(None);
        tok_rows.push(plan_row(
            "token",
            &t.name,
            &json!({
                "key_prefix": t.key.chars().take(8).collect::<String>(),
                "username": t.username,
                "enabled": t.enabled,
                "rpm_limit": t.rpm_limit,
                "daily_token_limit": t.daily_token_limit,
            }),
            existing.is_some(),
            json!({ "existing_id": existing }),
        ));
    }
    sections["tokens"] = json!(tok_rows);

    // mappings
    let mut map_rows = Vec::new();
    for m in &payload.mappings {
        let existing: Option<i64> =
            sqlx::query_scalar("SELECT id FROM model_mappings WHERE alias=?")
                .bind(&m.alias)
                .fetch_optional(&state.pool)
                .await
                .unwrap_or(None);
        map_rows.push(plan_row(
            "mapping",
            &m.alias,
            &json!({
                "targets": m.targets.iter().map(|t| json!({"channel": t.channel, "model": t.model})).collect::<Vec<_>>(),
            }),
            existing.is_some(),
            json!({ "existing_id": existing }),
        ));
    }
    sections["mappings"] = json!(map_rows);

    Ok(Json(json!({
        "sections": payload.sections,
        "created_at": payload.created_at,
        "plan": sections,
    })))
}

/// Decrypt, mapping every failure mode to a 400 with a message the SPA can
/// show verbatim. Wrong passphrase and corrupted file both land here — the
/// auth tag can't distinguish them, and telling the user which one it was
/// would be a lie.
#[allow(clippy::result_large_err)]
fn decrypt_or_400(passphrase: &str, file_b64: &str) -> Result<BackupPayload, Response> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(file_b64)
        .map_err(|_| bad_request("备份文件编码有误"))?;
    crate::config_backup::decrypt(passphrase, &bytes).map_err(|e| bad_request(e.to_string()))
}

// ---------- import commit ----------

/// One decision from the client. `name` identifies the row within its
/// section; `action` is one of overwrite / skip / keep_both.
#[derive(Deserialize)]
pub struct Decision {
    pub name: String,
    pub action: String,
}

#[derive(Deserialize)]
pub struct ImportCommitReq {
    #[serde(rename = "file")]
    pub file: String,
    pub passphrase: String,
    /// `sections` mirrors what the SPA showed in the preview; it's advisory
    /// here — the actual rows come from the file, and an unknown action
    /// anywhere is a hard 400.
    #[serde(default)]
    pub sections: Vec<String>,
    pub decisions: std::collections::HashMap<String, Vec<Decision>>,
}

/// POST /api/config/import/commit — admin only. Applies the plan. Rows with
/// no decision and no conflict are created; a conflicting row with no
/// decision is an error (the SPA is expected to send a decision for every
/// conflict it showed).
pub async fn commit_import(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<ImportCommitReq>,
) -> Result<Json<Value>, Response> {
    require_admin_err(&state, &headers)?;
    require_passphrase(&req.passphrase)?;
    let payload = decrypt_or_400(&req.passphrase, &req.file)?;

    let mut created = 0i64;
    let mut updated = 0i64;
    let mut skipped = 0i64;
    let mut kept_both = 0i64;

    // channels
    for c in &payload.channels {
        let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM channels WHERE name=?")
            .bind(&c.name)
            .fetch_optional(&state.pool)
            .await
            .unwrap_or(None);
        match (existing, action_for(&req.decisions, "channels", &c.name)) {
            (None, _) => {
                insert_channel(&state, c, &c.name).await?;
                created += 1;
            }
            (Some(id), Some(ConflictAction::Overwrite)) => {
                update_channel(&state, id, c).await?;
                updated += 1;
            }
            (Some(_), Some(ConflictAction::Skip)) => {
                skipped += 1;
            }
            (Some(_), Some(ConflictAction::KeepBoth)) => {
                let name = free_name(&state.pool, &c.name, TableCol("channels", "name")).await;
                insert_channel(&state, c, &name).await?;
                kept_both += 1;
            }
            (Some(_), None) => {
                return Err(bad_request(format!(
                    "渠道 `{}` 名称冲突，但没给处理方式",
                    c.name
                )));
            }
        }
    }

    // tokens
    for t in &payload.tokens {
        let existing: Option<i64> =
            sqlx::query_scalar("SELECT id FROM tokens WHERE name=? LIMIT 1")
                .bind(&t.name)
                .fetch_optional(&state.pool)
                .await
                .unwrap_or(None);
        match (existing, action_for(&req.decisions, "tokens", &t.name)) {
            (None, _) => {
                insert_token(&state, t, &t.name).await?;
                created += 1;
            }
            (Some(id), Some(ConflictAction::Overwrite)) => {
                update_token(&state, id, t).await?;
                updated += 1;
            }
            (Some(_), Some(ConflictAction::Skip)) => {
                skipped += 1;
            }
            (Some(_), Some(ConflictAction::KeepBoth)) => {
                let name = free_name(&state.pool, &t.name, TableCol("tokens", "name")).await;
                insert_token(&state, t, &name).await?;
                kept_both += 1;
            }
            (Some(_), None) => {
                return Err(bad_request(format!(
                    "令牌 `{}` 名称冲突，但没给处理方式",
                    t.name
                )));
            }
        }
    }

    // mappings
    for m in &payload.mappings {
        let existing: Option<i64> =
            sqlx::query_scalar("SELECT id FROM model_mappings WHERE alias=?")
                .bind(&m.alias)
                .fetch_optional(&state.pool)
                .await
                .unwrap_or(None);
        let targets: Vec<(String, String)> = m
            .targets
            .iter()
            .map(|t| (t.channel.clone(), t.model.clone()))
            .collect();
        match (existing, action_for(&req.decisions, "mappings", &m.alias)) {
            (None, _) => {
                insert_mapping(&state, m, &m.alias, &targets).await?;
                created += 1;
            }
            (Some(id), Some(ConflictAction::Overwrite)) => {
                update_mapping(&state, id, m, &targets).await?;
                updated += 1;
            }
            (Some(_), Some(ConflictAction::Skip)) => {
                skipped += 1;
            }
            (Some(_), Some(ConflictAction::KeepBoth)) => {
                let alias = free_name(&state.pool, &m.alias, TableCol("model_mappings", "alias")).await;
                insert_mapping(&state, m, &alias, &targets).await?;
                kept_both += 1;
            }
            (Some(_), None) => {
                return Err(bad_request(format!(
                    "模型路由 `{}` 别名冲突，但没给处理方式",
                    m.alias
                )));
            }
        }
    }

    Ok(Json(json!({
        "created": created,
        "updated": updated,
        "skipped": skipped,
        "kept_both": kept_both,
    })))
}

/// Pull the user's decision for one row, if any. Returns `None` both when
/// there's no decision and when the decision string is unrecognized — the
/// caller treats the latter as a 400 at the "no decision for a conflict"
/// site, which is where the user needs to see it.
fn action_for(
    decisions: &std::collections::HashMap<String, Vec<Decision>>,
    section: &str,
    name: &str,
) -> Option<ConflictAction> {
    decisions
        .get(section)?
        .iter()
        .find(|d| d.name == name)
        .and_then(|d| ConflictAction::parse(&d.action))
}

// ---------- row writers ----------

#[allow(clippy::result_large_err)]
async fn insert_channel(state: &AppState, c: &ChannelRow, name: &str) -> Result<(), Response> {
    sqlx::query(
        "INSERT INTO channels (name, website, base_url, base_url_anthropic, api_key, models, enabled, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(name)
    .bind(&c.website)
    .bind(&c.base_url)
    .bind(&c.base_url_anthropic)
    .bind(&c.api_key)
    .bind(&c.models)
    .bind(c.enabled as i64)
    .bind(c.created_at)
    .execute(&state.pool)
    .await
    .map_err(|_| bad_request(format!("渠道 `{name}` 写入失败（可能同名或字段过长）")))?;
    Ok(())
}

#[allow(clippy::result_large_err)]
async fn update_channel(state: &AppState, id: i64, c: &ChannelRow) -> Result<(), Response> {
    sqlx::query(
        "UPDATE channels SET website=?, base_url=?, base_url_anthropic=?, api_key=?, models=?, enabled=? WHERE id=?",
    )
    .bind(&c.website)
    .bind(&c.base_url)
    .bind(&c.base_url_anthropic)
    .bind(&c.api_key)
    .bind(&c.models)
    .bind(c.enabled as i64)
    .bind(id)
    .execute(&state.pool)
    .await
    .map_err(|_| bad_request("渠道更新失败"))?;
    Ok(())
}

#[allow(clippy::result_large_err)]
async fn insert_token(state: &AppState, t: &TokenRow, name: &str) -> Result<(), Response> {
    // Resolve the owner by username. A backup from a system where the token
    // belonged to an account that doesn't exist here is a real scenario
    // (importing a colleague's export), so a missing username is a clear
    // 400 rather than a silent "owned by nobody".
    let user_id: Option<i64> = sqlx::query_scalar("SELECT id FROM users WHERE username=?")
        .bind(&t.username)
        .fetch_optional(&state.pool)
        .await
        .unwrap_or(None);
    let user_id = user_id.ok_or_else(|| {
        bad_request(format!(
            "令牌 `{name}` 的归属账号 `{}` 在本系统不存在，请先创建该账号",
            t.username
        ))
    })?;
    sqlx::query(
        "INSERT INTO tokens (name, key, enabled, created_at, rpm_limit, daily_token_limit, user_id) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(name)
    .bind(&t.key)
    .bind(t.enabled as i64)
    .bind(t.created_at)
    .bind(t.rpm_limit.max(0))
    .bind(t.daily_token_limit.max(0))
    .bind(user_id)
    .execute(&state.pool)
    .await
    .map_err(|e| {
        if crate::db::is_unique_violation(&e) {
            bad_request(format!("令牌 `{name}` 的 key 与现有令牌重复"))
        } else {
            bad_request(format!("令牌 `{name}` 写入失败"))
        }
    })?;
    Ok(())
}

#[allow(clippy::result_large_err)]
async fn update_token(state: &AppState, id: i64, t: &TokenRow) -> Result<(), Response> {
    // Deliberately does NOT touch `key`. Overwriting a credential because an
    // admin clicked "overwrite" on a stale backup would 401 every client
    // using the old key, with no obvious cause. The name/flags/quotas sync;
    // rotating the key stays an explicit act on the Tokens page.
    sqlx::query("UPDATE tokens SET enabled=?, rpm_limit=?, daily_token_limit=? WHERE id=?")
        .bind(t.enabled as i64)
        .bind(t.rpm_limit.max(0))
        .bind(t.daily_token_limit.max(0))
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|_| bad_request("令牌更新失败"))?;
    Ok(())
}

#[allow(clippy::result_large_err)]
async fn insert_mapping(
    state: &AppState,
    _m: &MappingRow,
    alias: &str,
    targets: &[(String, String)],
) -> Result<(), Response> {
    if targets.is_empty() {
        return Err(bad_request(format!("模型路由 `{alias}` 没有有效目标")));
    }
    sqlx::query(
        "INSERT INTO model_mappings (alias, target_model, targets, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(alias)
    .bind(&targets[0].1)
    .bind(encode_targets(targets))
    .bind(now())
    .execute(&state.pool)
    .await
    .map_err(|e| {
        if crate::db::is_unique_violation(&e) {
            bad_request(format!("模型路由 `{alias}` 别名重复"))
        } else {
            bad_request(format!("模型路由 `{alias}` 写入失败"))
        }
    })?;
    Ok(())
}

#[allow(clippy::result_large_err)]
async fn update_mapping(
    state: &AppState,
    id: i64,
    _m: &MappingRow,
    targets: &[(String, String)],
) -> Result<(), Response> {
    if targets.is_empty() {
        return Err(bad_request("模型路由更新时没有有效目标"));
    }
    sqlx::query("UPDATE model_mappings SET target_model=?, targets=? WHERE id=?")
        .bind(&targets[0].1)
        .bind(encode_targets(targets))
        .bind(id)
        .execute(&state.pool)
        .await
        .map_err(|_| bad_request("模型路由更新失败"))?;
    Ok(())
}
