use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use log::{error, info, warn};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

// Default client ID for Sign in with ChatGPT: dynamic registration for desktop / open-source clients
pub const DEFAULT_CHATGPT_CLIENT_ID: &str = "dynamic_agent_client";
const AUTH_URL: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN_URL: &str = "https://auth.openai.com/api/accounts/oauth/token";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatGptSession {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<u64>,
    pub email: Option<String>,
    #[serde(default)]
    pub client_id: Option<String>,
}

impl ChatGptSession {
    pub fn session_file_path() -> Option<PathBuf> {
        let config_dir = crate::config::config_file_path()?.parent()?.to_path_buf();
        Some(config_dir.join("chatgpt_session.json"))
    }

    pub fn load() -> Option<Self> {
        if let Some(path) = Self::session_file_path() {
            if let Ok(raw) = fs::read_to_string(&path) {
                if let Ok(session) = serde_json::from_str::<Self>(&raw) {
                    return Some(session);
                }
            }
        }
        // Auto-fallback: if no Clicky session file exists, check for existing Codex session
        Self::import_codex_session().ok()
    }

    pub fn has_codex_session() -> bool {
        if let Ok(home) = std::env::var("HOME") {
            let path = PathBuf::from(home).join(".codex/auth.json");
            return path.exists();
        }
        false
    }

    /// Imports an active ChatGPT session from ~/.codex/auth.json
    pub fn import_codex_session() -> Result<Self, String> {
        let home = std::env::var("HOME")
            .map_err(|e| format!("Cannot locate HOME directory: {e}"))?;
        let codex_auth = PathBuf::from(home).join(".codex/auth.json");
        if !codex_auth.exists() {
            return Err("No se encontró el archivo de autenticación de Codex en ~/.codex/auth.json".to_string());
        }

        let raw = fs::read_to_string(&codex_auth)
            .map_err(|e| format!("Error al leer ~/.codex/auth.json: {e}"))?;

        let parsed: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| format!("Formato inválido en ~/.codex/auth.json: {e}"))?;

        let tokens = parsed
            .get("tokens")
            .ok_or("No se encontró la sección 'tokens' en ~/.codex/auth.json")?;

        let access_token = tokens
            .get("access_token")
            .and_then(|v| v.as_str())
            .ok_or("No se encontró 'access_token' en ~/.codex/auth.json")?
            .to_string();

        let refresh_token = tokens
            .get("refresh_token")
            .and_then(|v| v.as_str())
            .map(String::from);

        let email = parsed
            .get("email")
            .or_else(|| tokens.get("account_id"))
            .and_then(|v| v.as_str())
            .map(String::from);

        let session = ChatGptSession {
            access_token,
            refresh_token,
            expires_at: None,
            email,
            client_id: Some("app_EMoamEEZ73f0CkXaXp7hrann".to_string()),
        };

        session.save()?;
        info!("Imported active ChatGPT session from ~/.codex/auth.json");
        Ok(session)
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::session_file_path().ok_or("Cannot determine session file path")?;
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let serialized = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize session: {e}"))?;
        fs::write(path, serialized).map_err(|e| format!("Failed to save session: {e}"))
    }

    pub fn clear() -> Result<(), String> {
        if let Some(path) = Self::session_file_path() {
            if path.exists() {
                let _ = fs::remove_file(path);
            }
        }
        Ok(())
    }

    pub fn is_expired(&self) -> bool {
        if let Some(expires_at) = self.expires_at {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            // Expired if current time + 60s buffer >= expiration
            now + 60 >= expires_at
        } else {
            false
        }
    }

    pub async fn get_valid_token(
        &mut self,
        http_client: &Client,
        configured_client_id: &str,
    ) -> Result<String, String> {
        if !self.is_expired() {
            return Ok(self.access_token.clone());
        }

        let effective_client_id = self.client_id.as_deref().unwrap_or(configured_client_id);
        if let Some(refresh_token) = &self.refresh_token {
            info!("ChatGPT session token expired, attempting automatic refresh...");
            match refresh_access_token(http_client, effective_client_id, refresh_token).await {
                Ok(new_session) => {
                    *self = new_session;
                    let _ = self.save();
                    return Ok(self.access_token.clone());
                }
                Err(err) => {
                    warn!("Token refresh failed: {err}. Re-login might be required.");
                }
            }
        }

        Ok(self.access_token.clone())
    }
}

/// Generates a valid RFC 4122 compliant UUIDv4 string formatted as an URN (urn:uuid:xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx).
pub fn generate_uuid_v4() -> String {
    let mut bytes = [0u8; 16];
    let rng = ring::rand::SystemRandom::new();
    use ring::rand::SecureRandom;
    if rng.fill(&mut bytes).is_err() {
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let pid = std::process::id();
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = ((seed.wrapping_add(i as u32 * 37) >> (i % 8)) ^ (pid >> (i % 16))) as u8;
        }
    }

    // Version 4: set bits 4-7 of byte 6 to 0100 (0x40)
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    // Variant RFC 4122: set bits 6-7 of byte 8 to 10 (0x80..0xbf)
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    format!(
        "urn:uuid:{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

/// Retrieves or persists a stable opaque host identifier (RFC 4122 UUIDv4 URN) for dynamic client registration.
pub fn get_or_create_host_id() -> String {
    if let Some(config_dir) = crate::config::config_file_path().and_then(|p| p.parent().map(|d| d.to_path_buf())) {
        let _ = fs::create_dir_all(&config_dir);
        let host_id_file = config_dir.join("host_id");
        if let Ok(id) = fs::read_to_string(&host_id_file) {
            let trimmed = id.trim();
            // Validate that it's a valid urn:uuid of exactly 45 characters with version 4
            if trimmed.starts_with("urn:uuid:") && trimmed.len() == 45 && trimmed.chars().nth(23) == Some('4') {
                return trimmed.to_string();
            }
        }
        let uuid_urn = generate_uuid_v4();
        let _ = fs::write(&host_id_file, &uuid_urn);
        return uuid_urn;
    }
    generate_uuid_v4()
}

/// Opens a URL in the user's default browser, ensuring Linux / NixOS dynamic library
/// environment variables (LD_LIBRARY_PATH) do not corrupt the spawned browser process.
pub fn open_browser(url: &str) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        let launchers: &[(&str, &[&str])] = &[
            ("xdg-open", &[url]),
            ("gio", &["open", url]),
            ("firefox", &[url]),
            ("google-chrome-stable", &[url]),
            ("chromium", &[url]),
            ("brave", &[url]),
        ];

        for (bin, args) in launchers {
            if let Ok(child) = std::process::Command::new(bin)
                .args(*args)
                .env_remove("LD_LIBRARY_PATH")
                .env_remove("LD_PRELOAD")
                .spawn()
            {
                info!("Launched browser via {} (pid: {})", bin, child.id());
                return Ok(());
            }
        }
    }

    webbrowser::open(url).map_err(|e| format!("Failed to open browser: {e}"))
}

/// Initiates the OAuth PKCE flow by opening the user's browser and listening
/// on a local HTTP port for the authorization redirect callback.
pub async fn login_with_chatgpt(
    http_client: &Client,
    client_id: &str,
    on_auth_url: Option<std::sync::Arc<std::sync::Mutex<Option<String>>>>,
) -> Result<ChatGptSession, String> {
    // 1. Generate PKCE verifier and challenge
    let code_verifier = generate_random_string(64);
    let code_challenge = generate_code_challenge(&code_verifier);
    let state = generate_random_string(32);

    // 2. Bind local HTTP server on an available port
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("Failed to bind local callback server: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("Failed to read local port: {e}"))?
        .port();

    let redirect_uri = format!("http://127.0.0.1:{}/callback", port);

    let host_id = get_or_create_host_id();

    // 3. Construct OAuth URL
    let mut auth_url = format!(
        "{}?response_type=code&client_id={}&redirect_uri={}&scope=openid%20profile%20email%20offline_access&state={}&code_challenge={}&code_challenge_method=S256",
        AUTH_URL,
        urlencoding(client_id),
        urlencoding(&redirect_uri),
        urlencoding(&state),
        urlencoding(&code_challenge)
    );

    if client_id == DEFAULT_CHATGPT_CLIENT_ID {
        auth_url.push_str(&format!(
            "&ext_agent_host_id={}&agent_name_hint={}",
            urlencoding(&host_id),
            urlencoding("Clicky Desktop")
        ));
    }

    info!("Opening browser for Sign in with ChatGPT: {}", auth_url);
    if let Some(ref slot) = on_auth_url {
        if let Ok(mut lock) = slot.lock() {
            *lock = Some(auth_url.clone());
        }
    }

    if let Err(e) = open_browser(&auth_url) {
        warn!("Failed to open browser automatically: {e}. Open URL manually: {auth_url}");
    }

    // 4. Wait for redirect callback with timeout (2 minutes)
    let (code, effective_client_id) = tokio::time::timeout(tokio::time::Duration::from_secs(120), async {
        let (mut socket, _) = listener
            .accept()
            .await
            .map_err(|e| format!("Connection error: {e}"))?;

        let mut buffer = [0u8; 4096];
        let bytes_read = socket
            .read(&mut buffer)
            .await
            .map_err(|e| format!("Read error: {e}"))?;
        let request_str = String::from_utf8_lossy(&buffer[..bytes_read]);

        // Parse query params from GET /callback?code=...&state=...
        let (extracted_code, extracted_state, returned_client_id) = parse_oauth_callback(&request_str)?;

        if extracted_state != state {
            let error_response = "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<h3>Error: CSRF state mismatch</h3>";
            let _ = socket.write_all(error_response.as_bytes()).await;
            return Err("State mismatch in OAuth callback".to_string());
        }

        let success_html = r#"<!DOCTYPE html>
<html>
<head><title>Clicky Desktop - ChatGPT Login</title></head>
<body style="font-family: system-ui, sans-serif; background: #121214; color: #f0f0f0; text-align: center; padding: 60px;">
  <h1 style="color: #60a5fa;">✓ Autenticación Exitosa</h1>
  <p>Tu cuenta de ChatGPT ha sido conectada con Clicky Desktop.</p>
  <p style="color: #888;">Ya puedes cerrar esta pestaña y regresar a la aplicación.</p>
</body>
</html>"#;

        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            success_html.len(),
            success_html
        );
        let _ = socket.write_all(response.as_bytes()).await;
        let _ = socket.flush().await;

        let final_client_id = returned_client_id.unwrap_or_else(|| client_id.to_string());
        Ok((extracted_code, final_client_id))
    })
    .await
    .map_err(|_| "OAuth login timed out (waited 2 minutes)".to_string())??;

    // 5. Exchange authorization code for tokens
    info!("Authorization code received, exchanging for access token with client_id: {}...", effective_client_id);
    exchange_code_for_token(http_client, &effective_client_id, &code, &code_verifier, &redirect_uri).await
}

async fn exchange_code_for_token(
    http_client: &Client,
    client_id: &str,
    code: &str,
    code_verifier: &str,
    redirect_uri: &str,
) -> Result<ChatGptSession, String> {
    let params = [
        ("grant_type", "authorization_code"),
        ("client_id", client_id),
        ("code", code),
        ("code_verifier", code_verifier),
        ("redirect_uri", redirect_uri),
    ];

    let response = http_client
        .post(TOKEN_URL)
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("Failed to send token request: {e}"))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("Failed to read token response: {e}"))?;

    if !status.is_success() {
        error!("OAuth token exchange error (status {status}): {body}");
        return Err(format!("OAuth token error ({status}): {body}"));
    }

    let parsed: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("Invalid token JSON: {e}"))?;

    let access_token = parsed
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or("Missing access_token in OAuth response")?
        .to_string();

    let refresh_token = parsed
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(String::from);

    let expires_in = parsed.get("expires_in").and_then(|v| v.as_u64());
    let expires_at = expires_in.map(|secs| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + secs
    });

    let session = ChatGptSession {
        access_token,
        refresh_token,
        expires_at,
        email: None,
        client_id: Some(client_id.to_string()),
    };

    let _ = session.save();
    info!("Successfully authenticated with ChatGPT!");
    Ok(session)
}

async fn refresh_access_token(
    http_client: &Client,
    client_id: &str,
    refresh_token: &str,
) -> Result<ChatGptSession, String> {
    let params = [
        ("grant_type", "refresh_token"),
        ("client_id", client_id),
        ("refresh_token", refresh_token),
    ];

    let response = http_client
        .post(TOKEN_URL)
        .form(&params)
        .send()
        .await
        .map_err(|e| format!("Refresh request failed: {e}"))?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();

    if !status.is_success() {
        return Err(format!("Refresh error ({status}): {body}"));
    }

    let parsed: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("Invalid JSON: {e}"))?;

    let access_token = parsed
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or("Missing access_token")?
        .to_string();

    let new_refresh_token = parsed
        .get("refresh_token")
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| Some(refresh_token.to_string()));

    let expires_in = parsed.get("expires_in").and_then(|v| v.as_u64());
    let expires_at = expires_in.map(|secs| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + secs
    });

    Ok(ChatGptSession {
        access_token,
        refresh_token: new_refresh_token,
        expires_at,
        email: None,
        client_id: Some(client_id.to_string()),
    })
}

fn parse_oauth_callback(request: &str) -> Result<(String, String, Option<String>), String> {
    let first_line = request.lines().next().ok_or("Empty HTTP request")?;
    let path_and_query = first_line
        .split_whitespace()
        .nth(1)
        .ok_or("Malformed HTTP line")?;

    let query = path_and_query.split('?').nth(1).ok_or("Missing query string")?;

    let mut code = None;
    let mut state = None;
    let mut client_id = None;
    let mut error = None;
    let mut error_description = None;

    for param in query.split('&') {
        let mut parts = param.split('=');
        if let (Some(k), Some(v)) = (parts.next(), parts.next()) {
            match k {
                "code" => code = Some(v.to_string()),
                "state" => state = Some(v.to_string()),
                "client_id" => client_id = Some(v.to_string()),
                "error" => error = Some(v.to_string()),
                "error_description" => error_description = Some(v.to_string()),
                _ => {}
            }
        }
    }

    if let Some(err) = error {
        let desc = error_description.unwrap_or_default();
        return Err(format!("OpenAI OAuth error: {} ({})", err, desc));
    }

    let code = code.ok_or("Missing 'code' parameter in callback")?;
    let state = state.ok_or("Missing 'state' parameter in callback")?;

    Ok((code, state, client_id))
}

fn generate_random_string(length: usize) -> String {
    use std::time::SystemTime;
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let mut bytes = vec![0u8; length];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = ((seed.wrapping_add(i as u32 * 37) >> (i % 8)) & 0xFF) as u8;
    }
    // Mix with PID
    let pid = std::process::id();
    for (i, b) in bytes.iter_mut().enumerate() {
        *b ^= ((pid >> (i % 16)) & 0xFF) as u8;
    }
    URL_SAFE_NO_PAD.encode(bytes)
}

fn generate_code_challenge(verifier: &str) -> String {
    let hash = ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(hash.as_ref())
}

fn urlencoding(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(b as char);
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", b));
            }
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_random_string() {
        let s1 = generate_random_string(32);
        let s2 = generate_random_string(32);
        assert!(!s1.is_empty());
        assert!(!s2.is_empty());
    }

    #[test]
    fn test_code_challenge_is_base64url() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = generate_code_challenge(verifier);
        assert!(!challenge.is_empty());
        assert!(!challenge.contains('+'));
        assert!(!challenge.contains('/'));
        assert!(!challenge.contains('='));
    }

    #[test]
    fn test_parse_oauth_callback_success() {
        let request = "GET /callback?code=sample_code_123&state=xyzState456 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        let (code, state, client_id) = parse_oauth_callback(request).expect("Should parse query");
        assert_eq!(code, "sample_code_123");
        assert_eq!(state, "xyzState456");
        assert_eq!(client_id, None);
    }

    #[test]
    fn test_urlencoding() {
        assert_eq!(urlencoding("hello world"), "hello%20world");
        assert_eq!(urlencoding("a-b_c.d~e"), "a-b_c.d~e");
    }

    #[test]
    fn test_generate_uuid_v4_format() {
        let urn = generate_uuid_v4();
        assert!(urn.starts_with("urn:uuid:"));
        assert_eq!(urn.len(), 45);
        let uuid = &urn["urn:uuid:".len()..];
        // Must match 8-4-4-4-12 format
        assert_eq!(uuid.chars().nth(8), Some('-'));
        assert_eq!(uuid.chars().nth(13), Some('-'));
        assert_eq!(uuid.chars().nth(18), Some('-'));
        assert_eq!(uuid.chars().nth(23), Some('-'));
        // Version 4 check
        assert_eq!(uuid.chars().nth(14), Some('4'));
        // Variant check (8, 9, a, or b)
        let variant = uuid.chars().nth(19).unwrap();
        assert!(matches!(variant, '8' | '9' | 'a' | 'b'));
    }
}
