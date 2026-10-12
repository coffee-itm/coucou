// Preferences, stored as plain JSON in settings.json under platform::config_dir().
// No secret ever lands here — API keys live in the OS keychain (see secrets.rs).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub sound_enabled: bool,
    pub sound_volume: f64,
    pub auto_close_interval: f64,
    pub absence_interval: f64,
    pub active_integrations: Vec<String>,
    /// "primary" = the main display, "cursor" = whichever display the mouse is on.
    pub screen: String,
    pub autostart: bool,
    pub hooks_installed: bool,
    /// Chat provider: "anthropic" | "google" | "openai" | "custom"
    #[serde(default = "default_chat_provider")]
    pub chat_provider: String,
    /// Anthropic model used by the chat.
    #[serde(default = "default_model")]
    pub model: String,
    /// Google AI model.
    #[serde(default = "default_google_model")]
    pub google_model: String,
    /// OpenAI model.
    #[serde(default = "default_openai_model")]
    pub openai_model: String,
    /// Custom (OpenAI-compatible) base URL, e.g. "http://localhost:3001/v1"
    #[serde(default)]
    pub custom_base_url: String,
    /// Custom model ID.
    #[serde(default = "default_custom_model")]
    pub custom_model: String,
}

fn default_chat_provider() -> String {
    "anthropic".into()
}

fn default_model() -> String {
    crate::claude::DEFAULT_MODEL.to_string()
}

fn default_google_model() -> String {
    "gemini-2.0-flash".into()
}

fn default_openai_model() -> String {
    "gpt-4o".into()
}

fn default_custom_model() -> String {
    "custom-model".into()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sound_enabled: true,
            sound_volume: 0.12,
            auto_close_interval: 15.0,
            absence_interval: 180.0,
            active_integrations: vec![
                "integration_resend".into(),
                "integration_n8n".into(),
                "integration_vercel".into(),
                "integration_github".into(),
            ],
            screen: "primary".into(),
            autostart: false,
            hooks_installed: false,
            chat_provider: default_chat_provider(),
            model: default_model(),
            google_model: default_google_model(),
            openai_model: default_openai_model(),
            custom_base_url: "".into(),
            custom_model: default_custom_model(),
        }
    }
}

pub use crate::platform::{config_dir, local_dir};

pub fn hook_exe_path() -> PathBuf {
    local_dir().join("bin").join(crate::platform::HOOK_EXE)
}

fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

pub fn load() -> Settings {
    match std::fs::read(settings_path()) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => Settings::default(),
    }
}

pub fn save(settings: &Settings) -> std::io::Result<()> {
    let dir = config_dir();
    crate::platform::ensure_private_dir(&dir)?;
    let json = serde_json::to_vec_pretty(settings)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(settings_path(), json)
}
