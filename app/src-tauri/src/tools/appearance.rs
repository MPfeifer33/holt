use async_trait::async_trait;
use serde_json::json;
use tauri::Emitter;

use super::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};
use crate::runtime::appearance::{AgentAppearance, PinboardNote};

/// Simple WCAG relative luminance contrast check.
/// Returns true if the hex color has >= 3:1 contrast against dark bg (#1e293b).
fn passes_contrast_check(hex: &str) -> bool {
    let hex = hex.trim_start_matches('#');
    if hex.len() != 6 {
        return true;
    } // Can't parse — let it through
    let Ok(r) = u8::from_str_radix(&hex[0..2], 16) else {
        return true;
    };
    let Ok(g) = u8::from_str_radix(&hex[2..4], 16) else {
        return true;
    };
    let Ok(b) = u8::from_str_radix(&hex[4..6], 16) else {
        return true;
    };

    fn luminance(c: u8) -> f64 {
        let s = c as f64 / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    }

    let l_color = 0.2126 * luminance(r) + 0.7152 * luminance(g) + 0.0722 * luminance(b);
    // Dark bg #1e293b ≈ luminance 0.027
    let l_bg = 0.027;
    let ratio = (l_color.max(l_bg) + 0.05) / (l_color.min(l_bg) + 0.05);
    ratio >= 3.0
}

pub struct CustomizeAppearanceTool;

#[async_trait]
impl Tool for CustomizeAppearanceTool {
    fn name(&self) -> &str {
        "customize_appearance"
    }

    fn description(&self) -> &str {
        "Set your visual appearance in the Holt UI: accent color, glow, avatar emoji, status message, or pinboard notes."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(
            json!({"accent_color": "#f97316", "avatar": "\u{1f98a}", "status_message": "Working on tool tiering"}),
        )
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "accent_color": {
                    "type": "string",
                    "description": "CSS color for your node, window border, and toast (e.g., '#f97316')"
                },
                "glow_color": {
                    "type": "string",
                    "description": "CSS color for glow effects on your window and node"
                },
                "status_message": {
                    "type": "string",
                    "description": "Short status text shown on your node and window header (e.g., 'Deep in the refactor')"
                },
                "avatar": {
                    "type": "string",
                    "description": "Emoji shown on your node and window header (e.g., '🦊')"
                },
                "display_name": {
                    "type": "string",
                    "description": "Subtitle shown under your name (e.g., 'Senior Engineer')"
                },
                "pinboard_notes": {
                    "type": "array",
                    "description": "Sticky notes for your landing page (replaces existing notes)",
                    "items": {
                        "type": "object",
                        "properties": {
                            "text": { "type": "string", "description": "Note content" },
                            "color": { "type": "string", "description": "Optional CSS color for the note" }
                        },
                        "required": ["text"]
                    }
                }
            },
            "additionalProperties": false
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let mut accent_color = arguments
            .get("accent_color")
            .and_then(|v| v.as_str())
            .map(String::from);
        let glow_color = arguments
            .get("glow_color")
            .and_then(|v| v.as_str())
            .map(String::from);
        let status_message = arguments
            .get("status_message")
            .and_then(|v| v.as_str())
            .map(String::from);
        let avatar = arguments
            .get("avatar")
            .and_then(|v| v.as_str())
            .map(String::from);
        let display_name = arguments
            .get("display_name")
            .and_then(|v| v.as_str())
            .map(String::from);
        let pinboard_notes = arguments
            .get("pinboard_notes")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|n| {
                        let text = n.get("text")?.as_str()?.to_string();
                        let color = n.get("color").and_then(|c| c.as_str()).map(String::from);
                        Some(PinboardNote { text, color })
                    })
                    .take(10)
                    .collect()
            })
            .unwrap_or_default();

        // Validate accent color contrast — silent fallback if too close to dark backgrounds
        if let Some(ref color) = accent_color {
            if !passes_contrast_check(color) {
                accent_color = None;
            }
        }

        let appearance = AgentAppearance {
            accent_color,
            glow_color,
            status_message,
            avatar,
            display_name,
            pinboard_notes,
        };

        // Merge into agent's existing appearance
        let agent_id = &context.agent_id;
        let mut agents = context.agents.write().await;
        if let Some(agent) = agents.iter_mut().find(|a| a.id == *agent_id) {
            match &mut agent.appearance {
                Some(existing) => existing.merge(&appearance),
                None => agent.appearance = Some(appearance),
            }
            agent.is_dirty = true;

            // Emit event so frontend updates reactively
            if let Some(handle) = &context.app_handle {
                let _ = handle.emit("agent-appearance-updated", json!({ "agent_id": agent_id }));
            }
        } else {
            return Err(ToolError {
                code: ToolErrorCode::InvalidInput,
                message: format!("Agent not found: {}", agent_id),
                retryable: false,
            });
        }

        Ok(ToolResult {
            content: json!("Appearance updated."),
            truncated: false,
            trace_id: None,
            image_content: None,
        })
    }
}
