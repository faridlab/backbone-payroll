//! The GL-posting seam against the REAL backbone-accounting ledger — payroll is the 8th GL producer.
//! Proves the salary journal lands balanced, accounting accepts `source_type='payroll'`, and re-posting
//! reuses the one journal (posts at most once). ZERO normal Cargo edge to accounting — the envelope is
//! the wire contract; the ACL adapter (common::GlAdapter) maps it into accounting's PostingRequest.

mod common;
use common::*;

use backbone_payroll::application::service::payroll_events::LoggingSink;
use backbone_payroll::application::service::payroll_write_service::*;
use rust_decimal::Decimal;
use uuid::Uuid;

async fn posted_run(pool: &sqlx::PgPool, svc: &PayrollWriteService, a: &PayrollAccounts, company: Uuid) -> Uuid {
    let structure = svc.create_structure(NewStructure {
        company_id: company, name: "Staff".into(),
        components: vec![NewComponent {
            name: "Gaji Pokok".into(), component_type: "earning".into(),
            amount: dec("10000000"), gl_account_id: a.salary_expense,
        }],
    }).await.unwrap();
    let run = svc.create_payroll_entry(NewPayrollEntry {
        company_id: company, period_year: 2026, period_month: 7,
        salary_expense_account_id: a.salary_expense, salary_payable_account_id: a.salary_payable,
    }).await.unwrap();
    svc.add_salary_slip(run, NewSalarySlip {
        employee_id: Uuid::new_v4(), structure_id: structure, working_days: dec("22"), unpaid_days: dec("0"),
        statutory: vec![
            StatutoryLine { name: "BPJS".into(), amount: dec("240000"), gl_account_id: a.bpjs_payable },
            StatutoryLine { name: "PPh 21".into(), amount: dec("500000"), gl_account_id: a.pph21_payable },
        ],
    }).await.unwrap();
    svc.process_payroll_entry(run).await.unwrap();
    let _ = pool;
    run
}

// PGSEAM-1 — the salary journal lands in the REAL ledger, balanced and double-entry, and accounting
// accepts the 'payroll' posting source. Dr Salary Expense 10,000,000 · Cr Salary Payable 9,260,000 ·
// Cr BPJS 240,000 · Cr PPh21 500,000.
#[tokio::test]
async fn pgseam1_salary_journal_lands_balanced_in_real_ledger() {
    let pool = pool().await;
    let company = Uuid::new_v4();
    let a = payroll_accounts(&pool, company).await;
    let svc = PayrollWriteService::new(pool.clone());
    let gl = GlAdapter::new(pool.clone());

    let run = posted_run(&pool, &svc, &a, company).await;
    let out = svc.post_payroll_entry(run, today(), &gl, &LoggingSink).await.expect("real accounting accepts payroll post");
    assert!(!out.already);

    // Ledger balances: expense debit = gross; the three liabilities credit the rest; net = Σ − ded.
    assert_eq!(balance(&pool, a.salary_expense).await, dec("10000000"));
    assert_eq!(balance(&pool, a.salary_payable).await, dec("-9260000")); // credit shows negative
    assert_eq!(balance(&pool, a.bpjs_payable).await, dec("-240000"));
    assert_eq!(balance(&pool, a.pph21_payable).await, dec("-500000"));
    // Whole journal nets to zero across its accounts.
    let net: Decimal = balance(&pool, a.salary_expense).await
        + balance(&pool, a.salary_payable).await
        + balance(&pool, a.bpjs_payable).await
        + balance(&pool, a.pph21_payable).await;
    assert_eq!(net, Decimal::ZERO, "double-entry: Σ debits = Σ credits");
}

// PGSEAM-2 — re-posting reuses the one journal: the ledger is not doubled.
#[tokio::test]
async fn pgseam2_repost_reuses_one_journal() {
    let pool = pool().await;
    let company = Uuid::new_v4();
    let a = payroll_accounts(&pool, company).await;
    let svc = PayrollWriteService::new(pool.clone());
    let gl = GlAdapter::new(pool.clone());

    let run = posted_run(&pool, &svc, &a, company).await;
    let first = svc.post_payroll_entry(run, today(), &gl, &LoggingSink).await.unwrap();
    let second = svc.post_payroll_entry(run, today(), &gl, &LoggingSink).await.unwrap();

    assert!(second.already);
    assert_eq!(first.journal_id, second.journal_id);
    // Expense still shows exactly one run's worth — not 2×.
    assert_eq!(balance(&pool, a.salary_expense).await, dec("10000000"));
}
