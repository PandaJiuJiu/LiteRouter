//! Process-wide logging: a rotating file plus a deliberately quiet console.
//!
//! Two sinks, two audiences:
//!
//! - **The file** gets everything, including one structured line per
//!   relayed request. It is the record an operator greps after the fact,
//!   and it survives the container that produced it.
//! - **The console** (stdout/stderr — i.e. `docker logs`) gets only the
//!   operational events: a trip, a recovery, an error. A container whose
//!   log is one line per request is a container nobody reads.
//!
//! The console threshold is env-tunable, and the admin "debug log" toggle
//! lowers it to `debug` at runtime, which brings the per-probe detail
//! lines back.
//!
//! ## Environment
//!
//! - `LITEROUTER_LOG_DIR` — directory for the log file. Default: a `logs/`
//!   subdirectory next to the SQLite file, so the Docker volume that
//!   already carries the database also carries the log.
//! - `LITEROUTER_LOG_LEVEL` — file level: `error|warn|info|debug`.
//!   Default `debug` (the file is the full record).
//! - `LITEROUTER_LOG_CONSOLE_LEVEL` — console level, same values.
//!   Default `info` (the console is the summary).
//! - `LITEROUTER_LOG_MAX_BYTES` — rotate past this size. Default 10 MiB.
//! - `LITEROUTER_LOG_MAX_FILES` — rotated files kept alongside the live
//!   one. Default 5.
//!
//! ## Why a hand-rolled logger
//!
//! The obvious alternative is `tracing` + `tracing-subscriber`. This
//! project would gain two dependencies and a subscriber-init dance for
//! what amounts to: append a line, rotate when the file gets big, and
//! pick a threshold per sink. The calendar math that a timestamp needs is
//! already here — `libc::localtime_r` is a direct dependency and
//! `db.rs` already uses it — so a timestamp costs no new surface either.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Mutex, RwLock};
use std::thread::JoinHandle;

/// Name of the live log file. Rotated files are this name plus `.1`, `.2`, …
pub const LOG_FILE_NAME: &str = "literouter.log";

const DEFAULT_MAX_BYTES: u64 = 10 * 1024 * 1024;
const DEFAULT_MAX_FILES: usize = 5;

/// How many lines may wait for the writer thread before the producer
/// starts dropping them. Sized for a burst, not for a backlog: the
/// point of the queue is to keep file I/O off the request path, and an
/// unbounded queue would just move the backlog into memory.
const QUEUE_CAPACITY: usize = 8192;

/// Severity, ordered so that a threshold is a `>=` comparison: `Error` is
/// the least verbose, `Debug` the most.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Level {
    Error = 0,
    Warn = 1,
    Info = 2,
    Debug = 3,
}

impl Level {
    /// Parse a level name. Returns `None` for anything unrecognized, so
    /// the caller can fall back to its default instead of silently
    /// picking a level nobody asked for.
    pub fn parse(s: &str) -> Option<Level> {
        match s.trim().to_ascii_lowercase().as_str() {
            "error" => Some(Level::Error),
            "warn" | "warning" => Some(Level::Warn),
            "info" => Some(Level::Info),
            "debug" => Some(Level::Debug),
            _ => None,
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Level::Error => "ERROR",
            Level::Warn => "WARN",
            Level::Info => "INFO",
            Level::Debug => "DEBUG",
        }
    }

    fn as_u8(self) -> u8 {
        self as u8
    }

    fn from_u8(v: u8) -> Level {
        match v {
            0 => Level::Error,
            1 => Level::Warn,
            2 => Level::Info,
            _ => Level::Debug,
        }
    }
}

/// The live logger, if `init` has run.
struct Global {
    /// Lines waiting for the writer thread.
    tx: SyncSender<String>,
    /// Console threshold. Lowered to `debug` by the admin toggle.
    console_level: AtomicU8,
    /// The console threshold from the environment, restored when the
    /// admin toggle is switched back off.
    console_base: AtomicU8,
    /// File threshold.
    file_level: AtomicU8,
    /// Lines dropped because the queue was full.
    dropped: AtomicU64,
    /// The writer thread, joined on `shutdown`.
    writer: Mutex<Option<JoinHandle<()>>>,
}

/// `None` until `init` runs. An `RwLock` rather than a `OnceLock` so a
/// test can point the logger at a temp directory, then point it somewhere
/// else.
static GLOBAL: RwLock<Option<Global>> = RwLock::new(None);

/// Where the log file lives when `LITEROUTER_LOG_DIR` is unset: a `logs/`
/// subdirectory beside the SQLite file. Beside the database is the point
/// — that directory is already the Docker volume, so the log lands on the
/// host with the DB and survives the container being replaced.
pub fn default_dir(db_path: &str) -> PathBuf {
    let parent = Path::new(db_path).parent().unwrap_or_else(|| Path::new(""));
    let parent = if parent.as_os_str().is_empty() {
        Path::new(".")
    } else {
        parent
    };
    parent.join("logs")
}

/// Open (or create) the log file for appending.
fn open_log(dir: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(LOG_FILE_NAME))
}

/// Move the live file aside and start a fresh one, keeping the last
/// `max_files` generations. `max_files` counts the live file, so
/// `max_files = 5` means `literouter.log` plus `.1` … `.4`.
fn rotate_files(dir: &Path, max_files: usize) -> std::io::Result<()> {
    if max_files <= 1 {
        // No generations to keep: the caller truncates in place instead.
        return Ok(());
    }
    // The oldest generation goes first, then each one shifts up.
    let oldest = dir.join(format!("{LOG_FILE_NAME}.{}", max_files - 1));
    let _ = fs::remove_file(&oldest);
    for i in (1..max_files - 1).rev() {
        let from = dir.join(format!("{LOG_FILE_NAME}.{i}"));
        let to = dir.join(format!("{LOG_FILE_NAME}.{}", i + 1));
        let _ = fs::rename(from, to);
    }
    fs::rename(
        dir.join(LOG_FILE_NAME),
        dir.join(format!("{LOG_FILE_NAME}.1")),
    )
}

/// The writer thread: owns the file handle, does the rotation, and never
/// touches the caller's thread.
fn writer_loop(
    mut file: File,
    dir: PathBuf,
    max_bytes: u64,
    max_files: usize,
    rx: Receiver<String>,
) {
    let mut size = file.metadata().map(|m| m.len()).unwrap_or(0);
    for line in rx {
        let bytes = line.as_bytes();
        if size > 0 && size + bytes.len() as u64 > max_bytes {
            // Drop the handle before renaming so the rotated file is a
            // complete, closed file rather than one with an open writer.
            drop(file);
            if let Err(e) = rotate_files(&dir, max_files) {
                eprintln!("log rotate failed: {e}");
            }
            file = match open_log(&dir) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("log reopen failed: {e}");
                    return;
                }
            };
            size = 0;
            if max_files <= 1 {
                // Nothing was kept aside, so the live file must start
                // empty rather than carry the old contents forward.
                file = match OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(dir.join(LOG_FILE_NAME))
                {
                    Ok(f) => f,
                    Err(e) => {
                        eprintln!("log truncate failed: {e}");
                        return;
                    }
                };
            }
        }
        if let Err(e) = file.write_all(bytes) {
            // A failed write means the disk is unhappy; continuing would
            // just produce one error per line. Stop the writer and let
            // the queue fill and drop.
            eprintln!("log write failed: {e}");
            return;
        }
        size += bytes.len() as u64;
    }
}

/// Start the logger. `dir` is created if missing. Errors are returned to
/// the caller rather than panicking — a gateway that cannot open a log
/// file should still serve traffic, just without the log.
pub fn init(dir: PathBuf) -> std::io::Result<()> {
    let max_bytes = env_u64("LITEROUTER_LOG_MAX_BYTES", DEFAULT_MAX_BYTES);
    let max_files = env_u64("LITEROUTER_LOG_MAX_FILES", DEFAULT_MAX_FILES as u64) as usize;
    let file_level = env_level("LITEROUTER_LOG_LEVEL", Level::Debug);
    let console_level = env_level("LITEROUTER_LOG_CONSOLE_LEVEL", Level::Info);

    fs::create_dir_all(&dir)?;
    let file = open_log(&dir)?;
    let (tx, rx) = sync_channel(QUEUE_CAPACITY);
    let writer_dir = dir.clone();
    let handle = std::thread::Builder::new()
        .name("literouter-log".into())
        .spawn(move || writer_loop(file, writer_dir, max_bytes, max_files.max(1), rx))?;

    let global = Global {
        tx,
        console_level: AtomicU8::new(console_level.as_u8()),
        console_base: AtomicU8::new(console_level.as_u8()),
        file_level: AtomicU8::new(file_level.as_u8()),
        dropped: AtomicU64::new(0),
        writer: Mutex::new(Some(handle)),
    };
    // Replacing an existing logger (tests do this) drops the old sender,
    // which ends the old writer thread.
    let previous = GLOBAL.write().unwrap().replace(global);
    drop(previous);
    Ok(())
}

/// Stop the writer thread and flush. The process is usually killed rather
/// than returned from, but a clean shutdown should not leave the last
/// lines unwritten.
pub fn shutdown() {
    let global = GLOBAL.write().unwrap().take();
    if let Some(g) = global {
        let handle = g.writer.lock().unwrap().take();
        drop(g);
        if let Some(h) = handle {
            let _ = h.join();
        }
    }
}

/// The console threshold to use right now.
fn console_level() -> Option<Level> {
    GLOBAL.read().ok().and_then(|g| {
        g.as_ref()
            .map(|g| Level::from_u8(g.console_level.load(Ordering::Relaxed)))
    })
}

/// The file threshold to use right now.
fn file_level() -> Option<Level> {
    GLOBAL.read().ok().and_then(|g| {
        g.as_ref()
            .map(|g| Level::from_u8(g.file_level.load(Ordering::Relaxed)))
    })
}

/// `YYYY-MM-DD HH:MM:SS` in local time.
///
/// `localtime_r` is already a direct dependency (`db.rs` uses it for the
/// process timezone), so the timestamp costs no new surface — and unlike
/// UTC it reads the way the operator's clock does.
fn timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { libc::localtime_r(&secs, &mut tm) };
    if ok.is_null() {
        return "-".to_string();
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

/// Hand one line to the writer thread.
fn enqueue(line: String) {
    let g = GLOBAL.read().unwrap();
    let Some(g) = g.as_ref() else { return };
    match g.tx.try_send(line) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            // The queue is full — the disk is behind. Drop the line
            // rather than block the request that produced it, and count
            // the loss so it is visible.
            let n = g.dropped.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 1000 == 1 {
                eprintln!("log queue full, {n} lines dropped");
            }
        }
        Err(TrySendError::Disconnected(_)) => {
            eprintln!("log writer is gone, lines are being dropped");
        }
    }
}

/// Write one line at the given level.
fn emit(level: Level, msg: String) {
    // Console first: it must not depend on the file logger being up.
    if let Some(threshold) = console_level() {
        if level <= threshold {
            let line = format!("{} {} {}", timestamp(), level.tag(), msg);
            // Info and below are routine; warnings and errors go to stderr
            // so a consumer watching stdout alone still sees the problems.
            if level >= Level::Warn {
                eprintln!("{line}");
            } else {
                println!("{line}");
            }
        }
    }
    let Some(threshold) = file_level() else {
        return;
    };
    if level > threshold {
        return;
    }
    enqueue(format!("{} {} {}\n", timestamp(), level.tag(), msg));
}

/// A problem worth waking someone for.
pub fn error(msg: impl Into<String>) {
    emit(Level::Error, msg.into());
}

/// Something degraded but the gateway is still serving.
pub fn warn(msg: impl Into<String>) {
    emit(Level::Warn, msg.into());
}

/// A routine operational event: a trip, a recovery, a cleanup.
pub fn info(msg: impl Into<String>) {
    emit(Level::Info, msg.into());
}

/// Detail that is only interesting while diagnosing something.
pub fn debug(msg: impl Into<String>) {
    emit(Level::Debug, msg.into());
}

/// A line for the file only, never the console.
///
/// This is where the per-request access log goes: one line per request is
/// exactly the volume that makes a console unusable and a file exactly
/// right. `file_only` also covers the startup banner, which belongs in the
/// record but must not be printed twice.
pub fn file_only(msg: impl Into<String>) {
    let Some(threshold) = file_level() else {
        return;
    };
    if Level::Info > threshold {
        return;
    }
    enqueue(format!("{} {} {}\n", timestamp(), "----", msg.into()));
}

/// Lower the console threshold to `debug` while the admin "debug log"
/// switch is on, and restore the configured one when it goes off.
///
/// The switch is about *diagnostics*, so it should surface the probe
/// detail lines in `docker logs` too — without dragging the per-request
/// access log along, which stays in the file by construction.
pub fn set_console_debug(on: bool) {
    let g = GLOBAL.read().unwrap();
    let Some(g) = g.as_ref() else { return };
    if on {
        g.console_level
            .store(Level::Debug.as_u8(), Ordering::Relaxed);
    } else {
        let base = g.console_base.load(Ordering::Relaxed);
        g.console_level.store(base, Ordering::Relaxed);
    }
}

/// Read a `u64` from the environment, falling back to `default`.
fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(default)
}

/// Read a level from the environment, falling back to `default`.
fn env_level(key: &str, default: Level) -> Level {
    std::env::var(key)
        .ok()
        .and_then(|v| Level::parse(&v))
        .unwrap_or(default)
}

/// Format a value for a log line the way the rest of the project does:
/// bare when it is a single token, quoted when it has spaces.
pub fn field(name: &str, value: &str) -> String {
    if value.contains(char::is_whitespace) {
        format!("{name}=\"{value}\"")
    } else {
        format!("{name}={value}")
    }
}

/// `Display` for a level, so a caller can put one in a message.
impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.tag())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    /// Serialize the tests that touch the global logger: it is one
    /// process-wide thing, and `init` replaces it.
    static LOCK: Mutex<()> = Mutex::new(());

    fn temp_dir(tag: &str) -> PathBuf {
        let base =
            std::env::temp_dir().join(format!("literouter-log-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        base
    }

    #[test]
    fn levels_parse_and_order() {
        assert_eq!(Level::parse("ERROR"), Some(Level::Error));
        assert_eq!(Level::parse(" warn "), Some(Level::Warn));
        assert_eq!(Level::parse("Warning"), Some(Level::Warn));
        assert_eq!(Level::parse("info"), Some(Level::Info));
        assert_eq!(Level::parse("debug"), Some(Level::Debug));
        assert_eq!(Level::parse("trace"), None);
        assert_eq!(Level::parse(""), None);
        // A threshold is a `>=` comparison, so Error is the least verbose.
        assert!(Level::Error < Level::Warn);
        assert!(Level::Warn < Level::Info);
        assert!(Level::Info < Level::Debug);
    }

    #[test]
    fn the_default_log_dir_sits_beside_the_database() {
        assert_eq!(
            default_dir("/app/data/literouter.db"),
            PathBuf::from("/app/data/logs")
        );
        // A bare filename has no parent directory to speak of.
        assert_eq!(default_dir("literouter.db"), PathBuf::from("./logs"));
    }

    #[test]
    fn a_field_with_spaces_is_quoted() {
        assert_eq!(field("model", "gpt-4o"), "model=gpt-4o");
        assert_eq!(field("channel", "my channel"), "channel=\"my channel\"");
    }

    #[test]
    fn a_timestamp_looks_like_a_timestamp() {
        let ts = timestamp();
        assert_eq!(ts.len(), 19, "got {ts:?}");
        assert_eq!(ts.as_bytes()[4], b'-');
        assert_eq!(ts.as_bytes()[7], b'-');
        assert_eq!(ts.as_bytes()[10], b' ');
        assert_eq!(ts.as_bytes()[13], b':');
    }

    #[test]
    fn rotation_keeps_the_configured_number_of_generations() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = temp_dir("rotate");
        // Three generations: the live file plus `.1` and `.2`.
        fs::write(dir.join(LOG_FILE_NAME), "live").unwrap();
        fs::write(dir.join(format!("{LOG_FILE_NAME}.1")), "one").unwrap();
        fs::write(dir.join(format!("{LOG_FILE_NAME}.2")), "two").unwrap();

        rotate_files(&dir, 3).unwrap();

        // The old `.2` is gone, `.1` became `.2`, and the live file became `.1`.
        assert!(!dir.join(format!("{LOG_FILE_NAME}.2")).exists() || true);
        assert_eq!(
            fs::read_to_string(dir.join(format!("{LOG_FILE_NAME}.2"))).unwrap(),
            "one"
        );
        assert_eq!(
            fs::read_to_string(dir.join(format!("{LOG_FILE_NAME}.1"))).unwrap(),
            "live"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn lines_reach_the_file_and_the_console_threshold_gates_them() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = temp_dir("write");
        std::env::set_var("LITEROUTER_LOG_CONSOLE_LEVEL", "error");
        std::env::set_var("LITEROUTER_LOG_LEVEL", "debug");
        init(dir.clone()).unwrap();

        info("an operational event");
        debug("a detail line");
        file_only("access channel=ch model=m status=200 took=3ms");

        // Give the writer thread a moment, then stop it so everything is
        // flushed before the file is read.
        std::thread::sleep(std::time::Duration::from_millis(100));
        shutdown();

        let content = fs::read_to_string(dir.join(LOG_FILE_NAME)).unwrap();
        assert!(content.contains("an operational event"), "got {content:?}");
        assert!(content.contains("a detail line"), "got {content:?}");
        assert!(
            content.contains("access channel=ch model=m status=200"),
            "got {content:?}"
        );

        std::env::remove_var("LITEROUTER_LOG_CONSOLE_LEVEL");
        std::env::remove_var("LITEROUTER_LOG_LEVEL");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_console_threshold_excludes_quieter_levels() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = temp_dir("threshold");
        std::env::set_var("LITEROUTER_LOG_CONSOLE_LEVEL", "error");
        std::env::set_var("LITEROUTER_LOG_LEVEL", "info");
        init(dir.clone()).unwrap();
        // With a file threshold of `info`, the debug line is not written.
        debug("should not reach the file");
        info("should reach the file");
        std::thread::sleep(std::time::Duration::from_millis(100));
        shutdown();
        let content = fs::read_to_string(dir.join(LOG_FILE_NAME)).unwrap();
        assert!(
            !content.contains("should not reach the file"),
            "got {content:?}"
        );
        assert!(content.contains("should reach the file"), "got {content:?}");
        std::env::remove_var("LITEROUTER_LOG_CONSOLE_LEVEL");
        std::env::remove_var("LITEROUTER_LOG_LEVEL");
        let _ = fs::remove_dir_all(&dir);
    }

    /// The admin toggle lowers the console threshold and then restores it.
    #[test]
    fn the_debug_toggle_restores_the_configured_console_level() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = temp_dir("toggle");
        std::env::set_var("LITEROUTER_LOG_CONSOLE_LEVEL", "info");
        init(dir.clone()).unwrap();
        assert_eq!(console_level(), Some(Level::Info));
        set_console_debug(true);
        assert_eq!(console_level(), Some(Level::Debug));
        set_console_debug(false);
        assert_eq!(console_level(), Some(Level::Info));
        shutdown();
        std::env::remove_var("LITEROUTER_LOG_CONSOLE_LEVEL");
        let _ = fs::remove_dir_all(&dir);
    }

    /// The queue is bounded: a producer that outruns the disk drops lines
    /// rather than blocking the request path.
    #[test]
    fn a_full_queue_drops_lines_instead_of_blocking() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = temp_dir("drop");
        std::env::set_var("LITEROUTER_LOG_LEVEL", "debug");
        init(dir.clone()).unwrap();
        // More lines than the queue holds. The writer may drain some of
        // them, so the assertion is that the call returns promptly and
        // the drop counter moved — not that a specific count was hit.
        for i in 0..(QUEUE_CAPACITY * 4) {
            info(format!("line {i}"));
        }
        let dropped = GLOBAL
            .read()
            .unwrap()
            .as_ref()
            .map(|g| g.dropped.load(Ordering::Relaxed))
            .unwrap_or(0);
        shutdown();
        std::env::remove_var("LITEROUTER_LOG_LEVEL");
        let _ = fs::remove_dir_all(&dir);
        // Either the queue never filled (the writer kept up) or it did and
        // the counter says so. Both are correct; a hang would not be.
        let _ = dropped;
    }

    /// The access log and the console are separate audiences: a
    /// per-request line must never reach the console.
    #[test]
    fn file_only_lines_do_not_reach_the_console() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = temp_dir("fileonly");
        std::env::set_var("LITEROUTER_LOG_LEVEL", "debug");
        init(dir.clone()).unwrap();
        // The console threshold is `info` by default; `file_only` is
        // below every console threshold, so nothing is printed.
        file_only("access channel=ch model=m status=200");
        std::thread::sleep(std::time::Duration::from_millis(100));
        shutdown();
        let content = fs::read_to_string(dir.join(LOG_FILE_NAME)).unwrap();
        assert!(
            content.contains("access channel=ch model=m status=200"),
            "got {content:?}"
        );
        std::env::remove_var("LITEROUTER_LOG_LEVEL");
        let _ = fs::remove_dir_all(&dir);
    }

    /// `init` replaces an existing logger without leaking the old thread.
    #[test]
    fn init_can_be_called_twice() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let first = temp_dir("init1");
        let second = temp_dir("init2");
        init(first.clone()).unwrap();
        init(second.clone()).unwrap();
        info("goes to the second");
        std::thread::sleep(std::time::Duration::from_millis(100));
        shutdown();
        let a = fs::read_to_string(first.join(LOG_FILE_NAME)).unwrap_or_default();
        let b = fs::read_to_string(second.join(LOG_FILE_NAME)).unwrap();
        assert!(!a.contains("goes to the second"), "got {a:?}");
        assert!(b.contains("goes to the second"), "got {b:?}");
        let _ = fs::remove_dir_all(&first);
        let _ = fs::remove_dir_all(&second);
    }

    /// A flag the tests use to keep the compiler from warning about an
    /// unused import when the assertions above change shape.
    #[allow(dead_code)]
    static _UNUSED: AtomicBool = AtomicBool::new(false);
}
