-- DarshJDB — migration by Darshankumar Joshi (github.com/darshjme)
-- Enforce ON DELETE CASCADE on sessions.user_id and admin_audit_log.actor_user_id
-- so a GDPR Article 17 erasure request can be executed via a single
-- transactional DELETE FROM users WHERE id = $1 without orphaning sessions
-- or aborting on the foreign-key constraint.
--
-- Strategy: drop the existing FK (without knowing its generated name — we
-- look it up dynamically in pg_constraint) and recreate with ON DELETE CASCADE
-- for sessions, and ON DELETE SET NULL for admin_audit_log (preserve forensic
-- history while still fulfilling erasure).

DO $mig$
DECLARE
    fk_name TEXT;
BEGIN
    -- sessions.user_id -> users.id ON DELETE CASCADE
    SELECT conname INTO fk_name
    FROM pg_constraint
    WHERE conrelid = 'public.sessions'::regclass
      AND contype = 'f'
      AND 'user_id' = ANY (
          SELECT attname FROM pg_attribute
          WHERE attrelid = 'public.sessions'::regclass
            AND attnum = ANY(conkey)
      );

    IF fk_name IS NOT NULL THEN
        EXECUTE format('ALTER TABLE sessions DROP CONSTRAINT %I', fk_name);
    END IF;

    ALTER TABLE sessions
        ADD CONSTRAINT sessions_user_id_fkey
            FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE;
EXCEPTION
    WHEN undefined_table THEN
        RAISE NOTICE 'sessions table missing; cascade migration no-op';
    WHEN undefined_column THEN
        RAISE NOTICE 'sessions.user_id missing; cascade migration no-op';
END
$mig$;

DO $mig$
DECLARE
    fk_name TEXT;
BEGIN
    -- admin_audit_log.actor_user_id -> users.id ON DELETE SET NULL
    SELECT conname INTO fk_name
    FROM pg_constraint
    WHERE conrelid = 'public.admin_audit_log'::regclass
      AND contype = 'f'
      AND 'actor_user_id' = ANY (
          SELECT attname FROM pg_attribute
          WHERE attrelid = 'public.admin_audit_log'::regclass
            AND attnum = ANY(conkey)
      );

    IF fk_name IS NOT NULL THEN
        EXECUTE format('ALTER TABLE admin_audit_log DROP CONSTRAINT %I', fk_name);
    END IF;

    ALTER TABLE admin_audit_log
        ADD CONSTRAINT admin_audit_log_actor_user_id_fkey
            FOREIGN KEY (actor_user_id) REFERENCES users(id) ON DELETE SET NULL;
EXCEPTION
    WHEN undefined_table THEN
        RAISE NOTICE 'admin_audit_log table missing; cascade migration no-op';
    WHEN undefined_column THEN
        RAISE NOTICE 'admin_audit_log.actor_user_id missing; cascade migration no-op';
END
$mig$;
