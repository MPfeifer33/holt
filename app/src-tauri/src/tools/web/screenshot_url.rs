use serde_json::json;
use std::process::Stdio;
use tokio::process::Command;

use crate::runtime::agent::ContentBlock;
use crate::tools::types::{Tool, ToolContext, ToolError, ToolErrorCode, ToolResult};

pub struct ScreenshotUrlTool;

#[async_trait::async_trait]
impl Tool for ScreenshotUrlTool {
    fn name(&self) -> &str {
        "screenshot_url"
    }

    fn description(&self) -> &str {
        "Take a screenshot of a web page using a headless browser."
    }

    fn example(&self) -> Option<serde_json::Value> {
        Some(json!({"url": "https://example.com"}))
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "The URL to screenshot" },
                "full_page": { "type": "boolean", "description": "Capture the full scrollable page (default: false)" },
                "width": { "type": "integer", "description": "Viewport width in pixels (default: 1280)" },
                "height": { "type": "integer", "description": "Viewport height in pixels (default: 720)" }
            },
            "required": ["url"]
        })
    }

    async fn execute(
        &self,
        arguments: serde_json::Value,
        _context: &ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let url = arguments
            .get("url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError {
                code: ToolErrorCode::InvalidInput,
                message: "Missing required parameter: url".to_string(),
                retryable: false,
            })?;

        let full_page = arguments
            .get("full_page")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let width = arguments
            .get("width")
            .and_then(|v| v.as_u64())
            .unwrap_or(1280);
        let height = arguments
            .get("height")
            .and_then(|v| v.as_u64())
            .unwrap_or(720);

        // Build inline Node.js script for Playwright
        // URL is passed via environment variable to prevent JS code injection
        let script = format!(
            r#"
const {{ chromium }} = require('playwright');
(async () => {{
    const url = process.env.SCREENSHOT_TARGET_URL;
    if (!url) {{ process.stderr.write('SCREENSHOT_TARGET_URL not set'); process.exit(1); }}
    const browser = await chromium.launch();
    const page = await browser.newPage({{ viewport: {{ width: {width}, height: {height} }} }});
    await page.goto(url, {{ waitUntil: 'networkidle', timeout: 30000 }}).catch(() => {{}});
    const buf = await page.screenshot({{ fullPage: {full_page}, type: 'png' }});
    process.stdout.write(buf.toString('base64'));
    await browser.close();
}})();
"#,
            width = width,
            height = height,
            full_page = if full_page { "true" } else { "false" },
        );

        let output = Command::new("node")
            .arg("-e")
            .arg(&script)
            .env("SCREENSHOT_TARGET_URL", url)
            .kill_on_drop(true)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| ToolError {
                code: ToolErrorCode::InternalError,
                message: format!(
                    "Failed to run screenshot script: {}. Is Node.js installed?",
                    e
                ),
                retryable: true,
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let msg = stderr.chars().take(500).collect::<String>();
            if msg.contains("Cannot find module") && msg.contains("playwright") {
                return Err(ToolError {
                    code: ToolErrorCode::InternalError,
                    message: "Playwright not installed. Run: npx playwright install chromium"
                        .to_string(),
                    retryable: false,
                });
            }
            return Err(ToolError {
                code: ToolErrorCode::InternalError,
                message: format!("Screenshot failed: {}", msg),
                retryable: true,
            });
        }

        let base64_data = String::from_utf8_lossy(&output.stdout).to_string();

        if base64_data.is_empty() {
            return Err(ToolError {
                code: ToolErrorCode::InternalError,
                message: "Screenshot returned empty data".to_string(),
                retryable: true,
            });
        }

        Ok(ToolResult {
            content: json!({
                "type": "screenshot",
                "url": url,
                "width": width,
                "height": height,
                "full_page": full_page,
                "media_type": "image/png",
            }),
            truncated: false,
            trace_id: None,
            image_content: Some(vec![ContentBlock::Image {
                data: base64_data,
                media_type: "image/png".to_string(),
            }]),
        })
    }
}
