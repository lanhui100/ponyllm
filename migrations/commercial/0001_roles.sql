-- 0001_roles.sql
-- Commercial Stage 1 foundation: migration bookkeeping, least-privilege roles,
-- and shared guard functions.
--
-- Applied by `ponyllm-billing`'s forward-only runner, which wraps each file in
-- one transaction together with its bookkeeping row. Do NOT add BEGIN/COMMIT
-- here: a nested COMMIT would end the runner's transaction early.
--
-- There is no down migration. Rollback disables commercial ingress and reverts
-- code; it never rewrites applied schema or the append-only ledger.

CREATE TABLE IF NOT EXISTS commercial_schema_migrations (
    version    BIGINT PRIMARY KEY,
    filename   TEXT NOT NULL,
    checksum   TEXT NOT NULL CHECK (checksum ~ '^[0-9a-f]{64}$'),
    applied_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT commercial_schema_migrations_version_positive CHECK (version >= 1)
);

COMMENT ON TABLE commercial_schema_migrations IS
    'Forward-only commercial schema versions with SHA-256 file checksums. Not tenant-owned, so deliberately outside RLS.';

-- Tenant-scoped transactions run as this role. It is NOLOGIN (credentials are
-- injected by the operator), never a superuser, and explicitly NOT BYPASSRLS,
-- so ENABLE + FORCE ROW LEVEL SECURITY applies to every query it makes. There
-- is no client-controllable bypass flag anywhere in this schema.
DO $$
BEGIN
    IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'ponyllm_commercial_tenant') THEN
        CREATE ROLE ponyllm_commercial_tenant
            NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOBYPASSRLS;
    END IF;
END
$$;

ALTER ROLE ponyllm_commercial_tenant
    NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOINHERIT NOBYPASSRLS;

COMMENT ON ROLE ponyllm_commercial_tenant IS
    'Non-owner, non-BYPASSRLS role for tenant-scoped commercial transactions (Stage 1, RLS).';

-- Append-only guard used by the ledger and the audit log. Row-level triggers
-- reject UPDATE/DELETE; statement-level triggers reject TRUNCATE (which
-- row-level triggers never see).
CREATE OR REPLACE FUNCTION commercial_reject_append_only_mutation()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    RAISE EXCEPTION
        'append_only_violation: % on % is forbidden', TG_OP, TG_TABLE_NAME
        USING ERRCODE = 'feature_not_supported',
              HINT = 'the commercial ledger and commercial audit log are append-only';
END;
$$;

COMMENT ON FUNCTION commercial_reject_append_only_mutation() IS
    'Raises on any UPDATE/DELETE/TRUNCATE of an append-only commercial table.';
