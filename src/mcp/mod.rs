use crate::app::platform;
use crate::ipc::{self, IpcRequest, IpcResponse};
use crate::screenshot;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    #[serde(default)]
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}

/// Runs the Model Context Protocol (MCP) server over standard I/O (stdin/stdout).
/// Any CLI agent (Antigravity CLI, Claude Code, Cursor, Aider) can invoke
/// `clicky-desktop --mcp` as an MCP server.
pub async fn run_mcp_server() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin);
    let mut line = String::new();

    while reader.read_line(&mut line).await? > 0 {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            line.clear();
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(req) => req,
            Err(e) => {
                let err_resp = JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: None,
                    result: None,
                    error: Some(json!({
                        "code": -32700,
                        "message": format!("Parse error: {}", e)
                    })),
                };
                let mut out = serde_json::to_string(&err_resp)?;
                out.push('\n');
                stdout.write_all(out.as_bytes()).await?;
                stdout.flush().await?;
                line.clear();
                continue;
            }
        };

        // If it's a notification (no id), handle and don't reply
        if request.id.is_none() {
            line.clear();
            continue;
        }

        let id = request.id;
        let response = match request.method.as_str() {
            "initialize" => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: Some(json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "clicky-desktop",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                })),
                error: None,
            },
            "tools/list" => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: Some(json!({
                    "tools": [
                        {
                            "name": "clicky_status",
                            "description": "Check if Clicky Desktop companion is running and get its current state (Idle, Listening, Processing, Responding).",
                            "inputSchema": {
                                "type": "object",
                                "properties": {}
                            }
                        },
                        {
                            "name": "clicky_screenshot",
                            "description": "Capture the user's desktop screen(s) in real-time. Returns JPEG image data for multimodal vision analysis.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "save_path": {
                                        "type": "string",
                                        "description": "Optional local file path to save the screenshot image"
                                    }
                                }
                            }
                        },
                        {
                            "name": "clicky_point",
                            "description": "Fly Clicky's blue triangle cursor smoothly across the desktop using Bézier flight to point at coordinates (x, y) with an optional text label.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "x": {
                                        "type": "number",
                                        "description": "Global X coordinate in pixels"
                                    },
                                    "y": {
                                        "type": "number",
                                        "description": "Global Y coordinate in pixels"
                                    },
                                    "label": {
                                        "type": "string",
                                        "description": "Text label pointing at the target"
                                    }
                                },
                                "required": ["x", "y"]
                            }
                        },
                        {
                            "name": "clicky_speak",
                            "description": "Speak aloud to the user through their speakers using Clicky's neural TTS engine (Edge-TTS or ElevenLabs).",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "text": {
                                        "type": "string",
                                        "description": "Text message to speak aloud"
                                    }
                                },
                                "required": ["text"]
                            }
                        },
                        {
                            "name": "clicky_bubble",
                            "description": "Show a speech bubble on the desktop overlay near the user's cursor.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "text": {
                                        "type": "string",
                                        "description": "Text to display in speech bubble"
                                    }
                                },
                                "required": ["text"]
                            }
                        }
                    ]
                })),
                error: None,
            },
            "tools/call" => {
                let params = request.params.unwrap_or(Value::Null);
                let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

                let result = handle_tool_call(tool_name, arguments).await;
                match result {
                    Ok(val) => JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        id,
                        result: Some(val),
                        error: None,
                    },
                    Err(err_msg) => JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        id,
                        result: None,
                        error: Some(json!({
                            "code": -32000,
                            "message": err_msg
                        })),
                    },
                }
            }
            unknown => JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                id,
                result: None,
                error: Some(json!({
                    "code": -32601,
                    "message": format!("Method not found: {}", unknown)
                })),
            },
        };

        let mut out = serde_json::to_string(&response)?;
        out.push('\n');
        stdout.write_all(out.as_bytes()).await?;
        stdout.flush().await?;
        line.clear();
    }

    Ok(())
}

async fn handle_tool_call(name: &str, args: Value) -> Result<Value, String> {
    match name {
        "clicky_status" => match ipc::send_command(&IpcRequest::GetStatus).await {
            Ok(IpcResponse::Status {
                running,
                voice_state,
                version,
            }) => Ok(json!({
                "content": [{
                    "type": "text",
                    "text": format!("Clicky v{} is running (voice state: {})", version, voice_state)
                }]
            })),
            Ok(other) => Ok(json!({
                "content": [{
                    "type": "text",
                    "text": format!("Clicky response: {:?}", other)
                }]
            })),
            Err(e) => Ok(json!({
                "content": [{
                    "type": "text",
                    "text": format!("Clicky desktop is not running: {}", e)
                }]
            })),
        },
        "clicky_screenshot" => {
            let save_path = args.get("save_path").and_then(|v| v.as_str());

            // Try IPC screenshot first
            match ipc::send_command(&IpcRequest::Screenshot).await {
                Ok(IpcResponse::Screenshot {
                    display_count,
                    base64_jpeg,
                    label,
                }) => {
                    if let Some(path) = save_path {
                        if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&base64_jpeg) {
                            let _ = std::fs::write(path, bytes);
                        }
                    }
                    Ok(json!({
                        "content": [
                            {
                                "type": "text",
                                "text": format!("Captured {} display(s). {}", display_count, label)
                            },
                            {
                                "type": "image",
                                "data": base64_jpeg,
                                "mimeType": "image/jpeg"
                            }
                        ]
                    }))
                }
                _ => {
                    // Fallback to direct screenshot capture even if Clicky daemon is not running
                    let platform = platform::detect();
                    match screenshot::capture_all_screens(&platform) {
                        Ok(capture) => {
                            if let Some(first_shot) = capture.screenshots.first() {
                                if let Some(path) = save_path {
                                    let _ = std::fs::write(path, &first_shot.jpeg_data);
                                }
                                let b64 = base64::engine::general_purpose::STANDARD.encode(&first_shot.jpeg_data);
                                Ok(json!({
                                    "content": [
                                        {
                                            "type": "text",
                                            "text": format!("Captured {} display(s) directly via {}.", capture.screenshots.len(), first_shot.label)
                                        },
                                        {
                                            "type": "image",
                                            "data": b64,
                                            "mimeType": "image/jpeg"
                                        }
                                    ]
                                }))
                            } else {
                                Err("No monitors detected to capture".to_string())
                            }
                        }
                        Err(e) => Err(format!("Failed to capture screenshot: {}", e)),
                    }
                }
            }
        }
        "clicky_point" => {
            let x = args.get("x").and_then(|v| v.as_f64()).ok_or("Missing parameter 'x'")?;
            let y = args.get("y").and_then(|v| v.as_f64()).ok_or("Missing parameter 'y'")?;
            let label = args.get("label").and_then(|v| v.as_str()).map(|s| s.to_string());

            match ipc::send_command(&IpcRequest::Point { x, y, label: label.clone() }).await {
                Ok(IpcResponse::Success(msg)) => Ok(json!({
                    "content": [{
                        "type": "text",
                        "text": msg
                    }]
                })),
                Ok(IpcResponse::Error(err)) => Err(err),
                Err(e) => Err(format!("Clicky is not running (point requires running Clicky app): {}", e)),
                _ => Err("Unexpected response from Clicky".to_string()),
            }
        }
        "clicky_speak" => {
            let text = args.get("text").and_then(|v| v.as_str()).ok_or("Missing parameter 'text'")?;

            match ipc::send_command(&IpcRequest::Speak { text: text.to_string() }).await {
                Ok(IpcResponse::Success(msg)) => Ok(json!({
                    "content": [{
                        "type": "text",
                        "text": msg
                    }]
                })),
                Ok(IpcResponse::Error(err)) => Err(err),
                Err(e) => Err(format!("Clicky is not running (speak requires running Clicky app): {}", e)),
                _ => Err("Unexpected response from Clicky".to_string()),
            }
        }
        "clicky_bubble" => {
            let text = args.get("text").and_then(|v| v.as_str()).ok_or("Missing parameter 'text'")?;

            match ipc::send_command(&IpcRequest::ShowBubble { text: text.to_string() }).await {
                Ok(IpcResponse::Success(msg)) => Ok(json!({
                    "content": [{
                        "type": "text",
                        "text": msg
                    }]
                })),
                Ok(IpcResponse::Error(err)) => Err(err),
                Err(e) => Err(format!("Clicky is not running (bubble requires running Clicky app): {}", e)),
                _ => Err("Unexpected response from Clicky".to_string()),
            }
        }
        unknown => Err(format!("Unknown tool: {}", unknown)),
    }
}
