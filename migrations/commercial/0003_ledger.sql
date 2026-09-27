-- 0003_ledger.sql
-- Materialized wallets, reservations, and the append-only ledger.
--
-- Ledger direction is carried by entry_type, never by a signed amount:
--   credit | debit | refund_credit | compensating_credit
-- Every entry stores its tariff version and rounding rule. Releasing an
-- unsettled reservation writes NO ledger row: it only decreases the hold.
--
-- ledger_entries is append-only. UPDATE, DELETE and TRUNCATE all raise.

CREATE TABLE wallets (
    id                      UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id               UUID NOT NULL,
    currency                CHAR(3) NOT NULL DEFAULT 'USD'
                            CHECK (currency = 'USD'),
    available_micro_usd     NUMERIC(39,0) NOT NULL DEFAULT 0
        CHECK (available_micro_usd >= 0
               AND available_micro_usd < 340282366920938463463374607431768211456),
    reserved_micro_usd      NUMERIC(39,0) NOT NULL DEFAULT 0
        CHECK (reserved_micro_usd >= 0
               AND reserved_micro_usd < 340282366920938463463374607431768211456),
    credits_total_micro_usd NUMERIC(39,0) NOT NULL DEFAULT 0
        CHECK (credits_total_micro_usd >= 0
               AND credits_total_micro_usd < 340282366920938463463374607431768211456),
    debits_total_micro_usd  NUMERIC(39,0) NOT NULL DEFAULT 0
        CHECK (debits_total_micro_usd >= 0
               AND debits_total_micro_usd < 340282366920938463463374607431768211456),
    version                 BIGINT NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT wallets_tenant_fk FOREIGN KEY (tenant_id)
        REFERENCES tenants (tenant_id) ON DELETE RESTRICT,
    CONSTRAINT wallets_tenant_currency_unique UNIQUE (tenant_id, currency),
    CONSTRAINT wallets_tenant_id_unique UNIQUE (tenant_id, id)
);

COMMENT ON TABLE wallets IS
    'Transactional materialized balance per tenant/currency. The append-only ledger is the source of truth.';

CREATE TABLE reservations (
    id                 UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id          UUID NOT NULL,
    key_id             UUID NOT NULL,
    endpoint_name      TEXT NOT NULL
                       CHECK (char_length(btrim(endpoint_name)) BETWEEN 1 AND 200),
    -- Hashed at rest: the column stores a lowercase SHA-256 hex digest of the
    -- Idempotency-Key, never the key value. The CHECK makes a raw key
    -- unstorable, not merely discouraged.
    idempotency_key    TEXT NOT NULL CHECK (idempotency_key ~ '^[0-9a-f]{64}$'),
    request_id         TEXT NOT NULL CHECK (char_length(request_id) BETWEEN 1 AND 200),
    currency           CHAR(3) NOT NULL DEFAULT 'USD'
                       CHECK (currency = 'USD'),
    amount_micro_usd   NUMERIC(39,0) NOT NULL
        CHECK (amount_micro_usd > 0
               AND amount_micro_usd < 340282366920938463463374607431768211456),
    settled_micro_usd  NUMERIC(39,0) NOT NULL DEFAULT 0
        CHECK (settled_micro_usd >= 0
               AND settled_micro_usd < 340282366920938463463374607431768211456),
    state              TEXT NOT NULL DEFAULT 'reserved'
                       CHECK (state IN ('reserved', 'attempting', 'settled', 'released', 'expired')),
    operation_state    TEXT NOT NULL DEFAULT 'ok'
                       CHECK (operation_state IN ('ok', 'unknown_outcome')),
    tariff_version_id  UUID NOT NULL,
    rounding           TEXT NOT NULL DEFAULT 'up' CHECK (rounding = 'up'),
    lease_id           UUID NOT NULL DEFAULT gen_random_uuid(),
    fencing_token      BIGINT NOT NULL DEFAULT 0 CHECK (fencing_token >= 0),
    holder_id          TEXT NOT NULL DEFAULT ''
                       CHECK (char_length(holder_id) <= 200),
    lease_expires_at   TIMESTAMPTZ NOT NULL DEFAULT (now() + INTERVAL '30 seconds'),
    lease_heartbeat_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    version            BIGINT NOT NULL DEFAULT 1 CHECK (version >= 1),
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at         TIMESTAMPTZ NOT NULL,
    terminal_at        TIMESTAMPTZ,
    CONSTRAINT reservations_tenant_fk FOREIGN KEY (tenant_id)
        REFERENCES tenants (tenant_id) ON DELETE RESTRICT,
    CONSTRAINT reservations_key_fk FOREIGN KEY (tenant_id, key_id)
        REFERENCES tenant_keys (tenant_id, id) ON DELETE RESTRICT,
    CONSTRAINT reservations_tariff_fk FOREIGN KEY (tenant_id, tariff_version_id)
        REFERENCES tariff_versions (tenant_id, id) ON DELETE RESTRICT,
    CONSTRAINT reservations_tenant_id_unique UNIQUE (tenant_id, id),
    CONSTRAINT reservations_idempotency_unique
        UNIQUE (tenant_id, key_id, endpoint_name, idempotency_key),
    CONSTRAINT reservations_settled_le_reserved
        CHECK (settled_micro_usd <= amount_micro_usd),
    CONSTRAINT reservations_unknown_outcome_held
        CHECK (operation_state = 'ok' OR state = 'attempting'),
    CONSTRAINT reservations_terminal_timestamp
        CHECK ((state IN ('settled', 'released', 'expired')) = (terminal_at IS NOT NULL)),
    CONSTRAINT reservations_lease_expiry_after_creation
        CHECK (lease_expires_at > created_at - INTERVAL '1 second')
);

COMMENT ON TABLE reservations IS
    'One reservation per (tenant_id, key_id, endpoint_name, hashed idempotency key). State machine: reserved -> attempting -> settled|released, reserved -> expired before send.';

COMMENT ON COLUMN reservations.idempotency_key IS
    'Lowercase SHA-256 hex digest of the Idempotency-Key. Raw key material is not storable here.';

-- Canonical reservation state machine. Terminal rows are immutable, the held
-- amount can never change, and only these edges are legal:
--   reserved -> attempting | expired      (expired only before provider send)
--   attempting -> settled | released
-- Because terminal states are never a legal `from` state and terminal rows are
-- immutable, exactly one terminal transition can ever commit.
CREATE OR REPLACE FUNCTION commercial_enforce_reservation_transition()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF OLD.state IN ('settled', 'released', 'expired') THEN
        IF NEW IS DISTINCT FROM OLD THEN
            RAISE EXCEPTION
                'terminal_reservation_immutable: % is terminal and cannot be mutated', OLD.state
                USING ERRCODE = 'check_violation';
        END IF;
        RETURN NEW;
    END IF;

    IF NEW.tenant_id IS DISTINCT FROM OLD.tenant_id
        OR NEW.key_id IS DISTINCT FROM OLD.key_id
        OR NEW.endpoint_name IS DISTINCT FROM OLD.endpoint_name
        OR NEW.idempotency_key IS DISTINCT FROM OLD.idempotency_key
        OR NEW.currency IS DISTINCT FROM OLD.currency
        OR NEW.tariff_version_id IS DISTINCT FROM OLD.tariff_version_id
        OR NEW.request_id IS DISTINCT FROM OLD.request_id
        OR NEW.created_at IS DISTINCT FROM OLD.created_at
        OR NEW.expires_at IS DISTINCT FROM OLD.expires_at
    THEN
        RAISE EXCEPTION
            'reservation_identity_immutable: reservation identity columns cannot change'
            USING ERRCODE = 'check_violation';
    END IF;

    IF NEW.amount_micro_usd IS DISTINCT FROM OLD.amount_micro_usd THEN
        RAISE EXCEPTION
            'reservation_amount_immutable: the held amount cannot change'
            USING ERRCODE = 'check_violation';
    END IF;

    IF NEW.settled_micro_usd < OLD.settled_micro_usd THEN
        RAISE EXCEPTION
            'reservation_settled_regression: settled amount cannot decrease'
            USING ERRCODE = 'check_violation';
    END IF;

    IF NEW.state = OLD.state THEN
        NEW.version := OLD.version + 1;
        NEW.updated_at := now();
        RETURN NEW;
    END IF;

    IF NOT (
        (OLD.state = 'reserved' AND NEW.state IN ('attempting', 'expired'))
        OR (OLD.state = 'attempting' AND NEW.state IN ('settled', 'released'))
    ) THEN
        RAISE EXCEPTION
            'illegal_reservation_transition: % -> %', OLD.state, NEW.state
            USING ERRCODE = 'check_violation';
    END IF;

    IF NEW.state IN ('settled', 'released', 'expired') THEN
        NEW.terminal_at := COALESCE(NEW.terminal_at, now());
    END IF;

    NEW.version := OLD.version + 1;
    NEW.updated_at := now();
    RETURN NEW;
END;
$$;

CREATE TRIGGER reservations_state_machine
    BEFORE UPDATE ON reservations
    FOR EACH ROW EXECUTE FUNCTION commercial_enforce_reservation_transition();

-- A reservation is born `reserved` with no settlement and no terminal stamp.
-- A caller cannot insert itself directly into a terminal state.
CREATE OR REPLACE FUNCTION commercial_enforce_reservation_insert()
RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
    IF NEW.state <> 'reserved' THEN
        RAISE EXCEPTION
            'reservation_must_start_reserved: got %', NEW.state
            USING ERRCODE = 'check_violation';
    END IF;
    IF NEW.settled_micro_usd <> 0 THEN
        RAISE EXCEPTION
            'reservation_must_start_unsettled: got %', NEW.settled_micro_usd
            USING ERRCODE = 'check_violation';
    END IF;
    IF NEW.terminal_at IS NOT NULL THEN
        RAISE EXCEPTION
            'reservation_must_start_unterminated'
            USING ERRCODE = 'check_violation';
    END IF;
    IF NEW.operation_state <> 'ok' THEN
        RAISE EXCEPTION
            'reservation_must_start_ok: unknown_outcome requires a committed attempt'
            USING ERRCODE = 'check_violation';
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER reservations_initial_state
    BEFORE INSERT ON reservations
    FOR EACH ROW EXECUTE FUNCTION commercial_enforce_reservation_insert();

CREATE TABLE ledger_entries (
    id                   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id            UUID NOT NULL,
    wallet_id            UUID NOT NULL,
    currency             CHAR(3) NOT NULL DEFAULT 'USD'
                         CHECK (currency = 'USD'),
    entry_type           TEXT NOT NULL
                         CHECK (entry_type IN ('credit', 'debit', 'refund_credit', 'compensating_credit')),
    amount_micro_usd     NUMERIC(39,0) NOT NULL
        CHECK (amount_micro_usd > 0
               AND amount_micro_usd < 340282366920938463463374607431768211456),
    reservation_id       UUID,
    tariff_version_id    UUID,
    rounding             TEXT NOT NULL DEFAULT 'up' CHECK (rounding = 'up'),
    request_id           TEXT NOT NULL CHECK (char_length(request_id) BETWEEN 1 AND 200),
    idempotency_key_hash TEXT
                         CHECK (idempotency_key_hash IS NULL
                                OR idempotency_key_hash ~ '^[0-9a-f]{64}$'),
    actor_id             TEXT NOT NULL CHECK (char_length(actor_id) BETWEEN 1 AND 200),
    reason               TEXT NOT NULL CHECK (char_length(reason) BETWEEN 1 AND 500),
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT ledger_entries_wallet_fk FOREIGN KEY (tenant_id, wallet_id)
        REFERENCES wallets (tenant_id, id) ON DELETE RESTRICT,
    CONSTRAINT ledger_entries_reservation_fk FOREIGN KEY (tenant_id, reservation_id)
        REFERENCES reservations (tenant_id, id) ON DELETE RESTRICT,
    CONSTRAINT ledger_entries_tariff_fk FOREIGN KEY (tenant_id, tariff_version_id)
        REFERENCES tariff_versions (tenant_id, id) ON DELETE RESTRICT,
    CONSTRAINT ledger_entries_tenant_id_unique UNIQUE (tenant_id, id)
);

COMMENT ON TABLE ledger_entries IS
    'Append-only money ledger. Direction comes from entry_type; amounts are always positive.';

CREATE TRIGGER ledger_entries_append_only
    BEFORE UPDATE OR DELETE ON ledger_entries
    FOR EACH ROW EXECUTE FUNCTION commercial_reject_append_only_mutation();

CREATE TRIGGER ledger_entries_no_truncate
    BEFORE TRUNCATE ON ledger_entries
    FOR EACH STATEMENT EXECUTE FUNCTION commercial_reject_append_only_mutation();
