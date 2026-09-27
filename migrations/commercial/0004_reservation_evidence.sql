-- 0004_reservation_evidence.sql
-- Per-reservation evidence: the state-transition log and provider attempts.
--
-- Both tables reference the reservation through the composite
-- (tenant_id, reservation_id) pair, so an attempt or transition can never be
-- attached to another tenant's reservation.

CREATE TABLE ledger_state_transitions (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL,
    reservation_id  UUID NOT NULL,
    sequence_no     BIGINT NOT NULL CHECK (sequence_no >= 1),
    from_state      TEXT NOT NULL
                    CHECK (from_state IN ('reserved', 'attempting', 'settled', 'released', 'expired')),
    to_state        TEXT NOT NULL
                    CHECK (to_state IN ('reserved', 'attempting', 'settled', 'released', 'expired')),
    operation_state TEXT NOT NULL
                    CHECK (operation_state IN ('ok', 'unknown_outcome')),
    fencing_token   BIGINT NOT NULL CHECK (fencing_token >= 0),
    actor_id        TEXT NOT NULL CHECK (char_length(actor_id) BETWEEN 1 AND 200),
    occurred_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT ledger_state_transitions_tenant_fk FOREIGN KEY (tenant_id)
        REFERENCES tenants (tenant_id) ON DELETE RESTRICT,
    CONSTRAINT ledger_state_transitions_reservation_fk FOREIGN KEY (tenant_id, reservation_id)
        REFERENCES reservations (tenant_id, id) ON DELETE RESTRICT,
    CONSTRAINT ledger_state_transitions_sequence_unique UNIQUE (reservation_id, sequence_no),
    CONSTRAINT ledger_state_transitions_tenant_id_unique UNIQUE (tenant_id, id),
    CONSTRAINT ledger_state_transitions_legal_edge CHECK (
        (from_state = 'reserved' AND to_state IN ('attempting', 'expired'))
        OR (from_state = 'attempting' AND to_state IN ('settled', 'released'))
    )
);

COMMENT ON TABLE ledger_state_transitions IS
    'Ordered, unique state-machine evidence per reservation. sequence_no >= 1 and unique per reservation.';

CREATE TABLE ledger_attempts (
    id                              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id                       UUID NOT NULL,
    reservation_id                  UUID NOT NULL,
    attempt_no                      INTEGER NOT NULL CHECK (attempt_no >= 1),
    request_id                      TEXT NOT NULL CHECK (char_length(request_id) BETWEEN 1 AND 200),
    provider_id                     TEXT NOT NULL CHECK (char_length(btrim(provider_id)) BETWEEN 1 AND 100),
    outcome                         TEXT NOT NULL
        CHECK (outcome IN ('started', 'succeeded', 'failed_retryable', 'failed_terminal', 'cancelled', 'unknown')),
    -- Recorded BEFORE any provider send: retry/failover is only legal when the
    -- provider offers a stable idempotency key.
    provider_idempotency_capability TEXT NOT NULL
        CHECK (provider_idempotency_capability IN ('stable', 'none', 'unknown')),
    provider_idempotency_key_hash   TEXT
        CHECK (provider_idempotency_key_hash IS NULL
               OR provider_idempotency_key_hash ~ '^[0-9a-f]{64}$'),
    lease_id                        UUID NOT NULL,
    fencing_token                   BIGINT NOT NULL CHECK (fencing_token >= 0),
    holder_id                       TEXT NOT NULL CHECK (char_length(holder_id) BETWEEN 1 AND 200),
    lease_expires_at                TIMESTAMPTZ NOT NULL,
    lease_heartbeat_at              TIMESTAMPTZ NOT NULL,
    started_at                      TIMESTAMPTZ NOT NULL DEFAULT now(),
    ended_at                        TIMESTAMPTZ,
    created_at                      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT ledger_attempts_tenant_fk FOREIGN KEY (tenant_id)
        REFERENCES tenants (tenant_id) ON DELETE RESTRICT,
    CONSTRAINT ledger_attempts_reservation_fk FOREIGN KEY (tenant_id, reservation_id)
        REFERENCES reservations (tenant_id, id) ON DELETE RESTRICT,
    CONSTRAINT ledger_attempts_attempt_unique UNIQUE (reservation_id, attempt_no),
    CONSTRAINT ledger_attempts_tenant_id_unique UNIQUE (tenant_id, id),
    CONSTRAINT ledger_attempts_outcome_timestamp
        CHECK ((outcome = 'started') = (ended_at IS NULL)),
    CONSTRAINT ledger_attempts_capability_key
        CHECK (provider_idempotency_capability = 'stable'
               OR provider_idempotency_key_hash IS NULL),
    CONSTRAINT ledger_attempts_lease_window
        CHECK (lease_expires_at > lease_heartbeat_at)
);

COMMENT ON TABLE ledger_attempts IS
    'Immutable attempt evidence. Attempt ordinals are unique per reservation and never reused.';

COMMENT ON COLUMN ledger_attempts.provider_idempotency_capability IS
    'Provider idempotency capability recorded before send: stable | none | unknown.';

-- At most one successful attempt per reservation, enforced by the database
-- rather than by application convention.
CREATE UNIQUE INDEX ledger_attempts_one_succeeded_per_reservation
    ON ledger_attempts (reservation_id)
    WHERE outcome = 'succeeded';

-- Evidence tables are append-only. UPDATE, DELETE and TRUNCATE are strictly forbidden.
CREATE TRIGGER commercial_ledger_state_transitions_append_only
    BEFORE UPDATE OR DELETE ON ledger_state_transitions
    FOR EACH ROW
    EXECUTE FUNCTION commercial_reject_append_only_mutation();

CREATE TRIGGER commercial_ledger_state_transitions_truncate_guard
    BEFORE TRUNCATE ON ledger_state_transitions
    FOR EACH STATEMENT
    EXECUTE FUNCTION commercial_reject_append_only_mutation();

CREATE TRIGGER commercial_ledger_attempts_append_only
    BEFORE UPDATE OR DELETE ON ledger_attempts
    FOR EACH ROW
    EXECUTE FUNCTION commercial_reject_append_only_mutation();

CREATE TRIGGER commercial_ledger_attempts_truncate_guard
    BEFORE TRUNCATE ON ledger_attempts
    FOR EACH STATEMENT
    EXECUTE FUNCTION commercial_reject_append_only_mutation();

