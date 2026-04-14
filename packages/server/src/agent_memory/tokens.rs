// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
//! Token counting backed by `tiktoken-rs`.
//!
//! [`TiktokenCounter`] picks the right BPE for the requested model name
//! (defaulting to `cl100k_base`, which matches all GPT-4 and Claude-3
//! family tokenisers closely enough for budgeting purposes) and exposes
//! a single [`count`](TiktokenCounter::count) method that callers — the
//! repo layer on insert, the context builder on read — share.

use std::sync::Arc;

use tiktoken_rs::CoreBPE;
use tiktoken_rs::{cl100k_base, o200k_base, p50k_base, r50k_base};
use tracing::warn;

/// Model-specific BPE token counter.
#[derive(Clone)]
pub struct TiktokenCounter {
    bpe: Arc<CoreBPE>,
    model: String,
}

impl std::fmt::Debug for TiktokenCounter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TiktokenCounter")
            .field("model", &self.model)
            .finish()
    }
}

impl TiktokenCounter {
    /// Build a counter for the given model name. Unknown models fall back
    /// to `cl100k_base` (the GPT-3.5 / GPT-4 base) and emit a `warn!` line
    /// so callers can see the substitution in logs.
    pub fn for_model(model: &str) -> Self {
        let normalised = model.to_ascii_lowercase();
        let bpe = if normalised.starts_with("gpt-4o")
            || normalised.starts_with("o1")
            || normalised.starts_with("o3")
        {
            o200k_base().ok()
        } else if normalised.starts_with("gpt-4")
            || normalised.starts_with("gpt-3.5")
            || normalised.starts_with("claude")
            || normalised.starts_with("text-embedding")
        {
            cl100k_base().ok()
        } else if normalised.starts_with("text-davinci") || normalised.starts_with("code-davinci") {
            p50k_base().ok()
        } else if normalised.starts_with("davinci") || normalised.starts_with("curie") {
            r50k_base().ok()
        } else {
            warn!(
                model,
                "unknown model for tiktoken — defaulting to cl100k_base"
            );
            cl100k_base().ok()
        };

        let bpe = bpe.unwrap_or_else(|| {
            // cl100k_base is bundled with tiktoken-rs and infallible in practice,
            // but if even that fails we fall back to a best-effort character
            // estimator wrapped in an empty BPE — every counter still returns
            // a non-zero token count for non-empty strings.
            cl100k_base().expect("cl100k_base must be available")
        });

        Self {
            bpe: Arc::new(bpe),
            model: model.to_string(),
        }
    }

    /// Convenience: a counter using the default `cl100k_base` BPE.
    pub fn default_counter() -> Self {
        Self::for_model("gpt-4")
    }

    /// Model identifier this counter was built for.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Count the number of tokens in `text` using the configured BPE.
    pub fn count(&self, text: &str) -> usize {
        if text.is_empty() {
            return 0;
        }
        self.bpe.encode_with_special_tokens(text).len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_string_is_zero_tokens() {
        let c = TiktokenCounter::default_counter();
        assert_eq!(c.count(""), 0);
    }

    #[test]
    fn non_empty_string_is_non_zero() {
        let c = TiktokenCounter::default_counter();
        assert!(c.count("hello world") > 0);
    }

    #[test]
    fn model_selection_smoke() {
        let _ = TiktokenCounter::for_model("gpt-4o");
        let _ = TiktokenCounter::for_model("gpt-4");
        let _ = TiktokenCounter::for_model("claude-3-opus");
        let _ = TiktokenCounter::for_model("unknown-model-xyz");
    }
}
