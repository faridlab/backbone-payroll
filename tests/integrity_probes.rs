//! Integrity probes — the run engine's invariants: net pay never goes negative, no duplicate slip per
//! employee, and the draft→processed→posted transition gates hold against out-of-order calls.

mod common;
use common::*;

use backbone_payroll::application::service::payroll_events::LoggingSink;
use backbone_payroll::application::service::payroll_write_service::*;
use uuid::Uuid;

async fn setup() -> (sqlx::PgPool, Uuid, PayrollAccounts, PayrollWriteService, Uuid) {
    let pool = pool().await;
    let company = Uuid::new_v4();
    let a = payroll_accounts(&pool, company).await;
    let svc = PayrollWriteService::new(pool.clone());
    let structure = svc.create_structure(NewStructure {
        company_id: company, name: "Staff".into(),
        components: vec![NewComponent {
            name: "Gaji Pokok".into(), component_type: "earning".into(),
            amount: dec("5000000"), gl_account_id: a.salary_expense,
        }],
    }).await.unwrap();
    (pool, company, a, svc, structure)
}

fn new_run(company: Uuid, a: &PayrollAccounts) -> NewPayrollEntry {
    NewPayrollEntry {
        company_id: company, period_year: 2026, period_month: 7,
        salary_expense_account_id: a.salary_expense, salary_payable_account_id: a.salary_payable,
    }
}

// PIP-1 — deductions exceeding gross would make net pay negative → rejected (never persisted).
#[tokio::test]
async fn pip1_net_cannot_go_negative() {
    let (_pool, company, a, svc, structure) = setup().await;
    let run = svc.create_payroll_entry(new_run(company, &a)).await.unwrap();
    let r = svc.add_salary_slip(run, NewSalarySlip {
        employee_id: Uuid::new_v4(), structure_id: structure,
        working_days: dec("22"), unpaid_days: dec("0"),
        statutory: vec![StatutoryLine { name: "Loan".into(), amount: dec("6000000"), gl_account_id: a.bpjs_payable }],
    }).await;
    assert!(matches!(r, Err(PayrollError::Invalid(_))), "deductions > gross must be rejected");
}

// PIP-2 — an employee can appear at most once in a run.
#[tokio::test]
async fn pip2_no_duplicate_slip_per_employee() {
    let (_pool, company, a, svc, structure) = setup().await;
    let run = svc.create_payroll_entry(new_run(company, &a)).await.unwrap();
    let emp = Uuid::new_v4();
    let slip = NewSalarySlip { employee_id: emp, structure_id: structure, working_days: dec("22"), unpaid_days: dec("0"), statutory: vec![] };
    svc.add_salary_slip(run, NewSalarySlip { ..clone_slip(&slip) }).await.unwrap();
    let dup = svc.add_salary_slip(run, slip).await;
    assert!(matches!(dup, Err(PayrollError::Invalid(_))), "duplicate employee in a run must be rejected");
}

fn clone_slip(s: &NewSalarySlip) -> NewSalarySlip {
    NewSalarySlip { employee_id: s.employee_id, structure_id: s.structure_id, working_days: s.working_days, unpaid_days: s.unpaid_days, statutory: vec![] }
}

// PIP-3 — cannot post a run that has not been processed (still draft).
#[tokio::test]
async fn pip3_cannot_post_unprocessed_run() {
    let (_pool, company, a, svc, structure) = setup().await;
    let run = svc.create_payroll_entry(new_run(company, &a)).await.unwrap();
    svc.add_salary_slip(run, NewSalarySlip {
        employee_id: Uuid::new_v4(), structure_id: structure, working_days: dec("22"), unpaid_days: dec("0"), statutory: vec![],
    }).await.unwrap();
    let r = svc.post_payroll_entry(run, today(), &CountingGl::new(), &LoggingSink).await;
    assert!(matches!(r, Err(PayrollError::InvalidState(_))), "draft run cannot post");
}

// PIP-4 — a run with no slips cannot be processed.
#[tokio::test]
async fn pip4_empty_run_cannot_process() {
    let (_pool, company, a, svc, _structure) = setup().await;
    let run = svc.create_payroll_entry(new_run(company, &a)).await.unwrap();
    let r = svc.process_payroll_entry(run).await;
    assert!(matches!(r, Err(PayrollError::Invalid(_))), "empty run cannot process");
}

// PIP-5 — the processed→posted transition is one-way: a processed run cannot be re-processed, and a
// slip cannot be added after processing (the run is no longer draft).
#[tokio::test]
async fn pip5_transition_gates_are_one_way() {
    let (_pool, company, a, svc, structure) = setup().await;
    let run = svc.create_payroll_entry(new_run(company, &a)).await.unwrap();
    svc.add_salary_slip(run, NewSalarySlip {
        employee_id: Uuid::new_v4(), structure_id: structure, working_days: dec("22"), unpaid_days: dec("0"), statutory: vec![],
    }).await.unwrap();
    svc.process_payroll_entry(run).await.unwrap();

    let reprocess = svc.process_payroll_entry(run).await;
    assert!(matches!(reprocess, Err(PayrollError::InvalidState(_))), "cannot re-process");

    let late_slip = svc.add_salary_slip(run, NewSalarySlip {
        employee_id: Uuid::new_v4(), structure_id: structure, working_days: dec("22"), unpaid_days: dec("0"), statutory: vec![],
    }).await;
    assert!(matches!(late_slip, Err(PayrollError::InvalidState(_))), "cannot add a slip after processing");
}

// PIP-6 — a duplicate run for the same company/period is rejected (unique guard).
#[tokio::test]
async fn pip6_one_run_per_company_period() {
    let (_pool, company, a, svc, _structure) = setup().await;
    svc.create_payroll_entry(new_run(company, &a)).await.unwrap();
    let dup = svc.create_payroll_entry(new_run(company, &a)).await;
    assert!(matches!(dup, Err(PayrollError::Invalid(_))), "duplicate company/period run rejected");
}

// PIP-7 — proration never inflates gross above the structure (maturity council 2026-07-08). A NEGATIVE
// unpaid_days (a bad upstream hr.period_summary value) must not drive the proration factor above 1: the
// engine clamps unpaid to [0, working], and the DB CHECK backstops any other writer. Without the clamp,
// gross would be 5,000,000 × (22-(-5))/22 ≈ 6,136,363 — a balanced-but-over-booked salary journal.
#[tokio::test]
async fn pip7_negative_unpaid_days_cannot_inflate_gross() {
    let (pool, company, a, svc, structure) = setup().await;
    let run = svc.create_payroll_entry(new_run(company, &a)).await.unwrap();
    let slip = svc.add_salary_slip(run, NewSalarySlip {
        employee_id: Uuid::new_v4(), structure_id: structure,
        working_days: dec("22"), unpaid_days: dec("-5"), statutory: vec![],
    }).await.expect("negative unpaid days must be clamped, not rejected mid-insert");
    let gross = sqlx::query_scalar::<_, rust_decimal::Decimal>(
        "SELECT gross_pay FROM payroll.salary_slips WHERE id=$1")
        .bind(slip).fetch_one(&pool).await.unwrap();
    assert!(gross <= dec("5000000"), "gross must never exceed the structure base (got {gross})");
}
