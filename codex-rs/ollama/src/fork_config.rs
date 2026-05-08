//! Fork-only loader for an opencode-style external JSON config.
//!
//! Reads `$CODEX_FORK_CONFIG` if set, else `$HOME/.config/xtech/xtech.json`.
//! Any present field overrides the corresponding env var so the JSON file is
//! the source of truth at runtime.
//!
//! Schema:
//! ```jsonc
//! {
//!   "baseURL": "http://<gateway-host>/v1",
//!   "apiKey":  "sk-davis-...",
//!   "model":   "qwen3.5-122b"
//! }
//! ```

use std::path::PathBuf;

use serde::Deserialize;
use tracing::warn;

const FORK_CONFIG_ENV: &str = "CODEX_FORK_CONFIG";
const BASE_URL_ENV: &str = "CODEX_OSS_BASE_URL";
const API_KEY_ENV: &str = "OLLAMA_API_KEY";
const MODEL_ENV: &str = "CODEX_OSS_MODEL";

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ForkConfigJson {
    /// `"baseURL"` is opencode's spelling and the primary key.
    /// Aliases accept the snake_case and strict-camelCase variants.
    #[serde(alias = "baseURL", alias = "base_url")]
    base_url: Option<String>,
    #[serde(alias = "api_key")]
    api_key: Option<String>,
    model: Option<String>,
}

fn resolve_path() -> Option<PathBuf> {
    if let Ok(explicit) = std::env::var(FORK_CONFIG_ENV)
        && !explicit.trim().is_empty()
    {
        return Some(PathBuf::from(explicit));
    }
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("xtech")
            .join("xtech.json"),
    )
}

/// Load the fork config (if present) and export each populated field as the
/// matching env var. JSON values win over any pre-existing env value.
pub fn apply_fork_config_to_env() {
    let Some(path) = resolve_path() else {
        return;
    };
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return,
        Err(err) => {
            warn!(
                path = %path.display(),
                error = %err,
                "failed to read codex-fork.json; ignoring"
            );
            return;
        }
    };
    let parsed: ForkConfigJson = match serde_json::from_str(&raw) {
        Ok(parsed) => parsed,
        Err(err) => {
            warn!(
                path = %path.display(),
                error = %err,
                "failed to parse codex-fork.json; ignoring"
            );
            return;
        }
    };
    if let Some(base_url) = parsed.base_url.as_deref().filter(|v| !v.trim().is_empty()) {
        // SAFETY: called at process startup before any threads observe env.
        unsafe { std::env::set_var(BASE_URL_ENV, base_url) };
    }
    if let Some(api_key) = parsed.api_key.as_deref().filter(|v| !v.trim().is_empty()) {
        unsafe { std::env::set_var(API_KEY_ENV, api_key) };
    }
    if let Some(model) = parsed.model.as_deref().filter(|v| !v.trim().is_empty()) {
        unsafe { std::env::set_var(MODEL_ENV, model) };
    }
}

/// Returns the fork-configured model slug if `CODEX_OSS_MODEL` is set, else None.
pub fn fork_model_override() -> Option<String> {
    std::env::var(MODEL_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty())
}
