use crate::api::claude::ScreenshotForClaude;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use log::{debug, info, warn};
use reqwest::Client;
use serde_json::json;

const RESPONSES_API_URL: &str = "https://api.openai.com/v1/responses";
const CHAT_COMPLETIONS_FALLBACK_URL: &str = "https://api.openai.com/v1/chat/completions";

/// Sends a multimodal vision request to OpenAI's Responses API using
/// an OAuth access token (obtained via Sign in with ChatGPT).
pub async fn send_chatgpt_vision_request(
    http_client: &Client,
    access_token: &str,
    model: &str,
    system_prompt: &str,
    history_exchanges: &[(String, String)],
    transcript: &str,
    screenshots: &[ScreenshotForClaude],
) -> Result<String, String> {
    info!("Sending vision request via ChatGPT Responses API (model: {})...", model);

    // 1. Try modern /v1/responses API format first
    let mut input_items: Vec<serde_json::Value> = Vec::new();

    input_items.push(json!({
        "role": "system",
        "content": system_prompt
    }));

    for (user_text, assistant_text) in history_exchanges {
        input_items.push(json!({
            "role": "user",
            "content": user_text
        }));
        input_items.push(json!({
            "role": "assistant",
            "content": assistant_text
        }));
    }

    let mut current_user_content: Vec<serde_json::Value> = Vec::new();
    current_user_content.push(json!({
        "type": "input_text",
        "text": transcript
    }));

    for screenshot in screenshots {
        let b64 = BASE64_STANDARD.encode(&screenshot.jpeg_data);
        let image_url = format!("data:image/jpeg;base64,{}", b64);
        current_user_content.push(json!({
            "type": "input_image",
            "image_url": image_url
        }));
    }

    input_items.push(json!({
        "role": "user",
        "content": current_user_content
    }));

    let request_body = json!({
        "model": model,
        "input": input_items
    });

    let response = http_client
        .post(RESPONSES_API_URL)
        .header("Content-Type", "application/json")
        .bearer_auth(access_token)
        .json(&request_body)
        .send()
        .await;

    match response {
        Ok(res) if res.status().is_success() => {
            let body_text = res.text().await.map_err(|e| format!("Failed to read body: {e}"))?;
            debug!("Responses API success: {body_text}");
            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&body_text) {
                if let Some(text) = extract_text_from_responses_api(&parsed) {
                    return Ok(text);
                }
            }
        }
        Ok(res) => {
            let status = res.status();
            let body_text = res.text().await.unwrap_or_default();
            warn!("Responses API returned status {status}: {body_text}. Falling back to chat/completions endpoint...");
        }
        Err(err) => {
            warn!("Responses API request failed: {err}. Falling back to chat/completions endpoint...");
        }
    }

    // 2. Fallback to /v1/chat/completions endpoint using the OAuth access token
    crate::api::openai_compat::send_openai_vision_request(
        http_client,
        CHAT_COMPLETIONS_FALLBACK_URL,
        Some(access_token),
        model,
        system_prompt,
        history_exchanges,
        transcript,
        screenshots,
    )
    .await
}

fn extract_text_from_responses_api(json: &serde_json::Value) -> Option<String> {
    // 1. Direct output_text field
    if let Some(text) = json.get("output_text").and_then(|t| t.as_str()) {
        return Some(text.trim().to_string());
    }

    // 2. output array: output[0].content[0].text
    if let Some(outputs) = json.get("output").and_then(|o| o.as_array()) {
        for output in outputs {
            if let Some(content_items) = output.get("content").and_then(|c| c.as_array()) {
                for item in content_items {
                    if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                        return Some(text.trim().to_string());
                    }
                }
            }
        }
    }

    // 3. choices[0].message.content (standard completions compatibility)
    if let Some(content) = json.pointer("/choices/0/message/content").and_then(|c| c.as_str()) {
        return Some(content.trim().to_string());
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_extract_text_from_output_text() {
        let data = json!({
            "output_text": "hello from responses api [POINT:100,200:button]"
        });
        let result = extract_text_from_responses_api(&data).unwrap();
        assert_eq!(result, "hello from responses api [POINT:100,200:button]");
    }

    #[test]
    fn test_extract_text_from_output_array() {
        let data = json!({
            "output": [
                {
                    "content": [
                        { "type": "text", "text": "click here [POINT:300,400:menu]" }
                    ]
                }
            ]
        });
        let result = extract_text_from_responses_api(&data).unwrap();
        assert_eq!(result, "click here [POINT:300,400:menu]");
    }
}
