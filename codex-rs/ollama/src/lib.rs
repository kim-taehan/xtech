mod client;
mod fork_config;
mod parser;
mod pull;
mod url;

pub use client::OllamaClient;
use codex_core::config::Config;
use codex_model_provider_info::ModelProviderInfo;
pub use fork_config::ForkConfigSnapshot;
pub use fork_config::ForkModel;
pub use fork_config::apply_fork_config_to_env;
pub use fork_config::apply_runtime_model_override;
pub use fork_config::fork_config_snapshot;
pub use fork_config::fork_model_by_name;
pub use fork_config::fork_model_override;
pub use fork_config::fork_models;
pub use pull::CliProgressReporter;
pub use pull::PullEvent;
pub use pull::PullProgressReporter;
pub use pull::TuiProgressReporter;
use semver::Version;

/// Default OSS model to use when `--oss` is passed without an explicit `-m`.
pub const DEFAULT_OSS_MODEL: &str = "qwen3.5-122b";

/// Prepare the local OSS environment when `--oss` is selected.
///
/// - Ensures a local Ollama server is reachable.
/// - Checks if the model exists locally and pulls it if missing.
pub async fn ensure_oss_ready(config: &Config) -> std::io::Result<()> {
    // Only download when the requested model is the default OSS model (or when -m is not provided).
    let model = match config.model.as_ref() {
        Some(model) => model,
        None => DEFAULT_OSS_MODEL,
    };

    // Verify local Ollama is reachable.
    let ollama_client = crate::OllamaClient::try_from_oss_provider(config).await?;

    // If the model is not present locally, pull it. An empty catalog usually
    // means the host is a non-Ollama OpenAI-compatible gateway that does not
    // expose `/api/tags` / `/api/pull` (auth proxies, LiteLLM, vLLM, etc.). In
    // that case there is nothing to pull and we trust the operator's model
    // choice — the actual `/v1/responses` call will surface mismatches.
    match ollama_client.fetch_models().await {
        Ok(models) if models.is_empty() => {
            tracing::debug!(
                "Skipping model auto-pull: server returned an empty model catalog \
                 (likely a non-Ollama OpenAI-compatible gateway)."
            );
        }
        Ok(models) => {
            if !models.iter().any(|m| m == model) {
                let mut reporter = crate::CliProgressReporter::new();
                ollama_client
                    .pull_with_reporter(model, &mut reporter)
                    .await?;
            }
        }
        Err(err) => {
            // Not fatal; higher layers may still proceed and surface errors later.
            tracing::warn!("Failed to query local models from Ollama: {}.", err);
        }
    }

    Ok(())
}

fn min_responses_version() -> Version {
    Version::new(0, 13, 4)
}

fn supports_responses(version: &Version) -> bool {
    *version == Version::new(0, 0, 0) || *version >= min_responses_version()
}

/// Ensure the running Ollama server is new enough to support the Responses API.
///
/// Returns `Ok(())` when the version endpoint is missing or unparsable, or
/// when the provider is configured for the Chat Completions wire API (in
/// which case the `/api/version` admin endpoint is irrelevant — the gateway
/// only needs to expose `/v1/chat/completions`).
pub async fn ensure_responses_supported(provider: &ModelProviderInfo) -> std::io::Result<()> {
    if provider.wire_api == codex_model_provider_info::WireApi::Chat {
        return Ok(());
    }

    let client = crate::OllamaClient::try_from_provider(provider).await?;
    let Some(version) = client.fetch_version().await? else {
        return Ok(());
    };

    if supports_responses(&version) {
        return Ok(());
    }

    let min = min_responses_version();
    Err(std::io::Error::other(format!(
        "Ollama {version} is too old. Codex requires Ollama {min} or newer."
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supports_responses_for_dev_zero() {
        assert!(supports_responses(&Version::new(0, 0, 0)));
    }

    #[test]
    fn does_not_support_responses_before_cutoff() {
        assert!(!supports_responses(&Version::new(0, 13, 3)));
    }

    #[test]
    fn supports_responses_at_or_after_cutoff() {
        assert!(supports_responses(&Version::new(0, 13, 4)));
        assert!(supports_responses(&Version::new(0, 14, 0)));
    }
}
