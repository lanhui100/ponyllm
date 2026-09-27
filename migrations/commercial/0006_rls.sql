-- 0006_rls.sql
-- Forced row-level security for every tenant-owned table.
--
-- Every policy is exactly `tenant_id = current_setting('app.tenant_id', true)::uuid`
-- for both USING and WITH CHECK. There is:
--   * no policy `TO` clause, so the policy applies to every role, owner included
--     (together with FORCE, that removes the owner bypass);
--   * no client- or operator-controllable bypass setting anywhere in this
--     schema, so no runtime flag can widen visibility;
--   * a missing or malformed `app.tenant_id` makes the predicate NULL or raises,
--     which fails closed (zero rows / error), never open.

ALTER TABLE tenants ENABLE ROW LEVEL SECURITY;
ALTER TABLE tenants FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tenants_tenant_isolation ON tenants;
CREATE POLICY tenants_tenant_isolation ON tenants
    USING (tenant_id = current_setting('app.tenant_id', true)::uuid)
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true)::uuid);

ALTER TABLE tenant_keys ENABLE ROW LEVEL SECURITY;
ALTER TABLE tenant_keys FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tenant_keys_tenant_isolation ON tenant_keys;
CREATE POLICY tenant_keys_tenant_isolation ON tenant_keys
    USING (tenant_id = current_setting('app.tenant_id', true)::uuid)
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true)::uuid);

ALTER TABLE tenant_model_grants ENABLE ROW LEVEL SECURITY;
ALTER TABLE tenant_model_grants FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tenant_model_grants_tenant_isolation ON tenant_model_grants;
CREATE POLICY tenant_model_grants_tenant_isolation ON tenant_model_grants
    USING (tenant_id = current_setting('app.tenant_id', true)::uuid)
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true)::uuid);

ALTER TABLE tariff_versions ENABLE ROW LEVEL SECURITY;
ALTER TABLE tariff_versions FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS tariff_versions_tenant_isolation ON tariff_versions;
CREATE POLICY tariff_versions_tenant_isolation ON tariff_versions
    USING (tenant_id = current_setting('app.tenant_id', true)::uuid)
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true)::uuid);

ALTER TABLE wallets ENABLE ROW LEVEL SECURITY;
ALTER TABLE wallets FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS wallets_tenant_isolation ON wallets;
CREATE POLICY wallets_tenant_isolation ON wallets
    USING (tenant_id = current_setting('app.tenant_id', true)::uuid)
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true)::uuid);

ALTER TABLE reservations ENABLE ROW LEVEL SECURITY;
ALTER TABLE reservations FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS reservations_tenant_isolation ON reservations;
CREATE POLICY reservations_tenant_isolation ON reservations
    USING (tenant_id = current_setting('app.tenant_id', true)::uuid)
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true)::uuid);

ALTER TABLE ledger_entries ENABLE ROW LEVEL SECURITY;
ALTER TABLE ledger_entries FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS ledger_entries_tenant_isolation ON ledger_entries;
CREATE POLICY ledger_entries_tenant_isolation ON ledger_entries
    USING (tenant_id = current_setting('app.tenant_id', true)::uuid)
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true)::uuid);

ALTER TABLE ledger_state_transitions ENABLE ROW LEVEL SECURITY;
ALTER TABLE ledger_state_transitions FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS ledger_state_transitions_tenant_isolation ON ledger_state_transitions;
CREATE POLICY ledger_state_transitions_tenant_isolation ON ledger_state_transitions
    USING (tenant_id = current_setting('app.tenant_id', true)::uuid)
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true)::uuid);

ALTER TABLE ledger_attempts ENABLE ROW LEVEL SECURITY;
ALTER TABLE ledger_attempts FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS ledger_attempts_tenant_isolation ON ledger_attempts;
CREATE POLICY ledger_attempts_tenant_isolation ON ledger_attempts
    USING (tenant_id = current_setting('app.tenant_id', true)::uuid)
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true)::uuid);

ALTER TABLE commercial_idempotency ENABLE ROW LEVEL SECURITY;
ALTER TABLE commercial_idempotency FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS commercial_idempotency_tenant_isolation ON commercial_idempotency;
CREATE POLICY commercial_idempotency_tenant_isolation ON commercial_idempotency
    USING (tenant_id = current_setting('app.tenant_id', true)::uuid)
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true)::uuid);

ALTER TABLE commercial_audit_log ENABLE ROW LEVEL SECURITY;
ALTER TABLE commercial_audit_log FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS commercial_audit_log_tenant_isolation ON commercial_audit_log;
CREATE POLICY commercial_audit_log_tenant_isolation ON commercial_audit_log
    USING (tenant_id = current_setting('app.tenant_id', true)::uuid)
    WITH CHECK (tenant_id = current_setting('app.tenant_id', true)::uuid);

-- Least privilege for the tenant role.
-- Crucial: tenants CANNOT insert into ledger_entries (only controlled billing service/functions can),
-- CANNOT update wallets (balance updates are transactional/immutable),
-- and CANNOT update/delete append-only evidence tables (ledger_state_transitions, ledger_attempts).
GRANT USAGE ON SCHEMA public TO ponyllm_commercial_tenant;

GRANT SELECT ON
    wallets,
    ledger_entries,
    ledger_state_transitions,
    ledger_attempts,
    commercial_audit_log
TO ponyllm_commercial_tenant;

GRANT SELECT, INSERT, UPDATE ON
    tenants,
    tenant_keys,
    tenant_model_grants,
    tariff_versions,
    reservations,
    commercial_idempotency
TO ponyllm_commercial_tenant;

GRANT INSERT ON
    ledger_state_transitions,
    ledger_attempts
TO ponyllm_commercial_tenant;
