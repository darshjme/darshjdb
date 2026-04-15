// DarshJDB — created by Darshankumar Joshi (github.com/darshjme)
//
//! SQL dialect abstraction for the DarshanQL planner.
//!
//! v0.3.1 shipped with Postgres-specific SQL baked into
//! [`crate::query::plan_query`] and [`crate::query::plan_hybrid_query`]:
//! `to_jsonb(...)`, `::uuid`, `@>`, `#>> '{}'`, `ANY($1::uuid[])`,
//! `to_tsvector`, `<=>` — none of which run under SQLite. The goal of
//! this module is to funnel every SQL-dialect-specific fragment that
//! the planner emits through a single trait so the same logical plan
//! can target Postgres or SQLite at runtime without re-authoring the
//! planner.
//!
//! # Design
//!
//! `SqlDialect` is intentionally a *fragment emitter*, not a full
//! query builder. The planner still orchestrates joins, aliases, and
//! parameter indices; the dialect only decides *how* each piece is
//! spelled. This keeps the refactor small (no new AST, no IR) and
//! lets [`PgDialect`] preserve v0.3.1's output byte-for-byte.
//!
//! # Coverage
//!
//! Each method on [`SqlDialect`] corresponds to exactly one Postgres
//! feature the v0.3.1 planner relied on. See the per-method docs for
//! the Postgres expansion and its SQLite equivalent.
//!
//! # What this does *not* solve
//!
//! - `pgvector` cosine distance (`<=>`, `vector` type) — SQLite has
//!   no native vector type. [`SqliteDialect::cosine_distance`] and
//!   [`SqliteDialect::vector_literal`] return prepare-time failing
//!   sentinels (`__SQLITE_VECTOR_UNSUPPORTED__` /
//!   `__SQLITE_COSINE_DISTANCE_UNSUPPORTED__`) so a missed
//!   `supports_vector()` gate fails loudly at statement prepare
//!   time instead of silently returning zero rows. The v0.4 roadmap
//!   introduces an in-process vector fallback.
//! - Full-text search (`to_tsvector` / `plainto_tsquery`) — SQLite
//!   has FTS5 which requires a virtual table, not an expression.
//!   [`SqliteDialect`] falls back to a `LIKE` expression so the
//!   SELECT parses; production sprint follow-up can introduce FTS5.
//!
//! Both limitations are documented in `docs/SQL_DIALECTS.md`.

use std::fmt::Write;

/// Kind of a bound SQL parameter.
///
/// The planner hands the dialect raw JSON values and indicates whether
/// they should be bound as TEXT (which requires `to_jsonb` wrapping on
/// Postgres for JSONB column comparisons) or as a native JSON value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamKind {
    /// The parameter is a plain string — on Postgres it must be cast
    /// to `jsonb` before comparing against the `triples.value` column.
    Text,
    /// The parameter is an already-shaped JSON value; bind it directly.
    Json,
}

/// Direction of an ORDER BY clause.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    /// `ASC`
    Asc,
    /// `DESC`
    Desc,
}

impl SortDir {
    /// Keyword for the sort direction. Both dialects agree.
    pub fn keyword(self) -> &'static str {
        match self {
            SortDir::Asc => "ASC",
            SortDir::Desc => "DESC",
        }
    }
}

/// SQL dialect emitter used by the DarshanQL planner.
///
/// Implementations must be stateless, cheap to clone, and `Send + Sync`
/// so the plan cache can share one instance across concurrent requests.
pub trait SqlDialect: Send + Sync + std::fmt::Debug {
    /// Human-readable dialect name (`"postgres"`, `"sqlite"`).
    fn name(&self) -> &'static str;

    // ── Parameters ──────────────────────────────────────────────────

    /// Render the N-th (1-based) bind parameter.
    ///
    /// - Postgres: `$1`, `$2`, …
    /// - SQLite: `?1`, `?2`, … (numbered form is supported by both
    ///   `rusqlite` and `sqlx-sqlite` and matches our 1-based indexing).
    fn placeholder(&self, idx: u32) -> String;

    /// Wrap a bound parameter so it can be compared against a JSONB
    /// (or JSON-text) column.
    ///
    /// - Postgres: `to_jsonb($1::text)` for string params, `$1::jsonb`
    ///   otherwise.
    /// - SQLite: the column is stored as TEXT, so strings become
    ///   `json_quote(?1)` and non-strings are bound directly.
    fn jsonb_param(&self, idx: u32, kind: ParamKind) -> String;

    // ── Value comparison ────────────────────────────────────────────

    /// Build a `triples.value` comparison of the form
    /// `<col> <op> <param>`.
    ///
    /// `op` is the neutral comparison token (`=`, `!=`, `>`, etc.);
    /// the dialect is responsible for any wrapping required to make
    /// the comparison type-safe on the target store.
    fn compare_triple_value(&self, alias: &str, op: &str, param: &str) -> String;

    /// Build a JSON-containment check of the form `<col> @> <param>`.
    ///
    /// - Postgres: `<alias>.value @> <param>`
    /// - SQLite: unsound substring fallback via `instr()` — callers MUST
    ///   check [`Self::supports_jsonb_contains`] before reaching this
    ///   method. The SQLite fallback is only kept so a surprise call
    ///   site does not panic; the planner refuses Contains on SQLite.
    fn jsonb_contains(&self, alias: &str, param: &str) -> String;

    /// Whether this dialect supports structural JSON containment
    /// (`@>` on Postgres, nothing equivalent on SQLite).
    ///
    /// The planner MUST check this before emitting a
    /// [`crate::query::WhereOp::Contains`] clause and refuse with
    /// `InvalidQuery` when false. The default is `true` to preserve
    /// the v0.3.1 Postgres behaviour for any downstream dialect that
    /// does not override it.
    fn supports_jsonb_contains(&self) -> bool {
        true
    }

    /// Build a case-insensitive LIKE prefix match against the raw
    /// text of a JSON value.
    ///
    /// - Postgres: `<alias>.value #>> '{}' ILIKE <param>`
    /// - SQLite: `<alias>.value LIKE <param>` — SQLite's `LIKE` is
    ///   case-insensitive for ASCII by default, and the triple column
    ///   stores JSON as TEXT so no unwrap is needed.
    fn text_ilike(&self, alias: &str, param: &str) -> String;

    // ── Casts ───────────────────────────────────────────────────────

    /// Cast a bound parameter to the store's UUID type.
    ///
    /// - Postgres: `<param>::uuid`
    /// - SQLite: `<param>` (UUIDs are stored as TEXT).
    fn uuid_cast(&self, param: &str) -> String;

    /// Cast a bound parameter to a UUID array (for batched fetches).
    ///
    /// - Postgres: `<param>::uuid[]`
    /// - SQLite: `<param>` — SQLite has no array type, so the planner
    ///   rewrites `ANY($1::uuid[])` queries using [`Self::in_uuid_list`]
    ///   instead. This method is still exposed for callers that want
    ///   a single cast token.
    fn uuid_array_cast(&self, param: &str) -> String;

    /// Whether this dialect supports baking a single
    /// `entity_id = ANY($1::uuid[])` statement for batched UUID lookups.
    ///
    /// - Postgres: `true`. One prepared statement handles any batch
    ///   size because the whole array binds to a single parameter.
    /// - SQLite: `false`. `IN (...)` requires one placeholder per
    ///   value, so the planner emits a `__UUID_LIST__` template that
    ///   the store adapter expands at bind time.
    ///
    /// Default is `true` to preserve the v0.3.1 Postgres shape for any
    /// downstream dialect that does not override it.
    fn supports_uuid_array_any(&self) -> bool {
        true
    }

    /// Build an `entity_id IN (…)` expression for a dynamically-sized
    /// UUID list.
    ///
    /// The planner supplies the column reference and a slice of
    /// parameter placeholders (one per UUID); the dialect renders the
    /// `IN (p1, p2, …)` fragment. On Postgres this could also be
    /// `entity_id = ANY($1::uuid[])`, but the `IN` form works on both
    /// dialects and keeps the planner backend-agnostic.
    fn in_uuid_list(&self, column: &str, placeholders: &[String]) -> String;

    // ── Full-text search ────────────────────────────────────────────

    /// Render a full-text match expression of the form
    /// `<text-column> MATCHES <query-param>`.
    ///
    /// - Postgres: `to_tsvector('english', col #>> '{}') @@ plainto_tsquery('english', <param>)`
    /// - SQLite: approximated as `col LIKE '%' || <param> || '%'`.
    ///   The follow-up sprint will wire this through SQLite FTS5.
    fn fulltext_match(&self, alias: &str, query_param: &str) -> String;

    // ── Vector search (pgvector) ────────────────────────────────────

    /// Render a `vector` literal suitable for the store.
    ///
    /// - Postgres: `'[0.1,0.2,…]'::vector`
    /// - SQLite: unsupported — returns a sentinel that the planner
    ///   checks via [`Self::supports_vector`] before emitting.
    fn vector_literal(&self, values: &[f32]) -> String;

    /// Render the cosine-distance operator between an embedding column
    /// and a vector literal.
    ///
    /// - Postgres: `<alias>.embedding <=> <literal>`
    /// - SQLite: unsupported — `supports_vector` returns `false`.
    fn cosine_distance(&self, alias: &str, literal: &str) -> String;

    /// Whether this dialect supports native vector similarity search.
    fn supports_vector(&self) -> bool;

    // ── Misc ────────────────────────────────────────────────────────

    /// Render the `WITH RECURSIVE` keyword.
    ///
    /// Both Postgres and SQLite accept `WITH RECURSIVE`, so the
    /// default implementation returns `"WITH RECURSIVE"`. Defined on
    /// the trait so downstream backends (DuckDB, MySQL 8, etc.) can
    /// override if needed.
    fn recursive_cte_keyword(&self) -> &'static str {
        "WITH RECURSIVE"
    }

    /// Render the "current timestamp" expression for `CURRENT_TIMESTAMP`
    /// use in insert / update SQL paths.
    ///
    /// - Postgres: `NOW()`
    /// - SQLite: `datetime('now')`
    fn now_expr(&self) -> &'static str;

    // ── v0.3.2.1 — DarshanQL statement-type capability gates ────────
    //
    // The DarshanQL executor (packages/server/src/query/darshql/executor.rs)
    // ships with several statement types whose Pg implementation reaches
    // for features SQLite does not have today (recursive CTEs over JSONB
    // edges, schema DDL stored as triples with Pg-flavoured updates,
    // hybrid full-text + vector search). Until v0.3.3 lands a portable
    // form for each, the executor checks these capability methods at
    // dispatch time and refuses with a clear InvalidQuery error on
    // dialects that don't support them.
    //
    // Default is `true` so any new dialect inherits the v0.3.1 Postgres
    // shape and only needs to override the methods it cannot honour.

    /// Whether this dialect supports DDL statements emitted by the
    /// DarshanQL executor (`DEFINE TABLE`, `DEFINE FIELD`).
    ///
    /// The current Pg implementation persists schema as triples with
    /// `:schema/*` attributes and uses Pg-specific UPDATE forms. SQLite
    /// can replicate the storage but the executor's SQL has not been
    /// translated yet; v0.3.3 carries the rewrite.
    fn supports_ddl(&self) -> bool {
        true
    }

    /// Whether this dialect supports graph traversal queries
    /// (`->edge`, `<-edge`) emitted by the DarshanQL executor.
    ///
    /// The current implementation walks `:edge/in` / `:edge/out`
    /// triples via per-step Pg subqueries that depend on
    /// `to_jsonb` / `::uuid` casts. The portable variant lands in
    /// v0.3.3 once the planner emits a dialect-aware traversal plan.
    fn supports_graph_traversal(&self) -> bool {
        true
    }

    /// Whether this dialect supports hybrid (text + vector) search.
    ///
    /// Implies `supports_vector()`. Default reuses the vector capability
    /// so any dialect adding vectors automatically opts in.
    fn supports_hybrid_search(&self) -> bool {
        self.supports_vector()
    }
}

// ── PgDialect ───────────────────────────────────────────────────────

/// PostgreSQL dialect. Emits v0.3.1's exact SQL so the refactor is
/// a pure behaviour-preserving extraction.
#[derive(Debug, Clone, Copy, Default)]
pub struct PgDialect;

impl SqlDialect for PgDialect {
    fn name(&self) -> &'static str {
        "postgres"
    }

    fn placeholder(&self, idx: u32) -> String {
        format!("${idx}")
    }

    fn jsonb_param(&self, idx: u32, kind: ParamKind) -> String {
        match kind {
            ParamKind::Text => format!("to_jsonb(${idx}::text)"),
            ParamKind::Json => format!("${idx}::jsonb"),
        }
    }

    fn compare_triple_value(&self, alias: &str, op: &str, param: &str) -> String {
        format!("{alias}.value {op} {param}")
    }

    fn jsonb_contains(&self, alias: &str, param: &str) -> String {
        format!("{alias}.value @> {param}")
    }

    fn text_ilike(&self, alias: &str, param: &str) -> String {
        format!("{alias}.value #>> '{{}}' ILIKE {param}")
    }

    fn uuid_cast(&self, param: &str) -> String {
        format!("{param}::uuid")
    }

    fn uuid_array_cast(&self, param: &str) -> String {
        format!("{param}::uuid[]")
    }

    fn in_uuid_list(&self, column: &str, placeholders: &[String]) -> String {
        // On Postgres, IN() is equally valid and matches SQLite; we
        // keep a single code path in the planner for that reason.
        let mut s = String::with_capacity(16 + placeholders.len() * 4);
        s.push_str(column);
        s.push_str(" IN (");
        for (i, p) in placeholders.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            s.push_str(p);
        }
        s.push(')');
        s
    }

    fn fulltext_match(&self, alias: &str, query_param: &str) -> String {
        format!(
            "to_tsvector('english', {alias}.value #>> '{{}}') @@ plainto_tsquery('english', {query_param})"
        )
    }

    fn vector_literal(&self, values: &[f32]) -> String {
        let mut s = String::with_capacity(values.len() * 8 + 16);
        s.push('\'');
        s.push('[');
        for (i, v) in values.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            // Reuse std's f32 Display; matches v0.3.1 exactly.
            let _ = write!(s, "{v}");
        }
        s.push(']');
        s.push_str("'::vector");
        s
    }

    fn cosine_distance(&self, alias: &str, literal: &str) -> String {
        format!("{alias}.embedding <=> {literal}")
    }

    fn supports_vector(&self) -> bool {
        true
    }

    fn now_expr(&self) -> &'static str {
        "NOW()"
    }
}

// ── SqliteDialect ───────────────────────────────────────────────────

/// SQLite dialect. Emits portable SQL that works against a stock
/// SQLite build with the `json1` extension (which is bundled by
/// default in every supported rusqlite/sqlx-sqlite version).
#[derive(Debug, Clone, Copy, Default)]
pub struct SqliteDialect;

impl SqlDialect for SqliteDialect {
    fn name(&self) -> &'static str {
        "sqlite"
    }

    fn placeholder(&self, idx: u32) -> String {
        format!("?{idx}")
    }

    fn jsonb_param(&self, idx: u32, kind: ParamKind) -> String {
        // The SQLite triple store stores `value` as TEXT (JSON-encoded).
        // For string-typed comparisons we wrap the bound text in
        // json_quote() so the equality check matches a JSON-encoded
        // string on disk. Non-string JSON values are bound directly
        // as pre-encoded JSON text and compared literally.
        match kind {
            ParamKind::Text => format!("json_quote(?{idx})"),
            ParamKind::Json => format!("?{idx}"),
        }
    }

    fn compare_triple_value(&self, alias: &str, op: &str, param: &str) -> String {
        format!("{alias}.value {op} {param}")
    }

    /// Unsound fallback — callers MUST check
    /// [`SqlDialect::supports_jsonb_contains`] first.
    ///
    /// `instr(value, param) > 0` is substring match on serialized JSON
    /// text. It fails on scalar prefix collision (`instr('123','12') > 0`
    /// is true but `12 @> 123` is false) and on key reordering inside
    /// JSON objects. The planner gates `WhereOp::Contains` via
    /// `supports_jsonb_contains` and refuses SQLite; this method
    /// remains so a surprise call site does not panic, but any output
    /// it produces is wrong.
    fn jsonb_contains(&self, alias: &str, param: &str) -> String {
        format!("instr({alias}.value, {param}) > 0")
    }

    fn supports_jsonb_contains(&self) -> bool {
        false
    }

    fn text_ilike(&self, alias: &str, param: &str) -> String {
        // SQLite's LIKE is ASCII-case-insensitive by default, and the
        // column stores JSON text directly, so no #>> unwrap is needed.
        format!("{alias}.value LIKE {param}")
    }

    fn uuid_cast(&self, param: &str) -> String {
        // UUIDs are stored as TEXT. No cast required.
        param.to_string()
    }

    fn uuid_array_cast(&self, param: &str) -> String {
        // SQLite has no array type; this path should go through
        // `in_uuid_list`. We still return the raw placeholder so that
        // any lingering callers don't panic on a todo!().
        param.to_string()
    }

    fn supports_uuid_array_any(&self) -> bool {
        false
    }

    fn in_uuid_list(&self, column: &str, placeholders: &[String]) -> String {
        let mut s = String::with_capacity(16 + placeholders.len() * 4);
        s.push_str(column);
        s.push_str(" IN (");
        for (i, p) in placeholders.iter().enumerate() {
            if i > 0 {
                s.push_str(", ");
            }
            s.push_str(p);
        }
        s.push(')');
        s
    }

    fn fulltext_match(&self, alias: &str, query_param: &str) -> String {
        // Placeholder: a substring LIKE that is correct but not
        // ranked. FTS5 integration tracked in docs/SQL_DIALECTS.md.
        format!("{alias}.value LIKE '%' || {query_param} || '%'")
    }

    fn vector_literal(&self, _values: &[f32]) -> String {
        // Intentionally non-SQL. The planner must gate on
        // `supports_vector()` before ever calling this; returning
        // a syntactically-valid NULL (as v0.3.2 originally did)
        // silently produced zero-row results when a caller forgot
        // the gate. The sentinel below fails at statement prepare
        // time on any SQL engine, making a missed gate impossible
        // to miss.
        "__SQLITE_VECTOR_UNSUPPORTED__".to_string()
    }

    fn cosine_distance(&self, _alias: &str, _literal: &str) -> String {
        // See `vector_literal` — non-SQL prepare-time sentinel.
        "__SQLITE_COSINE_DISTANCE_UNSUPPORTED__".to_string()
    }

    fn supports_vector(&self) -> bool {
        false
    }

    fn now_expr(&self) -> &'static str {
        "datetime('now')"
    }

    // v0.3.2.1 — explicit refusal for the DarshanQL executor statement
    // types that still bake Postgres-only SQL. These return false on
    // SQLite so the executor's dispatch layer raises InvalidQuery
    // instead of attempting to run Pg syntax against rusqlite.
    fn supports_ddl(&self) -> bool {
        false
    }

    fn supports_graph_traversal(&self) -> bool {
        false
    }

    fn supports_hybrid_search(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Placeholder + parameter rendering ──────────────────────────

    #[test]
    fn pg_placeholder_is_dollar() {
        assert_eq!(PgDialect.placeholder(1), "$1");
        assert_eq!(PgDialect.placeholder(42), "$42");
    }

    #[test]
    fn sqlite_placeholder_is_question() {
        assert_eq!(SqliteDialect.placeholder(1), "?1");
        assert_eq!(SqliteDialect.placeholder(42), "?42");
    }

    #[test]
    fn pg_jsonb_param_wraps_text() {
        assert_eq!(
            PgDialect.jsonb_param(3, ParamKind::Text),
            "to_jsonb($3::text)"
        );
        assert_eq!(PgDialect.jsonb_param(3, ParamKind::Json), "$3::jsonb");
    }

    #[test]
    fn sqlite_jsonb_param_uses_json_quote_for_text() {
        assert_eq!(
            SqliteDialect.jsonb_param(3, ParamKind::Text),
            "json_quote(?3)"
        );
        assert_eq!(SqliteDialect.jsonb_param(3, ParamKind::Json), "?3");
    }

    // ── Value comparison ───────────────────────────────────────────

    #[test]
    fn pg_compare_matches_v031() {
        assert_eq!(
            PgDialect.compare_triple_value("tw0", "=", "to_jsonb($2::text)"),
            "tw0.value = to_jsonb($2::text)"
        );
    }

    #[test]
    fn sqlite_compare_uses_same_operator_syntax() {
        assert_eq!(
            SqliteDialect.compare_triple_value("tw0", "=", "json_quote(?2)"),
            "tw0.value = json_quote(?2)"
        );
    }

    #[test]
    fn pg_jsonb_contains_is_at_arrow() {
        assert_eq!(
            PgDialect.jsonb_contains("tw1", "$3::jsonb"),
            "tw1.value @> $3::jsonb"
        );
    }

    #[test]
    fn sqlite_jsonb_contains_falls_back_to_instr() {
        assert_eq!(
            SqliteDialect.jsonb_contains("tw1", "?3"),
            "instr(tw1.value, ?3) > 0"
        );
    }

    #[test]
    fn pg_text_ilike_uses_json_unwrap() {
        assert_eq!(
            PgDialect.text_ilike("tw0", "$2"),
            "tw0.value #>> '{}' ILIKE $2"
        );
    }

    #[test]
    fn sqlite_text_ilike_is_plain_like() {
        assert_eq!(SqliteDialect.text_ilike("tw0", "?2"), "tw0.value LIKE ?2");
    }

    // ── UUID casts ─────────────────────────────────────────────────

    #[test]
    fn pg_uuid_casts_match_v031() {
        assert_eq!(PgDialect.uuid_cast("$1"), "$1::uuid");
        assert_eq!(PgDialect.uuid_array_cast("$1"), "$1::uuid[]");
    }

    #[test]
    fn sqlite_uuid_casts_are_pass_through() {
        assert_eq!(SqliteDialect.uuid_cast("?1"), "?1");
        assert_eq!(SqliteDialect.uuid_array_cast("?1"), "?1");
    }

    #[test]
    fn both_dialects_build_in_uuid_list() {
        let ps = vec!["$1".to_string(), "$2".to_string(), "$3".to_string()];
        assert_eq!(
            PgDialect.in_uuid_list("entity_id", &ps),
            "entity_id IN ($1, $2, $3)"
        );
        let qs = vec!["?1".to_string(), "?2".to_string()];
        assert_eq!(
            SqliteDialect.in_uuid_list("entity_id", &qs),
            "entity_id IN (?1, ?2)"
        );
    }

    // ── Full-text ──────────────────────────────────────────────────

    #[test]
    fn pg_fulltext_uses_tsvector() {
        let sql = PgDialect.fulltext_match("t_search", "$2");
        assert!(sql.contains("to_tsvector('english'"));
        assert!(sql.contains("plainto_tsquery('english', $2)"));
        assert!(sql.contains("#>> '{}'"));
        assert!(sql.contains("@@"));
    }

    #[test]
    fn sqlite_fulltext_falls_back_to_like() {
        let sql = SqliteDialect.fulltext_match("t_search", "?2");
        assert_eq!(sql, "t_search.value LIKE '%' || ?2 || '%'");
    }

    // ── Vector ─────────────────────────────────────────────────────

    #[test]
    fn pg_vector_literal_matches_v031() {
        let lit = PgDialect.vector_literal(&[0.1, 0.2, 0.3]);
        assert_eq!(lit, "'[0.1,0.2,0.3]'::vector");
    }

    #[test]
    fn pg_vector_empty_literal() {
        assert_eq!(PgDialect.vector_literal(&[]), "'[]'::vector");
    }

    #[test]
    fn pg_cosine_distance_uses_arrow_eq() {
        let lit = PgDialect.vector_literal(&[0.5]);
        assert_eq!(
            PgDialect.cosine_distance("t_emb", &lit),
            "t_emb.embedding <=> '[0.5]'::vector"
        );
    }

    #[test]
    fn sqlite_does_not_support_vector() {
        assert!(!SqliteDialect.supports_vector());
        assert!(PgDialect.supports_vector());
    }

    #[test]
    fn sqlite_vector_sentinels_are_non_sql() {
        // m-3: any caller that forgets the supports_vector() gate
        // must see a statement-prepare failure, not a silent NULL.
        // The sentinels below are intentionally non-SQL — the test
        // pins the exact token so a future refactor can't
        // accidentally drift them back to a valid NULL expression.
        let lit = SqliteDialect.vector_literal(&[0.1, 0.2]);
        assert_eq!(lit, "__SQLITE_VECTOR_UNSUPPORTED__");
        assert!(!lit.contains("NULL"), "sentinel must not contain NULL");

        let cos = SqliteDialect.cosine_distance("t_emb", &lit);
        assert_eq!(cos, "__SQLITE_COSINE_DISTANCE_UNSUPPORTED__");
        assert!(!cos.contains("NULL"), "sentinel must not contain NULL");
    }

    // ── Misc ───────────────────────────────────────────────────────

    #[test]
    fn recursive_cte_default_is_same() {
        assert_eq!(PgDialect.recursive_cte_keyword(), "WITH RECURSIVE");
        assert_eq!(SqliteDialect.recursive_cte_keyword(), "WITH RECURSIVE");
    }

    #[test]
    fn now_expr_differs() {
        assert_eq!(PgDialect.now_expr(), "NOW()");
        assert_eq!(SqliteDialect.now_expr(), "datetime('now')");
    }

    // ── Dialect names ──────────────────────────────────────────────

    #[test]
    fn names_are_stable_identifiers() {
        assert_eq!(PgDialect.name(), "postgres");
        assert_eq!(SqliteDialect.name(), "sqlite");
    }

    // ── v0.3.2.1 capability gates ──────────────────────────────────

    #[test]
    fn pg_supports_all_executor_statement_types() {
        assert!(PgDialect.supports_ddl());
        assert!(PgDialect.supports_graph_traversal());
        assert!(PgDialect.supports_hybrid_search());
    }

    #[test]
    fn sqlite_refuses_executor_pg_only_paths() {
        assert!(!SqliteDialect.supports_ddl());
        assert!(!SqliteDialect.supports_graph_traversal());
        assert!(!SqliteDialect.supports_hybrid_search());
    }

    #[test]
    fn supports_hybrid_search_implies_vector_by_default() {
        // Default impl on the trait wires hybrid through vector;
        // overriding either independently is allowed.
        assert_eq!(
            PgDialect.supports_hybrid_search(),
            PgDialect.supports_vector()
        );
        assert_eq!(
            SqliteDialect.supports_hybrid_search(),
            SqliteDialect.supports_vector()
        );
    }
}
