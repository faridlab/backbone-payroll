//! Outbound GL-posting port (hand-authored, user-owned) — payroll side of the GL-posting contract
//! (`docs/erp/gl-posting-contract.md`).
//!
//! Payroll is a pure emitter of ONE balanced posting per run, `source_type = "payroll"`,
//! `posting_type='original'`, `source_id = payroll_entry_id`:
//!   Dr Salary Expense (gross) · Cr Salary Payable (net) · Cr <statutory payable> (BPJS + PPh 21, grouped
//!   by account). Because `gross = net + Σ deductions`, the posting balances. Reached only through a
//!   `GlPostSink`; the ACL adapter maps the envelope into accounting's `PostingRequest`. ZERO normal
//!   Cargo edge to accounting — the envelope is the wire contract, duplicated per producer by design.

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One debit/credit line of an emitted posting. Exactly one of `debit`/`credit` is > 0.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GlPostLine {
    pub account_id: Uuid,
    pub debit: Decimal,
    pub credit: Decimal,
    pub party_type: Option<String>,
    pub party_id: Option<Uuid>,
    pub description: Option<String>,
}

impl GlPostLine {
    pub fn debit(account_id: Uuid, amount: Decimal) -> Self {
        Self { account_id, debit: amount, credit: Decimal::ZERO, party_type: None, party_id: None, description: None }
    }
    pub fn credit(account_id: Uuid, amount: Decimal) -> Self {
        Self { account_id, debit: Decimal::ZERO, credit: amount, party_type: None, party_id: None, description: None }
    }
    pub fn with_description(mut self, d: impl Into<String>) -> Self {
        self.description = Some(d.into());
        self
    }
}

/// A balanced posting request emitted by payroll — the contract envelope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AccountingPostEnvelope {
    pub idempotency_key: String,
    pub company_id: Uuid,
    pub branch_id: Option<Uuid>,
    /// Posting source discriminator — payroll emits "payroll".
    pub source_type: String,
    /// The payroll run id.
    pub source_id: Uuid,
    pub source_reference: Option<String>,
    pub posting_date: chrono::NaiveDate,
    pub currency: String,
    /// "original" (payroll does not reverse in the MVP).
    pub posting_type: String,
    pub description: Option<String>,
    pub lines: Vec<GlPostLine>,
}

impl AccountingPostEnvelope {
    pub fn totals(&self) -> (Decimal, Decimal) {
        (
            self.lines.iter().map(|l| l.debit).sum(),
            self.lines.iter().map(|l| l.credit).sum(),
        )
    }
    pub fn is_balanced(&self) -> bool {
        let (d, c) = self.totals();
        d == c && !self.lines.is_empty()
    }
}

/// Acknowledgement returned by the GL after a successful post.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GlPostAck {
    pub post_id: Uuid,
    pub journal_id: Uuid,
    pub idempotent_reuse: bool,
}

/// Rejection returned by the GL (validation failure). `code` is the stable contract error string.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GlPostRejected {
    pub code: String,
    pub message: String,
}

/// The GL-posting seam. A composing service implements this over accounting's `PostingService`.
#[async_trait::async_trait]
pub trait GlPostSink: Send + Sync {
    async fn post(&self, envelope: &AccountingPostEnvelope) -> Result<GlPostAck, GlPostRejected>;
}
