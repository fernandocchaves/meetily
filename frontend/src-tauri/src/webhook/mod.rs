//! Meeting transcript webhook
//!
//! Sends the full transcript of a finished meeting to a user-configured HTTP
//! endpoint (e.g. Hermes) right after it's saved locally. Summarization is
//! left to whatever consumes the webhook — this only ships the raw
//! transcript, never a summary.

use hmac::{Hmac, Mac};
use log::{error, info};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::path::PathBuf;
use tauri::command;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookSettings {
    pub enabled: bool,
    pub url: String,
    pub secret: Option<String>,
}

impl Default for WebhookSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            url: String::new(),
            secret: None,
        }
    }
}

impl WebhookSettings {
    fn settings_path() -> Option<PathBuf> {
        dirs::data_dir().map(|p| p.join("com.meetily.ai").join("webhook_settings.json"))
    }

    pub fn load() -> Self {
        if let Some(path) = Self::settings_path() {
            if path.exists() {
                match std::fs::read_to_string(&path) {
                    Ok(contents) => match serde_json::from_str(&contents) {
                        Ok(settings) => return settings,
                        Err(e) => error!("Failed to parse webhook settings: {}", e),
                    },
                    Err(e) => error!("Failed to read webhook settings: {}", e),
                }
            }
        }
        Self::default()
    }

    pub fn save(&self) -> Result<(), String> {
        if let Some(path) = Self::settings_path() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create settings directory: {}", e))?;
            }

            let contents = serde_json::to_string_pretty(self)
                .map_err(|e| format!("Failed to serialize webhook settings: {}", e))?;

            std::fs::write(&path, contents)
                .map_err(|e| format!("Failed to write webhook settings: {}", e))?;

            info!("Saved webhook settings to {:?}", path);
            Ok(())
        } else {
            Err("Could not determine settings path".to_string())
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptSegmentPayload {
    pub text: String,
    pub timestamp: String,
    pub audio_start_time: Option<f64>,
    pub audio_end_time: Option<f64>,
    pub duration: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeetingWebhookPayload {
    pub meeting_id: String,
    pub title: String,
    pub started_at: Option<String>,
    pub transcript: Vec<TranscriptSegmentPayload>,
    /// Flattened "[timestamp] text" lines, one per segment. Hermes's webhook
    /// prompt templates JSON-serialize-and-truncate nested lists/dicts at
    /// 2000 chars, but substitute flat string fields directly — so this is
    /// what the Hermes-side prompt template should reference, not
    /// `transcript` directly. Derived server-side from `transcript`, so
    /// callers don't need to send it (defaults to empty and gets overwritten).
    #[serde(default)]
    pub transcript_text: String,
}

fn flatten_transcript(segments: &[TranscriptSegmentPayload]) -> String {
    segments
        .iter()
        .map(|s| format!("[{}] {}", s.timestamp, s.text))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Signs `body` per Hermes's webhook "Generic V2" scheme: hex HMAC-SHA256 of
/// `"{unix_timestamp}.{body}"`. Returns (signature, timestamp) — both must be
/// sent as headers, and `body` must be the exact bytes POSTed (no
/// re-serializing after signing).
fn sign_v2(secret: &str, body: &str) -> (String, String) {
    let timestamp = chrono::Utc::now().timestamp().to_string();
    let mut mac =
        HmacSha256::new_from_slice(secret.as_bytes()).expect("HMAC accepts a key of any size");
    mac.update(format!("{}.{}", timestamp, body).as_bytes());
    let signature = mac
        .finalize()
        .into_bytes()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();
    (signature, timestamp)
}

fn post_webhook(settings: &WebhookSettings, payload: &serde_json::Value) -> Result<(), String> {
    if !settings.enabled {
        return Err("Webhook is disabled".to_string());
    }
    if settings.url.is_empty() {
        return Err("Webhook URL is not configured".to_string());
    }

    let body = serde_json::to_string(payload)
        .map_err(|e| format!("Failed to serialize webhook payload: {}", e))?;

    let client = Client::new();
    let mut req = client
        .post(&settings.url)
        .header("Content-Type", "application/json");

    if let Some(secret) = &settings.secret {
        if !secret.is_empty() {
            let (signature, timestamp) = sign_v2(secret, &body);
            req = req
                .header("X-Webhook-Signature-V2", signature)
                .header("X-Webhook-Timestamp", timestamp);
        }
    }

    let response = req
        .body(body)
        .send()
        .map_err(|e| format!("Failed to send webhook: {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "Webhook endpoint returned status: {}",
            response.status()
        ));
    }

    info!("Webhook delivered to {}", settings.url);
    Ok(())
}

#[command]
pub fn get_webhook_settings() -> Result<WebhookSettings, String> {
    Ok(WebhookSettings::load())
}

#[command]
pub fn set_webhook_settings(settings: WebhookSettings) -> Result<(), String> {
    settings.save()
}

/// Sends the full transcript of a finished meeting to the configured webhook.
/// No-op (returns Err, not a panic) if the webhook isn't enabled/configured —
/// callers should treat failures here as non-fatal to the save/navigate flow.
#[command]
pub fn send_meeting_webhook(mut payload: MeetingWebhookPayload) -> Result<(), String> {
    payload.transcript_text = flatten_transcript(&payload.transcript);
    let settings = WebhookSettings::load();
    let body = serde_json::to_value(&payload)
        .map_err(|e| format!("Failed to serialize webhook payload: {}", e))?;
    post_webhook(&settings, &body)
}

/// Sends a small fixture payload to the configured webhook URL, for the
/// settings UI's "Send test webhook" button.
#[command]
pub fn test_webhook() -> Result<(), String> {
    let settings = WebhookSettings::load();
    let now = chrono::Local::now();
    let timestamp = now.format("%H:%M:%S").to_string();
    let segments = vec![TranscriptSegmentPayload {
        text: "This is a test webhook from Meetily.".to_string(),
        timestamp: timestamp.clone(),
        audio_start_time: Some(0.0),
        audio_end_time: Some(2.0),
        duration: Some(2.0),
    }];
    let payload = MeetingWebhookPayload {
        meeting_id: "test".to_string(),
        title: "Test Meeting".to_string(),
        started_at: Some(now.to_rfc3339()),
        transcript_text: flatten_transcript(&segments),
        transcript: segments,
    };
    let body = serde_json::to_value(&payload)
        .map_err(|e| format!("Failed to serialize test webhook payload: {}", e))?;
    post_webhook(&settings, &body)
}
