use crate::app::platform::PlatformInfo;
use crate::audio::UiEvent;
use crate::runtime::{self, TtsConfig};
use crate::screenshot;
use base64::Engine;
use log::{error, info, warn};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};

/// Returns the path to the Clicky IPC Unix domain socket.
pub fn get_socket_path() -> PathBuf {
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        if !runtime_dir.is_empty() {
            return PathBuf::from(runtime_dir).join("clicky-desktop.sock");
        }
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "default".to_string());
    PathBuf::from(format!("/tmp/clicky-desktop-{}.sock", user))
}

/// Commands accepted by the Clicky IPC socket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", content = "params")]
pub enum IpcRequest {
    Ping,
    GetStatus,
    OpenSettings,
    Kill,
    Point {
        x: f64,
        y: f64,
        label: Option<String>,
    },
    Speak {
        text: String,
    },
    ShowBubble {
        text: String,
    },
    Screenshot,
}

/// Responses sent over the Clicky IPC socket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", content = "data")]
pub enum IpcResponse {
    Pong,
    Status {
        running: bool,
        voice_state: String,
        version: String,
    },
    Success(String),
    Error(String),
    Screenshot {
        display_count: usize,
        base64_jpeg: String,
        label: String,
    },
}

/// Shared state exposed by the IPC server.
#[derive(Clone)]
pub struct IpcSharedState {
    pub voice_state: Arc<Mutex<String>>,
    pub platform: PlatformInfo,
    pub http_client: reqwest::Client,
    pub tts_config: Arc<Mutex<Option<TtsConfig>>>,
}

/// Starts the Unix domain socket server in a Tokio background task.
pub fn start_ipc_server(
    rt_handle: &tokio::runtime::Handle,
    ui_tx: std_mpsc::Sender<UiEvent>,
    shared_state: IpcSharedState,
) -> tokio::task::JoinHandle<()> {
    rt_handle.spawn(async move {
        let socket_path = get_socket_path();

        // Clean up any stale socket file
        if socket_path.exists() {
            let _ = std::fs::remove_file(&socket_path);
        }

        let listener = match UnixListener::bind(&socket_path) {
            Ok(l) => {
                info!("Clicky IPC server listening at: {}", socket_path.display());
                l
            }
            Err(e) => {
                error!(
                    "Failed to bind Clicky IPC socket at {}: {}",
                    socket_path.display(),
                    e
                );
                return;
            }
        };

        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    let ui_tx = ui_tx.clone();
                    let state = shared_state.clone();
                    tokio::spawn(async move {
                        if let Err(err) = handle_client(stream, ui_tx, state).await {
                            warn!("IPC client connection ended with error: {}", err);
                        }
                    });
                }
                Err(err) => {
                    error!("Error accepting IPC connection: {}", err);
                    break;
                }
            }
        }

        // Clean up on exit
        let _ = std::fs::remove_file(&socket_path);
    })
}

async fn handle_client(
    mut stream: UnixStream,
    ui_tx: std_mpsc::Sender<UiEvent>,
    state: IpcSharedState,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (reader, mut writer) = stream.split();
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();

    while buf_reader.read_line(&mut line).await? > 0 {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            line.clear();
            continue;
        }

        let request: IpcRequest = match serde_json::from_str(trimmed) {
            Ok(req) => req,
            Err(e) => {
                let err_resp = IpcResponse::Error(format!("Invalid request JSON: {}", e));
                let mut out = serde_json::to_string(&err_resp)?;
                out.push('\n');
                writer.write_all(out.as_bytes()).await?;
                line.clear();
                continue;
            }
        };

        let response = match request {
            IpcRequest::Ping => IpcResponse::Pong,
            IpcRequest::GetStatus => {
                let current_state = state
                    .voice_state
                    .lock()
                    .map(|s| s.clone())
                    .unwrap_or_else(|_| "Unknown".to_string());
                IpcResponse::Status {
                    running: true,
                    voice_state: current_state,
                    version: env!("CARGO_PKG_VERSION").to_string(),
                }
            }
            IpcRequest::OpenSettings => {
                let _ = ui_tx.send(UiEvent::OpenSettings);
                IpcResponse::Success("Opened Settings window".to_string())
            }
            IpcRequest::Kill => {
                let _ = ui_tx.send(UiEvent::Quit);
                IpcResponse::Success("Kill signal sent to Clicky".to_string())
            }
            IpcRequest::Point { x, y, label } => {
                let _ = ui_tx.send(UiEvent::ExternalPoint { x, y, label: label.clone() });
                IpcResponse::Success(format!(
                    "Pointing to ({:.0}, {:.0}) with label: {:?}",
                    x, y, label
                ))
            }
            IpcRequest::ShowBubble { text } => {
                let _ = ui_tx.send(UiEvent::ExternalBubble { text: text.clone() });
                IpcResponse::Success(format!("Bubble shown: {}", text))
            }
            IpcRequest::Speak { text } => {
                let tts_conf = {
                    let lock = state.tts_config.lock();
                    lock.ok().and_then(|c| c.clone())
                };

                if let Some(conf) = tts_conf {
                    let client = state.http_client.clone();
                    let tts_tx = ui_tx.clone();
                    let speak_text = text.clone();
                    tokio::spawn(async move {
                        match runtime::synthesize_speech(&client, &conf, &speak_text).await {
                            Ok(bytes) => {
                                let _ = tts_tx.send(UiEvent::TtsAudio(bytes));
                            }
                            Err(e) => {
                                let _ = tts_tx.send(UiEvent::TtsError(e));
                            }
                        }
                    });
                    IpcResponse::Success(format!("Speaking text: {}", text))
                } else {
                    IpcResponse::Error("No TTS provider configured".to_string())
                }
            }
            IpcRequest::Screenshot => {
                let plat = state.platform.clone();
                match tokio::task::spawn_blocking(move || screenshot::capture_all_screens(0.0, 0.0, &plat)).await
                {
                    Ok(Ok(capture)) => {
                        if let Some(first_shot) = capture.screenshots.first() {
                            let b64 = base64::engine::general_purpose::STANDARD.encode(&first_shot.jpeg_data);
                            IpcResponse::Screenshot {
                                display_count: capture.screenshots.len(),
                                base64_jpeg: b64,
                                label: first_shot.label.clone(),
                            }
                        } else {
                            IpcResponse::Error("No monitors captured".to_string())
                        }
                    }
                    Ok(Err(e)) => IpcResponse::Error(format!("Screenshot capture failed: {}", e)),
                    Err(e) => IpcResponse::Error(format!("Screenshot task panicked: {}", e)),
                }
            }
        };

        let mut out = serde_json::to_string(&response)?;
        out.push('\n');
        writer.write_all(out.as_bytes()).await?;
        line.clear();
    }

    Ok(())
}

/// Client helper: sends an IPC command to the running Clicky instance and awaits response.
pub async fn send_command(req: &IpcRequest) -> Result<IpcResponse, String> {
    let socket_path = get_socket_path();
    if !socket_path.exists() {
        return Err("Clicky is not running (socket file does not exist)".to_string());
    }

    let stream = match UnixStream::connect(&socket_path).await {
        Ok(s) => s,
        Err(e) => {
            return Err(format!(
                "Failed to connect to Clicky socket (is Clicky running?): {}",
                e
            ));
        }
    };

    let (reader, mut writer) = stream.into_split();
    let mut buf_reader = BufReader::new(reader);

    let mut req_str = serde_json::to_string(req).map_err(|e| e.to_string())?;
    req_str.push('\n');

    writer.write_all(req_str.as_bytes()).await.map_err(|e| e.to_string())?;

    let mut resp_str = String::new();
    let bytes_read = buf_reader.read_line(&mut resp_str).await.map_err(|e| e.to_string())?;
    if bytes_read == 0 {
        return Err("Clicky closed connection without response".to_string());
    }

    let response: IpcResponse = serde_json::from_str(resp_str.trim()).map_err(|e| {
        format!("Failed to parse response '{}': {}", resp_str.trim(), e)
    })?;

    Ok(response)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    fn test_ipc_request_serialization() {
        let req = IpcRequest::Point {
            x: 100.5,
            y: 200.0,
            label: Some("test target".to_string()),
        };
        let serialized = serde_json::to_string(&req).unwrap();
        assert!(serialized.contains("Point"));
        assert!(serialized.contains("test target"));

        let deserialized: IpcRequest = serde_json::from_str(&serialized).unwrap();
        match deserialized {
            IpcRequest::Point { x, y, label } => {
                assert_eq!(x, 100.5);
                assert_eq!(y, 200.0);
                assert_eq!(label, Some("test target".to_string()));
            }
            _ => panic!("Expected Point variant"),
        }
    }

    #[test]
    fn test_ipc_response_serialization() {
        let resp = IpcResponse::Status {
            running: true,
            voice_state: "Idle".to_string(),
            version: "0.1.0".to_string(),
        };
        let serialized = serde_json::to_string(&resp).unwrap();
        let deserialized: IpcResponse = serde_json::from_str(&serialized).unwrap();
        match deserialized {
            IpcResponse::Status { running, voice_state, version } => {
                assert!(running);
                assert_eq!(voice_state, "Idle");
                assert_eq!(version, "0.1.0");
            }
            _ => panic!("Expected Status variant"),
        }
    }

    #[test]
    fn test_socket_path_not_empty() {
        let path = get_socket_path();
        assert!(!path.as_os_str().is_empty());
    }
}
