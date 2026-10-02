//! 熔断器事件历史：append-only 的审计表写入。
//!
//! 由调用 `Breaker::record()` / `reset()` / `reset_key()` 的位置调用
//! [`record_breaker_event`]，把每一次状态变化（tripped / re-tripped /
//! recovered / reset_all / reset_key）落库到 `breaker_events` 表。调用方
//! 负责根据 `Breaker::record()` 返回的 [`breaker::Transition`] 决定写什么
//! 事件；本模块只关心落库，**不**碰 `Breaker` 内部数据结构。
//!
//! 写入是 fire-and-forget 的：proxy 失败路径每次失败都会触发记录，不允许
//! 阻塞请求路径。`record_breaker_event` 内部 `tokio::spawn`，失败仅
//! `eprintln!` 不抛。

use crate::db;
use sqlx::{Executor, SqlitePool};

/// 事件类型。与 `breaker_events.event` 列一一对应。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BreakerEventKind {
    /// Closed → Open：第一次失败。
    Tripped,
    /// Open → Open（退避翻倍）：探测再次失败。
    ReTripped,
    /// Open → Closed：探测成功。
    Recovered,
    /// 管理员手动"Reset all"。
    ResetAll,
    /// 探测任务发现 channel 消失后单 key 删除。
    ResetKey,
}

impl BreakerEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BreakerEventKind::Tripped => "tripped",
            BreakerEventKind::ReTripped => "re-tripped",
            BreakerEventKind::Recovered => "recovered",
            BreakerEventKind::ResetAll => "reset_all",
            BreakerEventKind::ResetKey => "reset_key",
        }
    }
}

/// 一条事件记录的输入数据。`channel_name` / `target_model` 是字符串而非
/// id —— 与 `log_attempts` 同模式，便于在 channel 被删除后保留可读性。
#[derive(Clone, Debug)]
pub struct BreakerEventRow {
    pub channel_name: String,
    pub target_model: String,
    pub event: BreakerEventKind,
    pub reason: String,
    pub backoff_secs: u64,
}

/// 把一条事件写入 `breaker_events` 表。`tokio::spawn` 不阻塞调用方；失败
/// 仅 `eprintln!`，避免在热路径上抛错导致请求链路回退。
pub fn record_breaker_event(pool: &SqlitePool, row: BreakerEventRow) {
    let pool = pool.clone();
    tokio::spawn(async move {
        let now = db::now();
        let sql = "INSERT INTO breaker_events
                   (channel_name, target_model, event, reason, backoff_secs, created_at)
                   VALUES (?, ?, ?, ?, ?, ?)";
        let outcome = pool
            .execute(
                sqlx::query(sql)
                    .bind(&row.channel_name)
                    .bind(&row.target_model)
                    .bind(row.event.as_str())
                    .bind(&row.reason)
                    .bind(row.backoff_secs as i64)
                    .bind(now),
            )
            .await;
        if let Err(e) = outcome {
            // 写失败不能阻塞 relay；保留一行日志即可。日志保留任务不清理
            // `breaker_events`（比 `logs` 稀疏得多），所以无须登记到
            // 清理逻辑里。
            eprintln!("breaker_events insert failed: {e}");
        }
    });
}
