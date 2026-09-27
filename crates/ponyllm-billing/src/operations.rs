//! Commercial Operations & Tenant Self-Service Interfaces (Stage 3)
//!
//! Provides:
//! - Tenant self-service: Key issuance/rotation, balance check, usage export
//! - Admin operations: Tenant provisioning, manual credit adjustment (compensating entries), tariff configuration
//! - Quota & Rate Limiting policy models (RPM, TPM, Concurrency limits)

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantProfileView {
    pub tenant_id: Uuid,
    pub name: String,
    pub status: String,
    pub currency: String,
    pub available_micro_usd: u128,
    pub reserved_micro_usd: u128,
    pub credits_total_micro_usd: u128,
    pub debits_total_micro_usd: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantKeyRotationRequest {
    pub key_id: Uuid,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantKeyRotationResponse {
    pub key_id: Uuid,
    pub new_key_plaintext: String,
    pub prefix: String,
    pub last4: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminCreditAdjustmentRequest {
    pub tenant_id: Uuid,
    pub amount_micro_usd: u128,
    pub reason: String,
    pub actor_id: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommercialRateLimitPolicy {
    pub tenant_id: Uuid,
    pub max_rpm: u32,
    pub max_tpm: u64,
    pub max_concurrency: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReconciliationReport {
    pub reconciled_at: chrono::DateTime<chrono::Utc>,
    pub total_tenants: usize,
    pub total_credits_micro_usd: u128,
    pub total_debits_micro_usd: u128,
    pub total_reserved_micro_usd: u128,
    pub total_available_micro_usd: u128,
    pub is_conserved: bool,
    pub discrepancies_count: usize,
}
