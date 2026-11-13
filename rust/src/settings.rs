//! Settings in `$XDG_CONFIG_HOME/codecanvas/settings.json`; the API key sits beside them, readable only by you.

use crate::llm::LlmConfig;
use crate::model::config_dir;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub provider: String,
    pub endpoint: String,
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub max_tool_calls: usize,
    pub searxng_url: String,
    pub max_results: usize,
    pub editor_command: String,
    pub exclude_dirs: String,
    pub explain_all_max_files: usize,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            provider: "llamacpp".into(), endpoint: String::new(), model: "local".into(), temperature: 0.2, max_tokens: 4096, max_tool_calls: 8,
            searxng_url: String::new(), max_results: 6, editor_command: String::new(),
            exclude_dirs: "node_modules, out, dist, build, .next, target, bin, obj, __pycache__, .venv, venv".into(), explain_all_max_files: 40,
        }
    }
}

fn settings_file() -> PathBuf { config_dir().join("settings.json") }
fn key_file() -> PathBuf { config_dir().join("apikey") }

impl Settings {
    pub fn load() -> Self { fs::read(settings_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default() }

    pub fn save(&self) -> std::io::Result<()> {
        fs::create_dir_all(config_dir())?;
        let tmp = settings_file().with_extension("tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        fs::rename(tmp, settings_file())
    }

    pub fn excluded(&self) -> Vec<String> { self.exclude_dirs.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect() }

    pub fn api_key() -> Option<String> {
        fs::read_to_string(key_file()).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
            .or_else(|| std::env::var("OPENAI_API_KEY").ok()).or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
    }

    pub fn set_api_key(key: &str) -> std::io::Result<()> {
        fs::create_dir_all(config_dir())?;
        if key.is_empty() { return fs::remove_file(key_file()).or(Ok(())); }
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        { use std::os::unix::fs::OpenOptionsExt; opts.mode(0o600); }
        std::io::Write::write_all(&mut opts.open(key_file())?, key.as_bytes())
    }

    pub fn llm(&self) -> LlmConfig { LlmConfig::new(&self.provider, &self.endpoint, &self.model, self.temperature, self.max_tokens, Self::api_key()) }
}
