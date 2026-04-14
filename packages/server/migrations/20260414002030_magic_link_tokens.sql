-- DarshJDB — migration by Darshankumar Joshi (github.com/darshjme)
-- magic_link_tokens: passwordless sign-in tokens for the magic link auth provider.
--
-- Stores only a SHA-256 hash of the raw token so a database leak cannot be
-- replayed as a sign-in. Tokens are single-use and expire after 15 minutes.

CREATE TABLE IF NOT EXISTS magic_link_tokens (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id      UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash   TEXT        NOT NULL,
    email        TEXT        NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at   TIMESTAMPTZ NOT NULL DEFAULT (now() + INTERVAL '15 minutes'),
    used_at      TIMESTAMPTZ,
    ip_address   INET,
    CONSTRAINT unique_unused_token UNIQUE (token_hash)
);

-- Partial index for the happy-path lookup: unused tokens by hash.
CREATE INDEX IF NOT EXISTS idx_magic_link_tokens_hash
    ON magic_link_tokens (token_hash)
    WHERE used_at IS NULL;
