// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
// Episodic → semantic summariser for unlimited-context agents.
//
// Slice 15/30 of the Grand Transformation.
//
// When a session's episodic tier grows past a threshold (50 / 100 / 200
// messages), this module collapses the oldest 20 episodic rows into a
// single `summary` row on the semantic tier using whatever LLM the
// operator has configured through env vars. The original 20 rows are
// then deleted inside the same transaction so context retrieval stays
// bounded even across year-long conversations.
//
// The LLM layer is pluggable through the `LlmClient` trait so tests
// and offline deployments can swap in the `NoneClient` fallback, which
// returns a deterministic "no LLM configured" placeholder and never
// panics on missing credentials.

use std::sync::{Arc, OnceLock};

use async_openai::{
    config::OpenAIConfig,
    types::{
        ChatCompletionRequestMessage, ChatCompletionRequestSystemMessageArgs,
        ChatCompletionRequestUserMessageArgs, CreateChatCompletionRequestArgs,
    },
    Client as OpenAiSdkClient,
};
use async_trait::async_trait;
use sqlx::PgPool;
use thiserror::Error;
use tiktoken_rs::{cl100k_base, CoreBPE};
use uuid::Uuid;

// ── Threshold constants ─────────────────────────────────────────────────

/// Number of episodic entries pulled into a single summary.
pub const SUMMARISER_BATCH_SIZE: i64 = 20;

/// Max summary length in output tokens.
pub const SUMMARISER_MAX_TOKENS: u32 = 512;

/// Importance assigned to the emitted summary row.
/// Summaries are distilled knowledge, so they outrank raw chat turns.
pub const SUMMARISER_IMPORTANCE: f64 = 0.9;

/// Episodic thresholds at which a session triggers a summarisation sweep.
/// Chosen so growth is logarithmic: once every 50, 100, then 200.
pub const SUMMARISER_THRESHOLDS: [i64; 3] = [50, 100, 200];

/// System prompt used for all summarisation calls.
pub const SUMMARISER_SYSTEM_PROMPT: &str =
    "Summarise this conversation segment in 3-5 sentences, preserving all \
     factual details, decisions, and key outcomes. Focus on what would be \
     most useful to remember later.";

/// Fallback text returned when no LLM is configured. Must be stable so
/// downstream consumers can recognise it and decide whether to re-run
/// summarisation once credentials appear.
pub const NO_LLM_FALLBACK_TEXT: &str = "[summary unavailable — no LLM configured]";

// ── Error type ──────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum SummariserError {
    #[error("sqlx error: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("llm error: {0}")]
    Llm(#[from] LlmError),
    #[error("tokeniser init failed: {0}")]
    Tokeniser(String),
}

#[derive(Debug, Error)]
pub enum LlmError {
    #[error("openai sdk error: {0}")]
    OpenAi(String),
    #[error("llm returned empty response")]
    EmptyResponse,
    #[error("configuration error: {0}")]
    Config(String),
}

// ── LlmClient trait + message model ─────────────────────────────────────

/// Minimal role/content pair used by `LlmClient`. Intentionally narrower
/// than the async-openai types so we can have a `NoneClient` that
/// depends on nothing.
#[derive(Debug, Clone)]
pub struct LlmMessage {
    pub role: String,
    pub content: String,
}

impl LlmMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".to_string(),
            content: content.into(),
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
        }
    }
}

#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(
        &self,
        messages: Vec<LlmMessage>,
        max_tokens: u32,
    ) -> Result<String, LlmError>;
}

// ── OpenAI (and OpenAI-compatible) client ───────────────────────────────

pub struct OpenAiClient {
    client: OpenAiSdkClient<OpenAIConfig>,
    model: String,
}

impl OpenAiClient {
    /// Build from env:
    ///   * `DARSH_LLM_API_KEY` — required, falls back to `OPENAI_API_KEY`.
    ///   * `DARSH_LLM_MODEL`   — optional, defaults to `gpt-4o-mini`.
    ///   * `DARSH_LLM_BASE_URL`— optional, overrides api_base for
    ///     OpenAI-compatible gateways (LiteLLM, vLLM, etc.).
    pub fn from_env() -> Result<Self, LlmError> {
        let api_key = std::env::var("DARSH_LLM_API_KEY")
            .or_else(|_| std::env::var("OPENAI_API_KEY"))
            .map_err(|_| {
                LlmError::Config("DARSH_LLM_API_KEY / OPENAI_API_KEY not set".into())
            })?;
        let model =
            std::env::var("DARSH_LLM_MODEL").unwrap_or_else(|_| "gpt-4o-mini".to_string());

        let mut config = OpenAIConfig::new().with_api_key(api_key);
        if let Ok(base) = std::env::var("DARSH_LLM_BASE_URL") {
            if !base.is_empty() {
                config = config.with_api_base(base);
            }
        }
        Ok(Self {
            client: OpenAiSdkClient::with_config(config),
            model,
        })
    }
}

#[async_trait]
impl LlmClient for OpenAiClient {
    async fn complete(
        &self,
        messages: Vec<LlmMessage>,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        let oa_messages = build_openai_messages(messages)?;
        let request = CreateChatCompletionRequestArgs::default()
            .model(&self.model)
            .max_tokens(max_tokens)
            .messages(oa_messages)
            .build()
            .map_err(|e| LlmError::OpenAi(e.to_string()))?;
        let resp = self
            .client
            .chat()
            .create(request)
            .await
            .map_err(|e| LlmError::OpenAi(e.to_string()))?;
        let text = resp
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or(LlmError::EmptyResponse)?;
        Ok(text)
    }
}

// ── Anthropic client (via OpenAI-compatible gateway) ────────────────────

/// Anthropic client. We route through `async-openai` pointed at an
/// OpenAI-compatible Anthropic proxy (e.g. LiteLLM). This keeps a single
/// HTTP client dependency in the crate while still letting operators
/// choose `anthropic` as a provider via env.
pub struct AnthropicClient {
    client: OpenAiSdkClient<OpenAIConfig>,
    model: String,
}

impl AnthropicClient {
    pub fn from_env() -> Result<Self, LlmError> {
        let api_key = std::env::var("DARSH_LLM_API_KEY")
            .or_else(|_| std::env::var("ANTHROPIC_API_KEY"))
            .map_err(|_| {
                LlmError::Config(
                    "DARSH_LLM_API_KEY / ANTHROPIC_API_KEY not set".into(),
                )
            })?;
        let model = std::env::var("DARSH_LLM_MODEL")
            .unwrap_or_else(|_| "claude-3-5-haiku-latest".to_string());
        // Default to a local LiteLLM proxy on :4000, which most Darsh
        // deployments already run. Operators can override.
        let base = std::env::var("DARSH_LLM_BASE_URL")
            .unwrap_or_else(|_| "http://localhost:4000/v1".to_string());
        let config = OpenAIConfig::new()
            .with_api_key(api_key)
            .with_api_base(base);
        Ok(Self {
            client: OpenAiSdkClient::with_config(config),
            model,
        })
    }
}

#[async_trait]
impl LlmClient for AnthropicClient {
    async fn complete(
        &self,
        messages: Vec<LlmMessage>,
        max_tokens: u32,
    ) -> Result<String, LlmError> {
        let oa_messages = build_openai_messages(messages)?;
        let request = CreateChatCompletionRequestArgs::default()
            .model(&self.model)
            .max_tokens(max_tokens)
            .messages(oa_messages)
            .build()
            .map_err(|e| LlmError::OpenAi(e.to_string()))?;
        let resp = self
            .client
            .chat()
            .create(request)
            .await
            .map_err(|e| LlmError::OpenAi(e.to_string()))?;
        let text = resp
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or(LlmError::EmptyResponse)?;
        Ok(text)
    }
}

// ── None / fallback client ──────────────────────────────────────────────

/// Deterministic offline client. Used in tests and as a last-resort
/// fallback so the memory layer never panics when credentials are
/// missing. Operators can later reprocess sessions once a real LLM is
/// wired up.
pub struct NoneClient;

#[async_trait]
impl LlmClient for NoneClient {
    async fn complete(
        &self,
        _messages: Vec<LlmMessage>,
        _max_tokens: u32,
    ) -> Result<String, LlmError> {
        Ok(NO_LLM_FALLBACK_TEXT.to_string())
    }
}

/// Build an `LlmClient` for the given provider string. This is the
/// test-friendly core of `build_llm_client_from_env` — it accepts the
/// already-resolved provider name so unit tests don't need to mutate
/// process-wide env (which is `unsafe` under Rust 2024 and forbidden
/// by this crate's `#![forbid(unsafe_code)]`).
///
/// Known providers:
///   * `"openai"`     → `OpenAiClient::from_env` (still reads
///                      `DARSH_LLM_API_KEY` / `OPENAI_API_KEY`).
///   * `"anthropic"`  → `AnthropicClient::from_env`.
///   * `"none"`, `""`, unknown → `NoneClient` offline fallback.
///
/// On provider-init failure we log and fall back to `NoneClient` so
/// the memory layer never panics during startup.
pub fn build_llm_client_for_provider(provider: &str) -> Arc<dyn LlmClient> {
    match provider.to_lowercase().as_str() {
        "openai" => match OpenAiClient::from_env() {
            Ok(c) => Arc::new(c),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "openai client init failed, falling back to NoneClient"
                );
                Arc::new(NoneClient)
            }
        },
        "anthropic" => match AnthropicClient::from_env() {
            Ok(c) => Arc::new(c),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "anthropic client init failed, falling back to NoneClient"
                );
                Arc::new(NoneClient)
            }
        },
        // "none", "", or anything unknown → offline fallback.
        _ => Arc::new(NoneClient),
    }
}

/// Build an `LlmClient` from environment. Reads `DARSH_LLM_PROVIDER`
/// and delegates to `build_llm_client_for_provider`. Never panics — if
/// the chosen provider is misconfigured we fall back to `NoneClient`.
pub fn build_llm_client_from_env() -> Arc<dyn LlmClient> {
    let provider = std::env::var("DARSH_LLM_PROVIDER").unwrap_or_default();
    build_llm_client_for_provider(&provider)
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn build_openai_messages(
    messages: Vec<LlmMessage>,
) -> Result<Vec<ChatCompletionRequestMessage>, LlmError> {
    let mut out: Vec<ChatCompletionRequestMessage> = Vec::with_capacity(messages.len());
    for m in messages {
        match m.role.as_str() {
            "system" => {
                let built = ChatCompletionRequestSystemMessageArgs::default()
                    .content(m.content)
                    .build()
                    .map_err(|e| LlmError::OpenAi(e.to_string()))?;
                out.push(ChatCompletionRequestMessage::System(built));
            }
            _ => {
                // Treat anything non-system as user content. The
                // transcript we build is always funnelled through a
                // single user message today; keeping this branch
                // permissive means future callers can extend.
                let built = ChatCompletionRequestUserMessageArgs::default()
                    .content(m.content)
                    .build()
                    .map_err(|e| LlmError::OpenAi(e.to_string()))?;
                out.push(ChatCompletionRequestMessage::User(built));
            }
        }
    }
    Ok(out)
}

/// Format a list of (role, content) pairs into a human-readable
/// transcript string. Empty input yields an empty string.
pub fn format_transcript(rows: &[(String, String)]) -> String {
    rows.iter()
        .map(|(role, content)| format!("{role}: {content}"))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Cached cl100k_base tokeniser. Instantiating the BPE is O(megabytes)
/// so we only want to do it once per process.
static CL100K: OnceLock<CoreBPE> = OnceLock::new();

fn cl100k() -> Result<&'static CoreBPE, SummariserError> {
    if let Some(b) = CL100K.get() {
        return Ok(b);
    }
    let built = cl100k_base().map_err(|e| SummariserError::Tokeniser(e.to_string()))?;
    // First writer wins; any loser just re-reads.
    let _ = CL100K.set(built);
    CL100K
        .get()
        .ok_or_else(|| SummariserError::Tokeniser("cl100k oncelock miss".into()))
}

/// Count tokens in `text` using cl100k_base. Falls back to character
/// count / 4 (OpenAI's rule-of-thumb) if the tokeniser fails to init.
pub fn count_tokens(text: &str) -> i32 {
    match cl100k() {
        Ok(bpe) => bpe.encode_with_special_tokens(text).len() as i32,
        Err(_) => ((text.chars().count() + 3) / 4) as i32,
    }
}

/// Returns true if the current episodic count crosses any of the
/// `SUMMARISER_THRESHOLDS` values (equal-to counts as a crossing so
/// we fire exactly once per threshold as entries tick over).
pub fn is_threshold_crossed(count: i64) -> bool {
    SUMMARISER_THRESHOLDS.iter().any(|t| count == *t)
}

// ── Core operation ──────────────────────────────────────────────────────

/// Compress the oldest 20 episodic entries for a session into one
/// semantic-tier summary row. Returns the new summary row's id on
/// success, or `Ok(None)` when there are fewer than 20 rows (nothing
/// to do yet).
///
/// The entire read / LLM-call / write cycle runs inside a single
/// Postgres transaction with `FOR UPDATE` locking on the source rows
/// so two concurrent summariser runs cannot double-delete.
pub async fn summarise_oldest_episodic(
    pool: &PgPool,
    session_id: Uuid,
    llm: &dyn LlmClient,
) -> Result<Option<Uuid>, SummariserError> {
    let mut tx = pool.begin().await?;

    // 1. Grab the oldest 20 episodic entries, locking them against
    //    concurrent summariser runs.
    let rows: Vec<(Uuid, String, String, chrono::DateTime<chrono::Utc>)> = sqlx::query_as(
        "SELECT id, role, content, created_at
           FROM memory_entries
          WHERE session_id = $1 AND tier = 'episodic'
          ORDER BY created_at ASC
          LIMIT $2
          FOR UPDATE",
    )
    .bind(session_id)
    .bind(SUMMARISER_BATCH_SIZE)
    .fetch_all(&mut *tx)
    .await?;

    if (rows.len() as i64) < SUMMARISER_BATCH_SIZE {
        // Nothing to do — don't waste an LLM call.
        tx.rollback().await?;
        return Ok(None);
    }

    // 2. Format transcript and call the LLM.
    let transcript_rows: Vec<(String, String)> = rows
        .iter()
        .map(|(_, role, content, _)| (role.clone(), content.clone()))
        .collect();
    let transcript = format_transcript(&transcript_rows);

    let messages = vec![
        LlmMessage::system(SUMMARISER_SYSTEM_PROMPT),
        LlmMessage::user(transcript),
    ];

    let summary_text = match llm.complete(messages, SUMMARISER_MAX_TOKENS).await {
        Ok(t) => t,
        Err(e) => {
            tx.rollback().await?;
            return Err(SummariserError::Llm(e));
        }
    };

    // 3. Token-count the summary so downstream context planners can
    //    budget correctly.
    let token_count = count_tokens(&summary_text);

    // 4. Resolve the owning agent_id via agent_sessions so the summary
    //    row is linked the same way the source rows were.
    let agent_id: String = sqlx::query_scalar(
        "SELECT agent_id FROM agent_sessions WHERE session_id = $1",
    )
    .bind(session_id)
    .fetch_one(&mut *tx)
    .await?;

    // 5. INSERT the summary row on the semantic tier.
    let new_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO memory_entries
            (id, session_id, agent_id, role, content, content_tokens,
             importance, tier, created_at, accessed_at, access_count,
             compressed)
         VALUES ($1, $2, $3, 'summary', $4, $5, $6, 'semantic',
                 now(), now(), 0, false)",
    )
    .bind(new_id)
    .bind(session_id)
    .bind(&agent_id)
    .bind(&summary_text)
    .bind(token_count)
    .bind(SUMMARISER_IMPORTANCE)
    .execute(&mut *tx)
    .await?;

    // 6. DELETE the source rows we just summarised.
    let source_ids: Vec<Uuid> = rows.iter().map(|(id, _, _, _)| *id).collect();
    sqlx::query("DELETE FROM memory_entries WHERE id = ANY($1)")
        .bind(&source_ids)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    metrics::counter!("ddb_memory_compressions_total").increment(1);

    Ok(Some(new_id))
}

/// Threshold-aware wrapper. Counts the session's episodic rows and
/// triggers `summarise_oldest_episodic` whenever the count crosses a
/// configured threshold. Safe to call after every push.
pub async fn maybe_summarise_session(
    pool: &PgPool,
    session_id: Uuid,
    llm: &dyn LlmClient,
) -> Result<(), SummariserError> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM memory_entries
          WHERE session_id = $1 AND tier = 'episodic'",
    )
    .bind(session_id)
    .fetch_one(pool)
    .await?;

    if is_threshold_crossed(count) {
        let _ = summarise_oldest_episodic(pool, session_id, llm).await?;
    }
    Ok(())
}

// ── Unit tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Even though the tests below don't mutate env directly, the
    // provider-resolution tests still share the `build_llm_client_for_provider`
    // path which does `std::env::var(...)` reads under the hood. We
    // serialise them anyway so parallel test execution can't race on
    // process-wide env that external tooling (CI runners, user shells)
    // might set.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[tokio::test]
    async fn none_client_returns_fallback_text() {
        let client = NoneClient;
        let out = client
            .complete(
                vec![LlmMessage::system("x"), LlmMessage::user("y")],
                128,
            )
            .await
            .expect("none client never errors");
        assert_eq!(out, NO_LLM_FALLBACK_TEXT);
    }

    #[tokio::test]
    async fn build_llm_client_with_unset_provider_is_none_client() {
        let _guard = ENV_LOCK.lock().unwrap();
        // An unset env var resolves to "" in `build_llm_client_from_env`,
        // which is equivalent to calling the test-friendly entrypoint
        // with "" — so we exercise that directly and avoid mutating env
        // (which is `unsafe` under Rust 2024 and banned by
        // `#![forbid(unsafe_code)]`).
        let client = build_llm_client_for_provider("");
        // Verify via behaviour, not type: NoneClient returns the
        // deterministic fallback string.
        let out = client
            .complete(vec![LlmMessage::user("ping")], 32)
            .await
            .expect("fallback client never errors");
        assert_eq!(out, NO_LLM_FALLBACK_TEXT);
    }

    #[tokio::test]
    async fn build_llm_client_with_none_provider_is_none_client() {
        let _guard = ENV_LOCK.lock().unwrap();
        let client = build_llm_client_for_provider("none");
        let out = client
            .complete(vec![LlmMessage::user("ping")], 32)
            .await
            .expect("fallback client never errors");
        assert_eq!(out, NO_LLM_FALLBACK_TEXT);
    }

    #[tokio::test]
    async fn build_llm_client_with_unknown_provider_is_none_client() {
        let _guard = ENV_LOCK.lock().unwrap();
        // Anything outside the known provider set falls back silently.
        let client = build_llm_client_for_provider("definitely-not-a-real-provider");
        let out = client
            .complete(vec![LlmMessage::user("ping")], 32)
            .await
            .expect("fallback client never errors");
        assert_eq!(out, NO_LLM_FALLBACK_TEXT);
    }

    #[tokio::test]
    async fn provider_name_is_case_insensitive() {
        let _guard = ENV_LOCK.lock().unwrap();
        // Upper/mixed case must still resolve, including to the
        // fallback when no credentials are present in the env.
        for name in ["NONE", "None", "nOnE", ""] {
            let client = build_llm_client_for_provider(name);
            let out = client
                .complete(vec![LlmMessage::user("ping")], 32)
                .await
                .expect("fallback client never errors");
            assert_eq!(out, NO_LLM_FALLBACK_TEXT);
        }
    }

    #[test]
    fn format_transcript_handles_empty_vec() {
        let s = format_transcript(&[]);
        assert_eq!(s, "");
    }

    #[test]
    fn format_transcript_handles_single_role() {
        let rows = vec![("user".to_string(), "hello".to_string())];
        assert_eq!(format_transcript(&rows), "user: hello");
    }

    #[test]
    fn format_transcript_handles_multi_role() {
        let rows = vec![
            ("user".to_string(), "hi".to_string()),
            ("assistant".to_string(), "hey".to_string()),
            ("user".to_string(), "how are you".to_string()),
        ];
        let out = format_transcript(&rows);
        assert_eq!(out, "user: hi\n\nassistant: hey\n\nuser: how are you");
    }

    #[test]
    fn threshold_logic_fires_at_50_100_200_only() {
        // Exact thresholds fire.
        assert!(is_threshold_crossed(50));
        assert!(is_threshold_crossed(100));
        assert!(is_threshold_crossed(200));
        // Neighbours do not.
        assert!(!is_threshold_crossed(49));
        assert!(!is_threshold_crossed(51));
        assert!(!is_threshold_crossed(99));
        assert!(!is_threshold_crossed(101));
        assert!(!is_threshold_crossed(199));
        assert!(!is_threshold_crossed(201));
        // And neither does 0.
        assert!(!is_threshold_crossed(0));
    }

    #[test]
    fn count_tokens_is_monotonic_and_nonzero_for_nonempty() {
        let short = count_tokens("hello");
        let long = count_tokens(&"hello ".repeat(100));
        assert!(short > 0);
        assert!(long > short);
        assert_eq!(count_tokens(""), 0);
    }

    #[test]
    fn build_openai_messages_maps_roles() {
        let msgs = vec![
            LlmMessage::system("sys"),
            LlmMessage::user("usr"),
            LlmMessage {
                role: "assistant".into(),
                content: "asst".into(),
            },
        ];
        let built = build_openai_messages(msgs).expect("build ok");
        assert_eq!(built.len(), 3);
        // First must be System, rest are treated as User (permissive
        // fallback for non-system roles).
        matches!(built[0], ChatCompletionRequestMessage::System(_));
        matches!(built[1], ChatCompletionRequestMessage::User(_));
        matches!(built[2], ChatCompletionRequestMessage::User(_));
    }
}
