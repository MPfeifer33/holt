//! A2A message logger — writes all inter-agent messages to daily gzipped log files.
//!
//! Log location: ~/.local/share/holt/logs/a2a/YYYY-MM-DD.log.gz
//! Format: [ISO_TIMESTAMP] FROM_AGENT -> TO_AGENT: message content

use chrono::{NaiveDate, Utc};
use flate2::write::GzEncoder;
use flate2::Compression;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// Thread-safe A2A log writer. Holds a gzip encoder for the current day's file.
pub struct A2ALogger {
    log_dir: PathBuf,
    inner: Mutex<LoggerInner>,
}

struct LoggerInner {
    current_date: Option<NaiveDate>,
    encoder: Option<GzEncoder<File>>,
}

impl A2ALogger {
    /// Create a new logger. Creates the log directory if it doesn't exist.
    pub fn new() -> Self {
        let log_dir = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join("holt")
            .join("logs")
            .join("a2a");

        if let Err(e) = fs::create_dir_all(&log_dir) {
            tracing::warn!("Failed to create A2A log directory: {e}");
        }

        Self {
            log_dir,
            inner: Mutex::new(LoggerInner {
                current_date: None,
                encoder: None,
            }),
        }
    }

    /// Log a message. Rotates the file daily.
    pub fn log_message(&self, from_name: &str, to_name: &str, content: &str) {
        let now = Utc::now();
        let today = now.date_naive();
        let timestamp = now.to_rfc3339();

        let mut inner = match self.inner.lock() {
            Ok(guard) => guard,
            Err(e) => {
                tracing::error!("A2A logger mutex poisoned: {e}");
                return;
            }
        };

        // Rotate if day changed or no encoder yet
        if inner.current_date != Some(today) {
            // Flush and drop old encoder
            if let Some(encoder) = inner.encoder.take() {
                let _ = encoder.finish();
            }

            let filename = format!("{}.log.gz", today.format("%Y-%m-%d"));
            let path = self.log_dir.join(filename);

            match OpenOptions::new().create(true).append(true).open(&path) {
                Ok(file) => {
                    inner.encoder = Some(GzEncoder::new(file, Compression::default()));
                    inner.current_date = Some(today);
                }
                Err(e) => {
                    tracing::warn!("Failed to open A2A log file: {e}");
                    return;
                }
            }
        }

        // Write the log line
        if let Some(ref mut encoder) = inner.encoder {
            let line = format!("[{timestamp}] {from_name} -> {to_name}: {content}\n");
            if let Err(e) = encoder.write_all(line.as_bytes()) {
                tracing::warn!("Failed to write A2A log entry: {e}");
            }
            // Flush periodically to ensure data isn't lost on crash
            let _ = encoder.flush();
        }
    }
}

impl Drop for A2ALogger {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.inner.lock() {
            if let Some(encoder) = inner.encoder.take() {
                let _ = encoder.finish();
            }
        }
    }
}
