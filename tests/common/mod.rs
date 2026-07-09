//! Shared test helpers: a live pool, a real-accounting GL adapter, account seeding + ledger balances,
//! a counting GL sink, and NPWP generation for real-HR employees. Fresh random ids per test so rows
//! never collide across parallel runs.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use backbone_accounting::application::service::posting_service::{
    PostingLine, PostingRequest, PostingService,
};
use backbone_payroll::application::service::payroll_events::{PayrollEvent, PayrollEventSink};
use backbone_payroll::application::service::payroll_gl::{
    AccountingPostEnvelope, GlPostAck, GlPostRejected, GlPostSink,
};
use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

pub fn dburl() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5433/backbone_payroll".into())
}
pub async fn pool() -> PgPool {
    PgPool::connect(&dburl()).await.expect("connect")
}
pub fn dec(s: &str) -> Decimal {
    s.parse().unwrap()
}
pub fn today() -> chrono::NaiveDate {
    chrono::Utc::now().date_naive()
}
/// A unique 15-digit NPWP derived from a random UUID (HR onboards enforce a unique NPWP index).
pub fn npwp() -> String {
    let u = Uuid::new_v4().as_u128();
    let n = (u % 1_000_000_000_000_000) as u64;
    format!("{n:015}")
}

pub async fn account(pool: &PgPool, company: Uuid, code: &str, atype: &str, subtype: &str, normal: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO accounting.accounts
             (id, company_id, account_number, account_code, name, account_type, account_subtype,
              normal_balance, is_header, is_detail, status)
           VALUES ($1,$2,$3,$4,$5,$6::account_type,$7::account_subtype,$8::normal_balance,
                   false,true,'active'::account_status)"#,
    )
    .bind(id).bind(company).bind(code).bind(code).bind(code).bind(atype).bind(subtype).bind(normal)
    .execute(pool).await.expect("seed account");
    id
}

pub async fn balance(pool: &PgPool, account: Uuid) -> Decimal {
    sqlx::query_scalar(
        "SELECT COALESCE(SUM(debit_amount),0) - COALESCE(SUM(credit_amount),0)
         FROM accounting.ledgers WHERE account_id=$1",
    )
    .bind(account)
    .fetch_one(pool)
    .await
    .expect("balance")
}

/// The GL accounts a payroll run posts to: salary expense (Dr), and the liability control accounts the
/// deductions/net credit. These are aggregate control accounts (current_liability), NOT AR/AP subledger
/// accounts — the payroll journal is an aggregate; per-employee settlement is the payments module's job.
pub struct PayrollAccounts {
    pub salary_expense: Uuid,
    pub salary_payable: Uuid,
    pub bpjs_payable: Uuid,
    pub pph21_payable: Uuid,
}
pub async fn payroll_accounts(pool: &PgPool, company: Uuid) -> PayrollAccounts {
    PayrollAccounts {
        salary_expense: account(pool, company, "6100-SAL", "expense", "operating_expense", "debit").await,
        salary_payable: account(pool, company, "2100-SPY", "liability", "current_liability", "credit").await,
        bpjs_payable: account(pool, company, "2110-BPJS", "liability", "current_liability", "credit").await,
        pph21_payable: account(pool, company, "2120-PPH", "liability", "current_liability", "credit").await,
    }
}

/// ACL: payroll's serialized envelope → accounting's PostingRequest against the REAL ledger.
pub struct GlAdapter {
    pub svc: PostingService,
}
impl GlAdapter {
    pub fn new(pool: PgPool) -> Self {
        Self { svc: PostingService::new(pool) }
    }
}
#[async_trait::async_trait]
impl GlPostSink for GlAdapter {
    async fn post(&self, e: &AccountingPostEnvelope) -> Result<GlPostAck, GlPostRejected> {
        let mut r = PostingRequest::original(e.company_id, &e.source_type, e.source_id, e.posting_date);
        r.source_reference = e.source_reference.clone();
        r.posting_type = e.posting_type.clone();
        r.lines = e.lines.iter().map(|l| PostingLine {
            account_id: l.account_id, debit: l.debit, credit: l.credit,
            party_type: l.party_type.clone(), party_id: l.party_id,
            cost_center_id: None, project_id: None, department_id: None, description: l.description.clone(),
        }).collect();
        match self.svc.post(r, None).await {
            Ok(x) => Ok(GlPostAck { post_id: x.post_id, journal_id: x.journal_id, idempotent_reuse: x.idempotent_reuse }),
            Err(x) => Err(GlPostRejected { code: x.code().to_string(), message: x.to_string() }),
        }
    }
}

/// A counting GL sink — records each post's idempotency_key so tests can assert how many posts reached
/// the ledger, without touching a real ledger.
#[derive(Clone, Default)]
pub struct CountingGl {
    pub posts: Arc<Mutex<Vec<AccountingPostEnvelope>>>,
}
impl CountingGl {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn count(&self) -> usize {
        self.posts.lock().unwrap().len()
    }
    pub fn last(&self) -> AccountingPostEnvelope {
        self.posts.lock().unwrap().last().cloned().expect("a post")
    }
}
#[async_trait::async_trait]
impl GlPostSink for CountingGl {
    async fn post(&self, e: &AccountingPostEnvelope) -> Result<GlPostAck, GlPostRejected> {
        self.posts.lock().unwrap().push(e.clone());
        Ok(GlPostAck { post_id: Uuid::new_v4(), journal_id: Uuid::new_v4(), idempotent_reuse: false })
    }
}

/// Captures the `PayrollPosted` events the run engine publishes so settlement-facing tests can assert the
/// payable breakdown the consumer receives.
#[derive(Clone, Default)]
pub struct CapturingEvents {
    pub events: Arc<Mutex<Vec<PayrollEvent>>>,
}
impl CapturingEvents {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn last_posted(&self) -> backbone_payroll::application::service::payroll_events::PayrollPosted {
        match self.events.lock().unwrap().last().cloned().expect("a PayrollPosted") {
            PayrollEvent::PayrollPosted(p) => p,
        }
    }
}
impl PayrollEventSink for CapturingEvents {
    fn publish(&self, event: &PayrollEvent) {
        self.events.lock().unwrap().push(event.clone());
    }
}
