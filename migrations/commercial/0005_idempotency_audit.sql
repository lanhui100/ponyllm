-- 0005_idempotency_audit.sql
-- Idempotency records and the append-only commercial audit log.
--
-- Key material is stored hashed only: the CHECK constraints make a raw
-- idempotency key or a raw body unrepresentable. Retention is at least 180
-- days measured from the database clock.

CREATE TABLE commercial_idempotency (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id          UUID NOT NULL,
    key_id             UUID NOT NULL,
    endpoint_name      TEXT NOT NULL
                       CHECK (char_length(btrim(endpoint_name)) BETWEEN 1 AND 200),
    -- Lowercase SHA-256 hex of the Idempotency-Key value. The raw key is not
    -- storable in this schema.
    key_value_hash     TEXT NOT NULL CHECK (key_value_hash ~ '^[0-9a-f]{64}$'),
    -- Keyed, allowlisted request projection; never contains prompts or secrets.
    fingerprint        TEXT NOT NULL CHECK (fingerprint ~ '^[0-9a-f]{64}$'),
    state              TEXT NOT NULL DEFAULT 'in_progress'
                       CHECK (state IN ('in_progress', 'succeeded', 'failed', 'unknown_outcome')),
    reservation_id     UUID,
    response_status    INTEGER
                       CHECK (response_status IS NULL
                              OR (response_status BETWEEN 100 AND 599)),
    response_body_hash TEXT
                       CHECK (response_body_hash IS NULL
                              OR response_body_hash ~ '^[0-9a-f]{64}$'),
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    terminal_at        TIMESTAMPTZ,
    retention_until    TIMESTAMPTZ NOT NULL DEFAULT (now() + INTERVAL '180 days'),
    CONSTRAINT commercial_idempotency_tenant_fk FOREIGN KEY (tenant_id)
        REFERENCES tenants (tenant_id) ON DELETE RESTRICT,
    CONSTRAINT commercial_idempotency_key_fk FOREIGN KEY (tenant_id, key_id)
        REFERENCES tenant_keys (tenant_id, id) ON DELETE RESTRICT,
    CONSTRAINT commercial_idempotency_reservation_fk FOREIGN KEY (tenant_id, reservation_id)
        REFERENCES reservations (tenant_id, id) ON DELETE RESTRICT,
    CONSTRAINT commercial_idempotency_scope_unique
        UNIQUE (tenant_id, key_id, endpoint_name, key_value_hash),
    CONSTRAINT commercial_idempotency_tenant_id_unique UNIQUE (tenant_id, id),
    CONSTRAINT commercial_idempotency_retention_180_days
        CHECK (retention_until >= created_at + INTERVAL '180 days')
);

COMMENT ON TABLE commercial_idempotency IS
    'Idempotency scope is (tenant_id, key_id, endpoint_name, key_value_hash). Same key with a different fingerprint is a 409, never an overwrite.';

CREATE TABLE commercial_audit_log (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    UUID NOT NULL,
    actor_id     TEXT NOT NULL CHECK (char_length(actor_id) BETWEEN 1 AND 200),
    actor_role   TEXT NOT NULL
                 CHECK (actor_role IN ('tenant_admin', 'platform_admin', 'system', 'recovery')),
    tenant_scope TEXT NOT NULL CHECK (tenant_scope IN ('tenant', 'platform')),
    action       TEXT NOT NULL CHECK (char_length(btrim(action)) BETWEEN 1 AND 100),
    target_id    TEXT CHECK (target_id IS NULL OR char_length(target_id) <= 200),
    reason       TEXT NOT NULL CHECK (char_length(reason) BETWEEN 1 AND 500),
    before_hash  TEXT CHECK (before_hash IS NULL OR before_hash ~ '^[0-9a-f]{64}$'),
    after_hash   TEXT CHECK (after_hash IS NULL OR after_hash ~ '^[0-9a-f]{64}$'),
    request_id   TEXT NOT NULL CHECK (char_length(request_id) BETWEEN 1 AND 200),
    outcome      TEXT NOT NULL CHECK (outcome IN ('succeeded', 'denied', 'failed')),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT commercial_audit_log_tenant_fk FOREIGN KEY (tenant_id)
        REFERENCES tenants (tenant_id) ON DELETE RESTRICT,
    CONSTRAINT commercial_audit_log_tenant_id_unique UNIQUE (tenant_id, id)
);

COMMENT ON TABLE commercial_audit_log IS
    'Append-only admin audit trail: actor, tenant scope, action, target, reason, before/after hashes, request id, outcome.';

CREATE TRIGGER commercial_audit_log_append_only
    BEFORE UPDATE OR DELETE ON commercial_audit_log
    FOR EACH ROW EXECUTE FUNCTION commercial_reject_append_only_mutation();

CREATE TRIGGER commercial_audit_log_no_truncate
    BEFORE TRUNCATE ON commercial_audit_log
    FOR EACH STATEMENT EXECUTE FUNCTION commercial_reject_append_only_mutation();
