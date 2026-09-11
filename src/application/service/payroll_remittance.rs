//! The remittance port — how a posted run's payables leave payroll for payment.
//!
//! Posting creates obligations (the per-account `payables` breakdown on `PayrollPosted`); PAYING
//! them is a separate, explicitly-triggered verb (`remit_payroll_entry`). The port keeps payroll
//! ignorant of payee resolution (which BPJS office, which tax office, which bank): an instruction
//! names only the GL account, amount, and whether it is statutory, and the composing host's
//! adapter turns it into an actual payment over its payment module. The default
//! [`UnwiredRemittance`] fails CLOSED — an unwired deployment can never mistake "nothing was
//! remitted" for success.

use rust_decimal::Decimal;
use uuid::Uuid;

/// One payable leaving payroll for payment. The `idempotency_key` is stable per
/// (company, run, account): re-running the remit verb re-sends the SAME key, so the sink (and
/// whatever it fronts) can dedup retries safely.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemittanceInstruction {
    /// The legacy tenancy twin (ADR-0029) — the payment contract keeps the field for unstripped
    /// consumers. The payroll tables hold no company column; the write path echoes the ambient
    /// org scope's legacy company id, or nil when none is bound. It is stamped onto the payment
    /// and into the idempotency key, but nothing in payroll keys a statement on it.
    pub company_id: Uuid,
    /// The posted run the payable came from (correlation id).
    pub run_id: Uuid,
    /// The GL payable account the obligation was credited to — the remittance target.
    pub gl_account_id: Uuid,
    /// Amount owed to that account, as grouped at post time.
    pub amount: Decimal,
    /// Routes the payment downstream: statutory authority (BPJS, PPh 21) vs an ordinary
    /// deduction payable (loan, advance).
    pub statutory: bool,
    /// See the type docs — stable across retries of the same remittance.
    pub idempotency_key: String,
}

impl RemittanceInstruction {
    pub fn new(
        company_id: Uuid,
        run_id: Uuid,
        gl_account_id: Uuid,
        amount: Decimal,
        statutory: bool,
    ) -> Self {
        Self {
            company_id,
            run_id,
            gl_account_id,
            amount,
            statutory,
            idempotency_key: format!("payroll_remittance:{company_id}:{run_id}:{gl_account_id}"),
        }
    }
}

/// The sink's ack: the instruction was accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemitAck {
    /// The downstream payment id when the sink created one synchronously (outbox-style sinks
    /// may have none yet).
    pub payment_id: Option<Uuid>,
    /// True when the idempotency key was already seen — nothing new was created.
    pub duplicate: bool,
}

/// Errors from the remittance seam. `Unwired` is the load-bearing variant: the default
/// [`UnwiredRemittance`] returns it, and the remit verb fails closed with the stable
/// `remittance_seam_unwired` code rather than silently skipping.
#[derive(Debug, thiserror::Error)]
pub enum RemittanceSeamError {
    #[error("the remittance seam is not wired — supply a RemittanceSink to remit posted payables")]
    Unwired,
    #[error("the remittance sink rejected the instruction ({code}): {message}")]
    Rejected { code: String, message: String },
    #[error("remittance seam transport error: {0}")]
    Transport(String),
}

impl RemittanceSeamError {
    /// Stable machine code the HTTP layer surfaces (422 for Unwired/Rejected).
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unwired => "remittance_seam_unwired",
            Self::Rejected { .. } => "remittance_rejected",
            Self::Transport(_) => "remittance_seam_error",
        }
    }
}

/// The port. Implemented by the composing host over its payment module's write service; payroll
/// only ever speaks this trait.
#[async_trait::async_trait]
pub trait RemittanceSink: Send + Sync {
    async fn remit(&self, instruction: &RemittanceInstruction) -> Result<RemitAck, RemittanceSeamError>;
}

/// The default port: nothing is wired. Remitting fails loudly — an explicit error, never a
/// payable silently assumed paid.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnwiredRemittance;

#[async_trait::async_trait]
impl RemittanceSink for UnwiredRemittance {
    async fn remit(&self, _instruction: &RemittanceInstruction) -> Result<RemitAck, RemittanceSeamError> {
        Err(RemittanceSeamError::Unwired)
    }
}
