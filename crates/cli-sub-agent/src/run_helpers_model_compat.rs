//! Pre-spawn model-tool compatibility validation.
//!
//! Catches known-incompatible model selections before a session is spawned,
//! saving tokens and surfacing a clear error rather than a cryptic runtime
//! failure. This is primarily a safety net for `--force-ignore-tier-setting`,
//! where tier configuration would otherwise guarantee a compatible model.

use anyhow::Result;
use csa_core::types::ToolName;
use csa_executor::ThinkingBudget;
use std::sync::LazyLock;

/// Codex ChatGPT-account-incompatible models.
///
/// These are rejected by codex when using a ChatGPT subscription rather than
/// an OpenAI API key. The codex CLI surfaces this at runtime:
/// "The '<model>' model is not supported when using Codex with a ChatGPT account."
pub(crate) static CODEX_CHATGPT_INCOMPATIBLE_MODELS: LazyLock<Vec<String>> = LazyLock::new(|| {
    csa_core::model_catalog::shipped_compatibility_models("codex_chatgpt_incompatible")
        .expect("shipped compatibility policy must be valid")
});

/// Well-known codex ChatGPT-account-compatible models, listed in error hints.
pub(crate) static CODEX_CHATGPT_COMPATIBLE_MODELS: LazyLock<Vec<String>> = LazyLock::new(|| {
    csa_core::model_catalog::shipped_compatibility_models("codex_chatgpt_compatible")
        .expect("shipped compatibility policy must be valid")
});

/// Validate that `model` is compatible with `tool` before spawning a session.
///
/// Thinking-budget suffixes (e.g. `/high`) are stripped before comparison so
/// both `o4-mini` and `o4-mini/high` are caught, while `o4-mini-high` (a
/// distinct model with no `/` suffix) is accepted.
///
/// When `default_model` matches the base model name, validation is skipped —
/// an explicit `[tools.<name>].default_model` in user config signals intentional
/// use, and the user accepts the consequence.
pub(crate) fn validate_tool_model_compat(
    tool: ToolName,
    model: &str,
    default_model: Option<&str>,
) -> Result<()> {
    let (base_model, _) = ThinkingBudget::try_split_from_model(model);

    // When the model matches the tool's configured default, skip validation.
    if default_model.is_some_and(|dm| {
        let (dm_base, _) = ThinkingBudget::try_split_from_model(dm);
        dm_base == base_model
    }) {
        return Ok(());
    }

    match tool {
        ToolName::Codex => validate_codex_model_compat(base_model),
        _ => Ok(()),
    }
}

fn validate_codex_model_compat(model: &str) -> Result<()> {
    if CODEX_CHATGPT_INCOMPATIBLE_MODELS
        .iter()
        .any(|candidate| candidate == model)
    {
        let compatible = CODEX_CHATGPT_COMPATIBLE_MODELS.join(", ");
        anyhow::bail!(
            "'{model}' is not supported when using codex with a ChatGPT account.\n\
             Compatible models: {compatible}\n\
             To suppress this check, set this model as the configured default in \
             [tools.codex].default_model in your config."
        );
    }
    Ok(())
}
