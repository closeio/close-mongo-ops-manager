//! Application log: written to a file and kept in memory for the log viewer.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use log::{Level, LevelFilter, Log, Metadata, Record};

/// Number of log lines kept in memory.
pub const MAX_LINES: usize = 5000;

/// In-memory tail of the application log, shared with the log viewer.
#[derive(Debug, Clone, Default)]
pub struct LogBuffer {
    inner: Arc<Mutex<LogLines>>,
}

#[derive(Debug, Default)]
struct LogLines {
    lines: VecDeque<String>,
    /// Lines ever pushed, including the ones dropped from the front.
    total: u64,
}

impl LogBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a (possibly multi-line) message.
    pub fn push(&self, message: &str) {
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        for line in message.lines() {
            if inner.lines.len() == MAX_LINES {
                inner.lines.pop_front();
            }
            inner.lines.push_back(line.to_owned());
            inner.total += 1;
        }
    }

    /// A copy of the buffered lines, oldest first.
    pub fn lines(&self) -> Vec<String> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.lines.iter().cloned().collect()
    }

    /// Number of buffered lines.
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .lines
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Lines ever pushed; changes whenever a line is added.
    pub fn total(&self) -> u64 {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).total
    }
}

struct Logger {
    level: LevelFilter,
    file: Option<Mutex<File>>,
    buffer: LogBuffer,
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= self.level && is_own_target(metadata.target())
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format_line(
            &chrono::Local::now()
                .format("%Y-%m-%d %H:%M:%S,%3f")
                .to_string(),
            record.level(),
            &record.args().to_string(),
        );
        if let Some(file) = &self.file {
            let mut file = file.lock().unwrap_or_else(|e| e.into_inner());
            // Logging must never take the application down.
            let _ = writeln!(file, "{line}");
        }
        self.buffer.push(&line);
    }

    fn flush(&self) {
        if let Some(file) = &self.file {
            let _ = file.lock().unwrap_or_else(|e| e.into_inner()).flush();
        }
    }
}

/// Only our own records: dependencies log through `log` too, and their
/// records are not useful in the log viewer.
fn is_own_target(target: &str) -> bool {
    target.starts_with("close_mongo_ops_manager")
}

/// Formats a log line like the Python version:
/// `2026-10-08 12:00:00,123 (INFO): message`.
///
/// Messages carry text from the database (other users' commands, server
/// errors), so control characters (CR, LF, ESC, BEL, DEL, C1, ...), Unicode
/// line/paragraph separators and bidi controls are escaped: one record is
/// always one line, and no raw control bytes reach the log file, the log
/// viewer or a terminal showing the file.
pub fn format_line(timestamp: &str, level: Level, message: &str) -> String {
    let mut escaped = String::with_capacity(message.len());
    for c in message.chars() {
        if c.is_control()
            || matches!(
                c,
                '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}'
            )
        {
            escaped.extend(c.escape_default());
        } else {
            escaped.push(c);
        }
    }
    format!("{timestamp} ({}): {escaped}", level_name(level))
}

fn level_name(level: Level) -> &'static str {
    match level {
        Level::Error => "ERROR",
        Level::Warn => "WARNING",
        Level::Info => "INFO",
        Level::Debug => "DEBUG",
        Level::Trace => "TRACE",
    }
}

static BUFFER: OnceLock<LogBuffer> = OnceLock::new();

/// Creates the log file afresh: a new owner-only (0600) file is created
/// exclusively next to `path` and renamed over it. A symbolic link, FIFO or
/// another user's file planted under the log name is replaced, never
/// followed, reused or truncated.
fn create_log_file(path: &Path) -> io::Result<File> {
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let name = path.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "the log path has no file name")
    })?;
    let mut prefix = std::ffi::OsString::from(".");
    prefix.push(name);
    prefix.push(".");
    // O_CREAT|O_EXCL with mode 0600 on Unix.
    let file = tempfile::Builder::new().prefix(&prefix).tempfile_in(dir)?;
    file.persist(path).map_err(|e| e.error)
}

/// Installs the global logger, replacing `path` with a new owner-only file.
/// If the file cannot be created, logging continues in memory only and the
/// error is returned alongside the buffer so it can be reported.
pub fn init(path: &Path, level: LevelFilter) -> (LogBuffer, Option<io::Error>) {
    if let Some(buffer) = BUFFER.get() {
        return (buffer.clone(), None);
    }
    let buffer = BUFFER.get_or_init(LogBuffer::new).clone();
    let (file, error) = match create_log_file(path) {
        Ok(file) => (Some(Mutex::new(file)), None),
        Err(e) => (None, Some(e)),
    };
    let logger = Logger {
        level,
        file,
        buffer: buffer.clone(),
    };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(level);
    }
    (buffer, error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_keeps_the_most_recent_lines() {
        let buffer = LogBuffer::new();
        for i in 0..(MAX_LINES + 10) {
            buffer.push(&format!("line {i}"));
        }
        let lines = buffer.lines();
        assert_eq!(lines.len(), MAX_LINES);
        assert_eq!(lines[0], "line 10");
        assert_eq!(buffer.total(), (MAX_LINES + 10) as u64);
    }

    #[test]
    fn multi_line_messages_are_split() {
        let buffer = LogBuffer::new();
        buffer.push("first\nsecond");
        assert_eq!(buffer.lines(), vec!["first", "second"]);
        assert_eq!(buffer.len(), 2);
    }

    #[test]
    fn line_format_matches_python_version() {
        assert_eq!(
            format_line("2026-10-08 12:00:00,123", Level::Warn, "careful"),
            "2026-10-08 12:00:00,123 (WARNING): careful"
        );
    }

    #[test]
    fn records_are_single_lines_without_control_bytes() {
        let line = format_line(
            "2026-10-08 12:00:00,000",
            Level::Info,
            "x\n2026-10-08 12:00:00,000 (INFO): forged\r\u{1b}]0;t\u{7}\u{9b}31m\u{202e}",
        );
        assert!(!line.chars().any(char::is_control));
        assert!(!line.contains('\u{202e}'));
        assert!(line.contains("x\\n2026-10-08"));
    }

    #[cfg(unix)]
    #[test]
    fn log_file_replaces_planted_entries_and_is_private() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

        let dir = tempfile::tempdir().unwrap();
        let victim = dir.path().join("victim");
        std::fs::write(&victim, "keep me").unwrap();
        let log = dir.path().join("app.log");

        // A symbolic link to another file is replaced, not followed.
        symlink(&victim, &log).unwrap();
        let mut file = create_log_file(&log).unwrap();
        writeln!(file, "hello").unwrap();
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep me");
        let meta = std::fs::symlink_metadata(&log).unwrap();
        assert!(meta.file_type().is_file());
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);

        // An existing world-writable file is replaced by a new private one.
        std::fs::remove_file(&log).unwrap();
        std::fs::write(&log, "old").unwrap();
        std::fs::set_permissions(&log, std::fs::Permissions::from_mode(0o666)).unwrap();
        let old_inode = std::fs::metadata(&log).unwrap().ino();
        create_log_file(&log).unwrap();
        let meta = std::fs::metadata(&log).unwrap();
        assert_ne!(meta.ino(), old_inode);
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        assert_eq!(meta.len(), 0);
    }

    #[test]
    fn only_own_targets_are_logged() {
        assert!(is_own_target("close_mongo_ops_manager::mongo"));
        assert!(!is_own_target("mongodb::cmap"));
    }
}
