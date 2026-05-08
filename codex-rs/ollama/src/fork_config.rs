//! Fork-only loader for an opencode-style external JSON config.
//!
//! Reads `$CODEX_FORK_CONFIG` if set, else `$HOME/.config/xtech/xtech.json`.
//! Any present field overrides the corresponding env var so the JSON file is
//! the source of truth at runtime.
//!
//! Schema (single-model, legacy):
//! ```jsonc
//! {
//!   "baseURL": "http://<gateway-host>/v1",
//!   "apiKey":  "sk-davis-...",
//!   "model":   "qwen3.5-122b"
//! }
//! ```
//!
//! Schema (multi-model):
//! ```jsonc
//! {
//!   "baseURL": "http://<gateway-host>/v1",
//!   "apiKey":  "sk-davis-...",
//!   "defaultModel": "qwen3.5-122b",
//!   "models": [
//!     "qwen3.5-122b",
//!     { "name": "deepseek-33b", "baseURL": "http://other/v1", "apiKey": "sk-other" }
//!   ]
//! }
//! ```
//!
//! Resolution rules for the active model at startup:
//! 1. `defaultModel` if set
//! 2. legacy top-level `model` field if set
//! 3. first entry of `models[]` if non-empty
//!
//! Per-model `baseURL`/`apiKey` overrides take precedence over the top-level
//! values when that model is active.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::RwLock;

use serde::Deserialize;
use tracing::warn;

const FORK_CONFIG_ENV: &str = "CODEX_FORK_CONFIG";
const BASE_URL_ENV: &str = "CODEX_OSS_BASE_URL";
const API_KEY_ENV: &str = "OLLAMA_API_KEY";
const MODEL_ENV: &str = "CODEX_OSS_MODEL";

/// Per-model entry parsed from `xtech.json`. Accepts either a bare string
/// (`"qwen3.5-122b"`) or an object with optional gateway overrides.
#[derive(Debug, Clone)]
pub struct ForkModel {
    pub name: String,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ForkConfigJson {
    /// `"baseURL"` is opencode's spelling and the primary key.
    /// Aliases accept the snake_case and strict-camelCase variants.
    #[serde(alias = "baseURL", alias = "base_url")]
    base_url: Option<String>,
    #[serde(alias = "api_key")]
    api_key: Option<String>,
    /// Legacy single-model field. Equivalent to `defaultModel` when only one
    /// model is configured.
    model: Option<String>,
    #[serde(alias = "default_model")]
    default_model: Option<String>,
    #[serde(default)]
    models: Vec<ForkModelEntryJson>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum ForkModelEntryJson {
    Bare(String),
    #[serde(rename_all = "camelCase")]
    Detailed {
        name: String,
        #[serde(alias = "baseURL", alias = "base_url")]
        base_url: Option<String>,
        #[serde(alias = "api_key")]
        api_key: Option<String>,
    },
}

impl ForkModelEntryJson {
    fn into_model(self) -> Option<ForkModel> {
        match self {
            ForkModelEntryJson::Bare(name) => {
                let name = name.trim().to_string();
                if name.is_empty() {
                    None
                } else {
                    Some(ForkModel {
                        name,
                        base_url: None,
                        api_key: None,
                    })
                }
            }
            ForkModelEntryJson::Detailed {
                name,
                base_url,
                api_key,
            } => {
                let name = name.trim().to_string();
                if name.is_empty() {
                    None
                } else {
                    Some(ForkModel {
                        name,
                        base_url: base_url.and_then(non_empty),
                        api_key: api_key.and_then(non_empty),
                    })
                }
            }
        }
    }
}

fn non_empty(s: String) -> Option<String> {
    if s.trim().is_empty() { None } else { Some(s) }
}

/// In-memory snapshot of the parsed config, kept after `apply_fork_config_to_env`
/// so the TUI can show the model list and apply per-model overrides on switch.
#[derive(Debug, Default, Clone)]
pub struct ForkConfigSnapshot {
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub default_model: Option<String>,
    pub models: Vec<ForkModel>,
}

static SNAPSHOT: OnceLock<RwLock<ForkConfigSnapshot>> = OnceLock::new();

fn snapshot_cell() -> &'static RwLock<ForkConfigSnapshot> {
    SNAPSHOT.get_or_init(|| RwLock::new(ForkConfigSnapshot::default()))
}

/// Returns the parsed snapshot of the fork config (empty if no config file
/// was found or parsing failed).
pub fn fork_config_snapshot() -> ForkConfigSnapshot {
    snapshot_cell()
        .read()
        .map(|guard| guard.clone())
        .unwrap_or_default()
}

/// Returns the list of models declared in the fork config.
pub fn fork_models() -> Vec<ForkModel> {
    fork_config_snapshot().models
}

/// Looks up a model entry by name (case-sensitive).
pub fn fork_model_by_name(name: &str) -> Option<ForkModel> {
    fork_config_snapshot()
        .models
        .into_iter()
        .find(|m| m.name == name)
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

    let top_base_url = parsed.base_url.and_then(non_empty);
    let top_api_key = parsed.api_key.and_then(non_empty);
    let legacy_model = parsed.model.and_then(non_empty);
    let configured_default = parsed.default_model.and_then(non_empty);

    let mut models: Vec<ForkModel> = parsed
        .models
        .into_iter()
        .filter_map(ForkModelEntryJson::into_model)
        .collect();

    // If a legacy top-level `model` is set but missing from `models[]`, fold
    // it in so the multi-model picker still surfaces it.
    if let Some(name) = &legacy_model
        && !models.iter().any(|m| m.name == *name)
    {
        models.insert(
            0,
            ForkModel {
                name: name.clone(),
                base_url: None,
                api_key: None,
            },
        );
    }

    // Resolve the active default: defaultModel → legacy `model` → models[0].
    let active_name = configured_default
        .or(legacy_model)
        .or_else(|| models.first().map(|m| m.name.clone()));

    // Apply env vars: top-level baseURL/apiKey first, then per-model overrides
    // for the active model take precedence.
    if let Some(base_url) = top_base_url.as_deref() {
        // SAFETY: called at process startup before any threads observe env.
        unsafe { std::env::set_var(BASE_URL_ENV, base_url) };
    }
    if let Some(api_key) = top_api_key.as_deref() {
        unsafe { std::env::set_var(API_KEY_ENV, api_key) };
    }

    if let Some(name) = active_name.as_deref() {
        unsafe { std::env::set_var(MODEL_ENV, name) };
        if let Some(active) = models.iter().find(|m| m.name == name) {
            if let Some(base_url) = active.base_url.as_deref() {
                unsafe { std::env::set_var(BASE_URL_ENV, base_url) };
            }
            if let Some(api_key) = active.api_key.as_deref() {
                unsafe { std::env::set_var(API_KEY_ENV, api_key) };
            }
        }
    }

    let snapshot = ForkConfigSnapshot {
        base_url: top_base_url,
        api_key: top_api_key,
        default_model: active_name,
        models,
    };
    if let Ok(mut guard) = snapshot_cell().write() {
        *guard = snapshot;
    }
}

/// Returns the fork-configured model slug if `CODEX_OSS_MODEL` is set, else None.
pub fn fork_model_override() -> Option<String> {
    std::env::var(MODEL_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty())
}

/// Apply the override for `model_name` at runtime: rewrites the env vars
/// observed by `ModelProviderInfo::api_key()` and returns the resolved
/// `(base_url, api_key)` pair so the caller can also patch any cached
/// provider info that already read these values at startup.
///
/// If the model is not in the fork config, falls back to top-level
/// baseURL/apiKey from the snapshot.
pub fn apply_runtime_model_override(model_name: &str) -> (Option<String>, Option<String>) {
    let snapshot = fork_config_snapshot();
    let entry = snapshot.models.iter().find(|m| m.name == model_name);

    let resolved_base_url = entry
        .and_then(|m| m.base_url.clone())
        .or_else(|| snapshot.base_url.clone());
    let resolved_api_key = entry
        .and_then(|m| m.api_key.clone())
        .or_else(|| snapshot.api_key.clone());

    // SAFETY: env mutation is racy with multi-threaded readers in the strict
    // POSIX sense, but `OLLAMA_API_KEY` is only read inside
    // `ModelProviderInfo::api_key()` (per-request) and the worst case is that
    // one in-flight request observes the previous value.
    if let Some(base_url) = resolved_base_url.as_deref() {
        unsafe { std::env::set_var(BASE_URL_ENV, base_url) };
    }
    if let Some(api_key) = resolved_api_key.as_deref() {
        unsafe { std::env::set_var(API_KEY_ENV, api_key) };
    }
    unsafe { std::env::set_var(MODEL_ENV, model_name) };

    (resolved_base_url, resolved_api_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_legacy_single_model_form() {
        let json = r#"{
            "baseURL": "http://gw/v1",
            "apiKey":  "sk-top",
            "model":   "qwen3.5-122b"
        }"#;
        let parsed: ForkConfigJson = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.base_url.as_deref(), Some("http://gw/v1"));
        assert_eq!(parsed.api_key.as_deref(), Some("sk-top"));
        assert_eq!(parsed.model.as_deref(), Some("qwen3.5-122b"));
        assert!(parsed.models.is_empty());
        assert!(parsed.default_model.is_none());
    }

    #[test]
    fn parses_multi_model_form_with_mixed_entries() {
        let json = r#"{
            "baseURL": "http://gw/v1",
            "apiKey":  "sk-top",
            "defaultModel": "qwen3.5-122b",
            "models": [
                "qwen3.5-122b",
                {
                    "name": "deepseek-33b",
                    "baseURL": "http://other/v1",
                    "apiKey": "sk-other"
                }
            ]
        }"#;
        let parsed: ForkConfigJson = serde_json::from_str(json).unwrap();
        let models: Vec<ForkModel> = parsed
            .models
            .into_iter()
            .filter_map(ForkModelEntryJson::into_model)
            .collect();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].name, "qwen3.5-122b");
        assert!(models[0].base_url.is_none());
        assert_eq!(models[1].name, "deepseek-33b");
        assert_eq!(models[1].base_url.as_deref(), Some("http://other/v1"));
        assert_eq!(models[1].api_key.as_deref(), Some("sk-other"));
        assert_eq!(parsed.default_model.as_deref(), Some("qwen3.5-122b"));
    }

    #[test]
    fn drops_blank_entries() {
        let bare = ForkModelEntryJson::Bare("   ".to_string());
        assert!(bare.into_model().is_none());
        let detailed = ForkModelEntryJson::Detailed {
            name: " ".to_string(),
            base_url: None,
            api_key: None,
        };
        assert!(detailed.into_model().is_none());
    }
}
