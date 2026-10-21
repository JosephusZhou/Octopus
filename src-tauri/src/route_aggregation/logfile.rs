//! On-disk log file for route aggregation entries.
//!
//! Format: JSON Lines (one serialized `LogEntry` per line, `\n` terminated).
//! The active file is changed at local midnight and named
//! `route_aggregation_YYYY-MM-DD.log`. Files older than seven calendar days
//! are removed by the startup cleanup and by a background periodic cleanup.
//!
//! Failures to open or write the file are swallowed: the in-memory log stays
//! usable, and we don't want a permission glitch to take down the proxy.

use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use chrono::{Local, NaiveDate};

use super::log::DiskLogEntry;

/// One serialized line on disk. Held inside a `Mutex` so concurrent pushers
/// don't interleave their writes.
struct Inner {
    writer: BufWriter<File>,
    base_path: PathBuf,
    log_dir: PathBuf,
    path: PathBuf,
    /// 每日文件的首条日志 ID 区间（按日期升序）：`(date, first_id)`。
    ///
    /// 用于按 id 定位所属日期文件，实现日志详情的按需读取。date 切换时由
    /// `write_entry` 追加；历史文件的首条 id 在 `open` 时扫描首行得到。
    rotations: Vec<(NaiveDate, u64)>,
}

/// Keep only the current day and the six preceding calendar days.
const LOG_RETENTION_DAYS: i64 = 7;

/// Recheck retention even when there is no incoming proxy traffic.
const CLEANUP_INTERVAL: Duration = Duration::from_secs(60 * 60);

const LOG_FILE_EXTENSION: &str = "log";

fn log_file_parts(base_path: &Path) -> (String, String) {
    let stem = base_path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("route_aggregation")
        .to_string();
    let extension = base_path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or(LOG_FILE_EXTENSION)
        .to_string();
    (stem, extension)
}

fn daily_path(base_path: &Path, date: NaiveDate) -> PathBuf {
    let (stem, extension) = log_file_parts(base_path);
    base_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{stem}_{}.{extension}", date.format("%Y-%m-%d")))
}

fn log_file_prefix(base_path: &Path) -> String {
    let (stem, _) = log_file_parts(base_path);
    format!("{stem}_")
}

fn log_file_date(name: &str, prefix: &str, extension: &str) -> Option<NaiveDate> {
    let suffix = format!(".{extension}");
    let date = name.strip_prefix(prefix)?.strip_suffix(&suffix)?;
    if date.len() != 10 {
        return None;
    }
    NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()
}

/// 读取某日志文件的首行日志 ID（用于初始化每日 id 区间）。
fn read_first_entry_id(path: &Path) -> Option<u64> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut line = String::new();
    let bytes = reader.read_line(&mut line).ok()?;
    if bytes == 0 {
        return None;
    }
    serde_json::from_str::<DiskLogEntry>(&line)
        .ok()
        .map(|entry| entry.id)
}

/// 扫描日志目录，构建「每日文件 → 首条日志 ID」区间（按日期升序）。
///
/// 空文件、解析失败的行会被跳过；当日文件由 `write_entry` 在首次写入时补充区间。
fn collect_rotations(log_dir: &Path, base_path: &Path) -> Vec<(NaiveDate, u64)> {
    let (_, extension) = log_file_parts(base_path);
    let prefix = log_file_prefix(base_path);
    let mut files: Vec<(NaiveDate, PathBuf)> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(log_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Some(date) = log_file_date(&name, &prefix, &extension) else {
                continue;
            };
            files.push((date, path));
        }
    }
    files.sort_by_key(|(date, _)| *date);

    let mut rotations = Vec::with_capacity(files.len());
    for (date, path) in files {
        if let Some(first_id) = read_first_entry_id(&path) {
            rotations.push((date, first_id));
        }
    }
    rotations
}

fn cleanup_old_files(dir: &Path, base_path: &Path, today: NaiveDate) {
    let (_, extension) = log_file_parts(base_path);
    let prefix = log_file_prefix(base_path);
    let cutoff = today - chrono::Duration::days(LOG_RETENTION_DAYS - 1);
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            eprintln!(
                "[route-aggregation] failed to read log dir {}: {}",
                dir.display(),
                error
            );
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(date) = log_file_date(&name, &prefix, &extension) else {
            continue;
        };
        if date < cutoff {
            if let Err(error) = std::fs::remove_file(&path) {
                eprintln!(
                    "[route-aggregation] failed to remove expired log {}: {}",
                    path.display(),
                    error
                );
            }
        }
    }
}

fn clear_other_files(dir: &Path, base_path: &Path, active_path: &Path) {
    let (_, extension) = log_file_parts(base_path);
    let prefix = log_file_prefix(base_path);
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            eprintln!(
                "[route-aggregation] failed to read log dir {} before clear: {}",
                dir.display(),
                error
            );
            return;
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if log_file_date(name, &prefix, &extension).is_none() || path == active_path {
            continue;
        }
        if let Err(error) = std::fs::remove_file(&path) {
            eprintln!(
                "[route-aggregation] failed to clear log {}: {}",
                path.display(),
                error
            );
        }
    }
}

fn start_cleanup_worker(inner: &Arc<Mutex<Inner>>) {
    let weak: Weak<Mutex<Inner>> = Arc::downgrade(inner);
    let _ = std::thread::Builder::new()
        .name("route-aggregation-log-cleanup".to_string())
        .spawn(move || loop {
            std::thread::sleep(CLEANUP_INTERVAL);
            let Some(inner) = weak.upgrade() else {
                break;
            };
            let guard = match inner.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            cleanup_old_files(&guard.log_dir, &guard.base_path, Local::now().date_naive());
        });
}

/// Cheap, thread-safe handle to the current daily on-disk log file.
#[derive(Clone)]
pub struct LogFile {
    inner: Arc<Mutex<Inner>>,
}

impl LogFile {
    /// Open the daily log derived from `path`, creating its parent directory
    /// if needed. The supplied path is a base path; for example,
    /// `route_aggregation.log` becomes `route_aggregation_2026-08-15.log`.
    pub fn open(path: &Path) -> Option<Self> {
        if let Some(dir) = path.parent() {
            if let Err(error) = std::fs::create_dir_all(dir) {
                eprintln!(
                    "[route-aggregation] failed to create log dir {}: {}",
                    dir.display(),
                    error
                );
                return None;
            }
        }

        let log_dir = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        let today = Local::now().date_naive();
        cleanup_old_files(&log_dir, path, today);
        let active_path = daily_path(path, today);
        let file = match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&active_path)
        {
            Ok(file) => file,
            Err(error) => {
                eprintln!(
                    "[route-aggregation] failed to open log file {}: {}",
                    active_path.display(),
                    error
                );
                return None;
            }
        };

        let rotations = collect_rotations(&log_dir, path);
        let inner = Arc::new(Mutex::new(Inner {
            writer: BufWriter::new(file),
            base_path: path.to_path_buf(),
            log_dir,
            path: active_path,
            rotations,
        }));
        start_cleanup_worker(&inner);
        Some(Self { inner })
    }

    /// Returns the active date's path for surfacing in the UI.
    pub fn path(&self) -> PathBuf {
        self.inner.lock().unwrap().path.clone()
    }

    /// Serialize `entry` and append it as a single JSON line. Failures are
    /// logged to stderr but never propagate — the in-memory ring is the
    /// primary UI surface; the file is best-effort.
    ///
    /// Also maintains the per-day first-ID index (`rotations`) so log details
    /// can be read back on demand by ID.
    pub fn write_entry(&self, entry: &DiskLogEntry) {
        let mut inner = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let line = match serde_json::to_string(entry) {
            Ok(line) => line,
            Err(error) => {
                eprintln!("[route-aggregation] serialize log entry failed: {}", error);
                return;
            }
        };

        let today_path = daily_path(&inner.base_path, Local::now().date_naive());
        if today_path != inner.path {
            if let Err(error) = inner.writer.flush() {
                eprintln!(
                    "[route-aggregation] flush before daily rotation failed: {}",
                    error
                );
            }
            match OpenOptions::new()
                .create(true)
                .append(true)
                .open(&today_path)
            {
                Ok(file) => {
                    inner.writer = BufWriter::new(file);
                    inner.path = today_path;
                    // 新日期文件的第一行就是本次 entry：记录区间首条 id
                    inner.rotations.push((Local::now().date_naive(), entry.id));
                    cleanup_old_files(&inner.log_dir, &inner.base_path, Local::now().date_naive());
                }
                Err(error) => eprintln!(
                    "[route-aggregation] reopen daily log {} failed: {}",
                    today_path.display(),
                    error
                ),
            }
        } else if inner.rotations.last().map(|(date, _)| *date) != Some(Local::now().date_naive()) {
            // 同一日期文件但索引缺少今天的区间（例如 open 时文件为空、或 clear 之后）：
            // 用本次 entry 的 id 补齐，保证今天写入的 id 能定位到该文件。
            inner.rotations.push((Local::now().date_naive(), entry.id));
        }

        if let Err(error) = inner.writer.write_all(line.as_bytes()) {
            eprintln!("[route-aggregation] write log line failed: {}", error);
            return;
        }
        if let Err(error) = inner.writer.write_all(b"\n") {
            eprintln!("[route-aggregation] write log newline failed: {}", error);
            return;
        }
        // Flush on every line so `tail -f` sees entries immediately.
        if let Err(error) = inner.writer.flush() {
            eprintln!("[route-aggregation] flush log failed: {}", error);
        }
    }

    /// Read back the full on-disk entry with `id`.
    ///
    /// Locates the owning date file via the per-day first-ID index, then scans
    /// that file line by line until the id matches. Returns `None` when the id
    /// is not on disk (e.g. removed by the 7-day retention, cleared by the
    /// user, or written by an older format). The file is read without holding
    /// the writer lock, so a slow scan never blocks forwarding writes.
    pub fn read_entry_by_id(&self, id: u64) -> Option<DiskLogEntry> {
        let (base_path, dates) = {
            let inner = match self.inner.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            (inner.base_path.clone(), inner.candidate_dates_for_id(id))
        };
        let mut found = None;
        for date in dates {
            let path = daily_path(&base_path, date);
            let Ok(file) = File::open(path) else {
                continue;
            };
            let reader = BufReader::new(file);
            for line in reader.lines() {
                let Ok(line) = line else { continue };
                let Ok(entry) = serde_json::from_str::<DiskLogEntry>(&line) else {
                    continue;
                };
                if entry.id == id {
                    // 同一 id 可能有两行：先写「无响应体」行，流式响应结束后补写
                    // 「完整响应体」行。取最后一个匹配（最完整的一行）。
                    found = Some(entry);
                }
            }
        }
        found
    }

    /// Read the most recent `count` entries from the active daily file
    /// (newest first). Used to backfill the in-memory list after an app
    /// restart, where the memory ring starts empty. Returns fewer entries
    /// when the file has fewer lines. The file is read without holding the
    /// writer lock, so a slow scan never blocks forwarding writes.
    ///
    /// 读取方式为**文件尾部反向读**（JSONL 每行一个 JSON，行内无真实换行，可
    /// 按 `\n` 安全切行）：从文件尾向前按 64KB 块扫描，凑够 `count` 个换行即停，
    /// 避免每次刷新都全量扫描整份当天日志。`TAIL_CAP` 防止多行巨型 body
    /// （单行可达 16MB 保护上限）时尾部文本无限累积。
    pub fn read_recent_entries(&self, count: usize) -> Vec<DiskLogEntry> {
        if count == 0 {
            return Vec::new();
        }
        let path = {
            let inner = match self.inner.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            inner.path.clone()
        };
        let Ok(mut file) = File::open(path) else {
            return Vec::new();
        };
        let Ok(metadata) = file.metadata() else {
            return Vec::new();
        };
        let len = metadata.len();
        if len == 0 {
            return Vec::new();
        }

        const BLOCK: u64 = 64 * 1024;
        // 极端保护：多条 16MB 巨型单行时尾部文本最多累积到该值，防止内存峰值失控
        const TAIL_CAP: usize = 32 * 1024 * 1024;

        let mut end = len;
        let mut tail: Vec<u8> = Vec::new();
        let mut newline_count = 0usize;
        loop {
            if end == 0 || newline_count >= count || tail.len() >= TAIL_CAP {
                break;
            }
            let start = end.saturating_sub(BLOCK);
            let chunk_len = (end - start) as usize;
            let mut chunk = vec![0u8; chunk_len];
            if file.seek(std::io::SeekFrom::Start(start)).is_err()
                || file.read_exact(&mut chunk).is_err()
            {
                break;
            }
            newline_count += chunk.iter().filter(|&&b| b == b'\n').count();
            let mut new_tail = chunk;
            new_tail.extend_from_slice(&tail);
            tail = new_tail;
            end = start;
        }

        // 从尾部向前解析，收集最后 `count` 条可解析的行。text 开头可能是半行
        // （扫描起点落在行中间），从尾部遍历不受影响；解析失败的行直接跳过。
        let text = String::from_utf8_lossy(&tail);
        let mut recent: std::collections::VecDeque<DiskLogEntry> =
            std::collections::VecDeque::new();
        for line in text.lines().rev() {
            if let Ok(entry) = serde_json::from_str::<DiskLogEntry>(line) {
                recent.push_front(entry);
                if recent.len() >= count {
                    break;
                }
            }
        }
        recent.into_iter().collect()
    }

    /// Clear the active log after the in-memory view is cleared, and remove
    /// the other daily files created by this logger.
    pub fn clear(&self) {
        let mut inner = match self.inner.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Err(error) = inner.writer.flush() {
            eprintln!("[route-aggregation] flush before clear failed: {}", error);
            return;
        }
        if let Err(error) = inner.writer.get_ref().set_len(0) {
            eprintln!("[route-aggregation] clear log failed: {}", error);
            return;
        }
        clear_other_files(&inner.log_dir, &inner.base_path, &inner.path);
        // 区间索引已失效：下一次 write_entry 会用当天首条 id 重建区间。
        inner.rotations.clear();
    }
}

impl Inner {
    /// 根据 id 定位候选日期文件（按日期升序的 first-id 区间），返回最多两个：
    /// 常规定位文件 + 其后的下一个文件。
    ///
    /// 第二个候选用于跨午夜边界：流式请求在午夜前进入（首行落旧文件），流结束
    /// 于新一天时补写的完整行会随日期轮转写进**新日期文件**；详情读取必须在
    /// 旧文件未命中时顺扫下一个文件，才能取到最完整的那一行。
    fn candidate_dates_for_id(&self, id: u64) -> Vec<NaiveDate> {
        let index = self
            .rotations
            .partition_point(|(_, first_id)| *first_id <= id);
        let mut dates = Vec::with_capacity(2);
        if index == 0 {
            // id 比所有已知首条 id 都小：回落到最早的文件
            if let Some((date, _)) = self.rotations.first() {
                dates.push(*date);
            }
        } else if let Some((date, _)) = self.rotations.get(index - 1) {
            dates.push(*date);
        }
        // 下一个文件：跨午夜补写行可能落在其中
        if let Some((date, _)) = self.rotations.get(index) {
            dates.push(*date);
        }
        dates
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_log_dir() -> PathBuf {
        std::env::temp_dir().join(format!(
            "octopus-route-log-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn daily_path_appends_date_before_extension() {
        let base = Path::new("/tmp/logs/route_aggregation.log");
        let date = NaiveDate::from_ymd_opt(2026, 8, 15).unwrap();
        assert_eq!(
            daily_path(base, date),
            PathBuf::from("/tmp/logs/route_aggregation_2026-08-15.log")
        );
    }

    #[test]
    fn cleanup_keeps_seven_calendar_days_and_ignores_unrelated_files() {
        let dir = temp_log_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("route_aggregation.log");
        let today = NaiveDate::from_ymd_opt(2026, 8, 15).unwrap();
        for offset in 0..8 {
            let date = today - chrono::Duration::days(offset);
            std::fs::write(daily_path(&base, date), b"log").unwrap();
        }
        std::fs::write(dir.join("route_aggregation_notes.log"), b"keep").unwrap();

        cleanup_old_files(&dir, &base, today);

        assert!(!daily_path(&base, today - chrono::Duration::days(7)).exists());
        assert!(daily_path(&base, today - chrono::Duration::days(6)).exists());
        assert!(dir.join("route_aggregation_notes.log").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn open_uses_current_daily_file() {
        let dir = temp_log_dir();
        let base = dir.join("route_aggregation.log");
        let log = LogFile::open(&base).unwrap();
        assert_eq!(log.path(), daily_path(&base, Local::now().date_naive()));
        assert!(log.path().starts_with(&dir));
        drop(log);
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn sample_disk_entry(id: u64) -> DiskLogEntry {
        DiskLogEntry {
            id,
            timestamp_ms: 1000 + id as i64,
            protocol: super::super::log::InboundProtocol::ClaudeMessages,
            inbound_method: "POST".into(),
            inbound_path: "/v1/messages".into(),
            inbound_headers: vec![],
            inbound_body: Some(serde_json::json!({ "prompt": format!("prompt-{id}") })),
            inbound_body_truncated: false,
            inbound_model: None,
            upstream_model: None,
            provider_id: None,
            provider_name: None,
            upstream_url: None,
            upstream_status: None,
            upstream_headers: vec![],
            upstream_body: Some(format!("response-{id}")),
            upstream_body_truncated: false,
            stream: false,
            duration_ms: 1,
            success: true,
            error: None,
            full_body: true,
        }
    }

    #[test]
    fn read_entry_by_id_finds_written_entries() {
        let dir = temp_log_dir();
        let base = dir.join("route_aggregation.log");
        let log = LogFile::open(&base).unwrap();

        for id in 1..=5 {
            log.write_entry(&sample_disk_entry(id));
        }

        // 命中已写入的条目（全量 body 读回）
        let found = log.read_entry_by_id(3).expect("entry 3 should exist");
        assert_eq!(
            found.inbound_body,
            Some(serde_json::json!({ "prompt": "prompt-3" }))
        );
        assert_eq!(found.upstream_body.as_deref(), Some("response-3"));
        assert!(found.full_body);

        // 未命中：返回 None
        assert!(log.read_entry_by_id(999).is_none());

        drop(log);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reopen_builds_rotation_index_from_first_lines() {
        let dir = temp_log_dir();
        let base = dir.join("route_aggregation.log");
        {
            let log = LogFile::open(&base).unwrap();
            for id in 1..=5 {
                log.write_entry(&sample_disk_entry(id));
            }
        }
        // 重新 open：区间由文件首行扫描重建，仍能按 id 读回
        let log = LogFile::open(&base).unwrap();
        assert!(log.read_entry_by_id(2).is_some());
        assert!(log.read_entry_by_id(7).is_none());
        drop(log);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn read_recent_entries_returns_tail_with_limits() {
        let dir = temp_log_dir();
        let base = dir.join("route_aggregation.log");
        let log = LogFile::open(&base).unwrap();
        for id in 1..=5 {
            log.write_entry(&sample_disk_entry(id));
        }

        // 只取尾部 3 条，且为倒序（最新在前）
        let recent = log.read_recent_entries(3);
        let ids: Vec<u64> = recent.iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![3, 4, 5]);

        // 请求条数超过文件行数时返回全部
        let all = log.read_recent_entries(10);
        assert_eq!(all.len(), 5);

        // count=0 不读文件
        assert!(log.read_recent_entries(0).is_empty());

        drop(log);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn read_recent_entries_reads_tail_without_full_scan() {
        let dir = temp_log_dir();
        let base = dir.join("route_aggregation.log");
        let log = LogFile::open(&base).unwrap();
        for id in 1..=2 {
            log.write_entry(&sample_disk_entry(id));
        }
        // 最后一条单行远超 64KB 反向读块：应跨块完整取回，且不丢前一条
        let mut big = sample_disk_entry(3);
        big.upstream_body = Some("x".repeat(200 * 1024));
        log.write_entry(&big);

        let recent = log.read_recent_entries(2);
        let ids: Vec<u64> = recent.iter().map(|e| e.id).collect();
        assert_eq!(ids, vec![2, 3]);
        assert_eq!(
            recent.last().unwrap().upstream_body.as_ref().unwrap().len(),
            200 * 1024
        );

        drop(log);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn read_entry_by_id_scans_next_day_file_for_cross_midnight_line() {
        let dir = temp_log_dir();
        std::fs::create_dir_all(&dir).unwrap();
        let base = dir.join("route_aggregation.log");
        let today = Local::now().date_naive();
        let yesterday = today - chrono::Duration::days(1);

        // 昨天文件：流式请求在午夜前进入，首行「无响应体」
        let mut no_body = sample_disk_entry(1);
        no_body.upstream_body = None;
        std::fs::write(
            daily_path(&base, yesterday),
            format!("{}\n", serde_json::to_string(&no_body).unwrap()),
        )
        .unwrap();

        // 今天文件：流结束于新一天，补写的「完整响应体」行
        std::fs::write(
            daily_path(&base, today),
            format!(
                "{}\n",
                serde_json::to_string(&sample_disk_entry(1)).unwrap()
            ),
        )
        .unwrap();

        // open 重建区间（昨天+今天两文件首行 id 均为 1）
        let log = LogFile::open(&base).unwrap();
        let found = log
            .read_entry_by_id(1)
            .expect("cross-midnight full line should be found");
        assert!(found.upstream_body.is_some());
        assert_eq!(found.upstream_body.as_deref(), Some("response-1"));

        drop(log);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
