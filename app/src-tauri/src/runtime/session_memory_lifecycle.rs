use chrono::Utc;
use serde::Serialize;
use std::fs::OpenOptions;
use std::io::Write;

use crate::config::app_config::base_config_dir;
use crate::trace::engine::{TraceEngine, TraceInput};
use crate::trace::truncation::{TraceOutcome, TraceType};

#[derive(Debug, Clone, Serialize)]
pub struct SessionMemoryStoreReceipt {
    pub session_memory_id: String,
    pub agent_ns: String,
    pub session_id: String,
    pub tags: Vec<String>,
    pub stored_memory_id: String,
    pub bridge_status: String,
    pub bridge_message: String,
    pub verification_status: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionMemoryLifecycleReport {
    pub source: String,
    pub agent_id: String,
    pub agent_ns: Option<String>,
    pub session_id: String,
    pub session_memory_id: String,
    pub trigger_tokens: u64,
    pub context_cap: u64,
    pub session_file_path: String,
    pub backup_file_path: Option<String>,
    pub fork_status: String,
    pub fork_error: Option<String>,
    pub fork_turns: Option<usize>,
    pub validation_status: String,
    pub bridge_store_attempted: bool,
    pub bridge_store_status: Option<String>,
    pub bridge_store_error: Option<String>,
    pub receipt: Option<SessionMemoryStoreReceipt>,
}

#[derive(Serialize)]
struct SessionMemoryLifecycleLogLine<'a> {
    timestamp: String,
    trace_id: String,
    report: &'a SessionMemoryLifecycleReport,
}

pub fn write_session_memory_lifecycle_trace(
    trace_engine: &TraceEngine,
    report: &SessionMemoryLifecycleReport,
) -> Result<String, Box<dyn std::error::Error>> {
    let outcome = if report.fork_status != "completed" {
        TraceOutcome::Partial {
            details: report
                .fork_error
                .clone()
                .unwrap_or_else(|| report.fork_status.clone()),
        }
    } else if report.validation_status != "valid" {
        TraceOutcome::Partial {
            details: report.validation_status.clone(),
        }
    } else if let Some(error) = &report.bridge_store_error {
        TraceOutcome::Partial {
            details: error.clone(),
        }
    } else if matches!(
        report.bridge_store_status.as_deref(),
        Some("skipped_unavailable") | Some("skipped_empty")
    ) {
        TraceOutcome::Partial {
            details: report
                .bridge_store_status
                .clone()
                .unwrap_or_else(|| "bridge_store_skipped".to_string()),
        }
    } else {
        TraceOutcome::Success
    };

    let trace_id = trace_engine.write_trace(TraceInput {
        agent_id: report.agent_id.clone(),
        trace_type: TraceType::SessionMemoryLifecycle,
        content: serde_json::to_value(report)?,
        outcome: Some(outcome),
        duration_ms: None,
        token_count: None,
        parent_trace_id: None,
        session_id: report.session_id.clone(),
        prompt_tokens: None,
        completion_tokens: None,
        cost_usd_cents: None,
    })?;

    if let Err(e) = append_session_memory_lifecycle_log(&trace_id, report) {
        tracing::warn!(
            agent_id = report.agent_id,
            error = %e,
            "session memory lifecycle disk log append failed"
        );
    }

    Ok(trace_id)
}

fn append_session_memory_lifecycle_log(
    trace_id: &str,
    report: &SessionMemoryLifecycleReport,
) -> Result<(), Box<dyn std::error::Error>> {
    let log_dir = base_config_dir().join("logs");
    std::fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join("session_memory_lifecycle.jsonl");

    let line = serde_json::to_string(&SessionMemoryLifecycleLogLine {
        timestamp: Utc::now().to_rfc3339(),
        trace_id: trace_id.to_string(),
        report,
    })?;

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")?;
    Ok(())
}
