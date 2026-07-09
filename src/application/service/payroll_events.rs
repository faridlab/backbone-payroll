//! Payroll domain events (hand-authored, user-owned) — the public extension surface.
//!
//! Payroll is a GL producer: on posting a run it emits `PayrollPosted` (the salary journal landed) so
//! settlement can pay the net + remit the statutory payables, and reporting can reconcile. A consuming
//! service supplies the sink (bus, outbox, …).

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// One obligation the posted run created, owed to the account it credits. `statutory` routes it: true →
/// remit to a statutory authority (BPJS, PPh 21); false → an ordinary deduction (loan, advance). This is
/// the breakdown `backbone-payments` settles — without it the consumer would have to re-query payroll's
/// private slip-line tables (a boundary violation) to split the lump `total_deductions` per authority
/// (completeness council 2026-07-08).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PayrollPayable {
    pub gl_account_id: Uuid,
    pub amount: Decimal,
    pub statutory: bool,
}

/// A payroll run was posted to the GL — the salary journal exists; net pay + statutory payables are owed.
/// Carries the payable breakdown the settlement consumer needs: net pay clears `salary_payable_account_id`
/// (amount = `total_net`); each entry in `payables` is remitted to its own account.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PayrollPosted {
    pub payroll_entry_id: Uuid,
    pub company_id: Uuid,
    pub journal_id: Uuid,
    pub post_id: Uuid,
    pub total_gross: Decimal,
    pub total_deductions: Decimal,
    pub total_net: Decimal,
    /// Net pay clears here (amount = `total_net`) — the employees' take-home payable.
    pub salary_payable_account_id: Uuid,
    /// Each deduction payable to remit, grouped by account (BPJS/PPh 21 payables + any other deduction).
    pub payables: Vec<PayrollPayable>,
}

/// The payroll domain-event union.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum PayrollEvent {
    PayrollPosted(PayrollPosted),
}

/// Sink the write path publishes to. A consuming service supplies its own (bus, outbox, …).
pub trait PayrollEventSink: Send + Sync {
    fn publish(&self, event: &PayrollEvent);
}

/// A no-op/logging sink for tests and single-process composition.
#[derive(Debug, Default, Clone)]
pub struct LoggingSink;

impl PayrollEventSink for LoggingSink {
    fn publish(&self, event: &PayrollEvent) {
        tracing::info!(?event, "payroll event");
    }
}
