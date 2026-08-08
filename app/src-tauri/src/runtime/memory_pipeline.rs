use chrono::Utc;
use serde::Serialize;
use std::fs::OpenOptions;
use std::io::Write;

use crate::config::app_config::base_config_dir;
use crate::memory::injection::MemoryContextReport;
use crate::trace::engine::{TraceEngine, TraceInput};
use crate::trace::truncation::{TraceOutcome, TraceType};

#[derive(Debug, Clone, Serialize)]
pub struct MemoryPipelineReport {
    pub source: String,
    pub agent_id: String,
    pub agent_ns: Option<String>,
    pub session_id: String,
    pub current_message: String,
    pub recent_messages: Vec<String>,
    pub memory_enabled: bool,
    pub memory_status: String,
    pub memory_error: Option<String>,
    pub a2a_messages_injected: usize,
    pub subagent_results_injected: usize,
    pub memory_context: Option<MemoryContextReport>,
}

#[derive(Serialize)]
struct MemoryPipelineLogLine<'a> {
    timestamp: String,
    trace_id: String,
    report: &'a MemoryPipelineReport,
}

pub fn write_memory_pipeline_trace(
    trace_engine: &TraceEngine,
    report: &MemoryPipelineReport,
) -> Result<String, Box<dyn std::error::Error>> {
    let trace_id = trace_engine.write_trace(TraceInput {
        agent_id: report.agent_id.clone(),
        trace_type: TraceType::MemoryPipeline,
        content: serde_json::to_value(report)?,
        outcome: Some(match report.memory_status.as_str() {
            "error" => TraceOutcome::Partial {
                details: report
                    .memory_error
                    .clone()
                    .unwrap_or_else(|| "memory_pipeline_error".to_string()),
            },
            _ => TraceOutcome::Success,
        }),
        duration_ms: None,
        token_count: None,
        parent_trace_id: None,
        session_id: report.session_id.clone(),
        prompt_tokens: None,
        completion_tokens: None,
        cost_usd_cents: None,
    })?;

    if let Err(e) = append_memory_pipeline_log(&trace_id, report) {
        tracing::warn!(
            agent_id = report.agent_id,
            error = %e,
            "memory pipeline disk log append failed"
        );
    }

    Ok(trace_id)
}

fn append_memory_pipeline_log(
    trace_id: &str,
    report: &MemoryPipelineReport,
) -> Result<(), Box<dyn std::error::Error>> {
    let log_dir = base_config_dir().join("logs");
    std::fs::create_dir_all(&log_dir)?;
    let log_path = log_dir.join("memory_pipeline.jsonl");

    let line = serde_json::to_string(&MemoryPipelineLogLine {
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
