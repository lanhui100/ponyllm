-- 0002_tenants.sql
-- Tenant identity, tenant keys (hashed only), per-key model grants, customer
-- tariff versions.
--
-- Tenant isolation pattern used throughout this schema:
--   * every tenant-owned table carries a NOT NULL tenant_id;
--   * every tenant-owned table exposes a UNIQUE (tenant_id, id) key;
--   * child rows reference the composite (tenant_id, <parent_id>) pair, so a
--     row can never point at another tenant's parent row.
-- `tenants` uses tenant_id as its primary key, so it has no separate id column.
--
-- Money columns are NUMERIC(39,0) (integer micro-USD, 1 USD = 1_000_000) with
-- an explicit 2^128 exclusive upper bound, matching the Rust `u128` type.

CREATE TABLE tenants (
    tenant_id    UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    display_name TEXT NOT NULL CHECK (char_length(btrim(display_name)) BETWEEN 1 AND 200),
    status       TEXT NOT NULL DEFAULT 'active'
                 CHECK (status IN ('active', 'suspended', 'closed')),
    currency     CHAR(3) NOT NULL DEFAULT 'USD'
                 CHECK (currency = 'USD'),
    retention_days INTEGER NOT NULL DEFAULT 180 CHECK (retention_days >= 180),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

COMMENT ON TABLE tenants IS
    'Commercial tenants. tenant_id is the sole authority for tenant-scoped reads and mutations.';

CREATE TABLE tenant_keys (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id    UUID NOT NULL,
    key_id       TEXT NOT NULL CHECK (key_id ~ '^[A-Za-z0-9._-]{1,128}$'),
    key_hash     TEXT NOT NULL CHECK (key_hash ~ '^\$(argon2id|scrypt)\$'),
    kdf          TEXT NOT NULL CHECK (kdf IN ('argon2id', 'scrypt')),
    kdf_salt     TEXT NOT NULL CHECK (char_length(kdf_salt) BETWEEN 16 AND 512),
    label        TEXT NOT NULL DEFAULT '' CHECK (char_length(label) <= 200),
    status       TEXT NOT NULL DEFAULT 'active'
                 CHECK (status IN ('active', 'revoked', 'expired')),
    rotated_from UUID,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at   TIMESTAMPTZ,
    revoked_at   TIMESTAMPTZ,
    CONSTRAINT tenant_keys_tenant_fk FOREIGN KEY (tenant_id)
        REFERENCES tenants (tenant_id) ON DELETE RESTRICT,
    CONSTRAINT tenant_keys_tenant_id_unique UNIQUE (tenant_id, id),
    CONSTRAINT tenant_keys_key_id_unique UNIQUE (tenant_id, key_id),
    CONSTRAINT tenant_keys_hash_matches_kdf
        CHECK (key_hash LIKE '$' || kdf || '$%'),
    CONSTRAINT tenant_keys_revocation_consistent
        CHECK ((status = 'revoked') = (revoked_at IS NOT NULL)),
    CONSTRAINT tenant_keys_rotation_same_tenant FOREIGN KEY (tenant_id, rotated_from)
        REFERENCES tenant_keys (tenant_id, id) ON DELETE RESTRICT
);

COMMENT ON TABLE tenant_keys IS
    'Tenant credentials, stored only as a memory-hard PHC-encoded hash. No column can hold a raw key.';

CREATE TABLE tenant_model_grants (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id         UUID NOT NULL,
    key_id            UUID NOT NULL,
    model             TEXT NOT NULL CHECK (char_length(btrim(model)) BETWEEN 1 AND 200),
    provider          TEXT NOT NULL CHECK (char_length(btrim(provider)) BETWEEN 1 AND 100),
    allowed           BOOLEAN NOT NULL DEFAULT TRUE,
    max_output_tokens BIGINT NOT NULL DEFAULT 0 CHECK (max_output_tokens >= 0),
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT tenant_model_grants_key_fk FOREIGN KEY (tenant_id, key_id)
        REFERENCES tenant_keys (tenant_id, id) ON DELETE RESTRICT,
    CONSTRAINT tenant_model_grants_tenant_id_unique UNIQUE (tenant_id, id),
    CONSTRAINT tenant_model_grants_scope_unique
        UNIQUE (tenant_id, key_id, model, provider)
);

COMMENT ON TABLE tenant_model_grants IS
    'Per-key model/provider grants. Enforced before routing, including aliases and failover (Stage 1.3).';

CREATE TABLE tariff_versions (
    id                           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id                    UUID NOT NULL,
    version                      INTEGER NOT NULL CHECK (version >= 1),
    currency                     CHAR(3) NOT NULL DEFAULT 'USD'
                                 CHECK (currency = 'USD'),
    effective_from               TIMESTAMPTZ NOT NULL,
    input_micro_usd_per_million  NUMERIC(39,0) NOT NULL
        CHECK (input_micro_usd_per_million >= 0
               AND input_micro_usd_per_million < 340282366920938463463374607431768211456),
    output_micro_usd_per_million NUMERIC(39,0) NOT NULL
        CHECK (output_micro_usd_per_million >= 0
               AND output_micro_usd_per_million < 340282366920938463463374607431768211456),
    rounding                     TEXT NOT NULL DEFAULT 'up' CHECK (rounding = 'up'),
    status                       TEXT NOT NULL DEFAULT 'draft'
                                 CHECK (status IN ('draft', 'active', 'retired')),
    created_at                   TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT tariff_versions_tenant_fk FOREIGN KEY (tenant_id)
        REFERENCES tenants (tenant_id) ON DELETE RESTRICT,
    CONSTRAINT tariff_versions_tenant_id_unique UNIQUE (tenant_id, id),
    CONSTRAINT tariff_versions_scope_unique UNIQUE (tenant_id, version, currency)
);

COMMENT ON TABLE tariff_versions IS
    'Customer tariff snapshots: integer micro-USD per 1,000,000 tokens, rounded up per line item.';
