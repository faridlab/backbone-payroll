//! Golden cases — the manufactured oracle for the salary run: exact gross→deductions→net math,
//! HR unpaid-day proration, run roll-up, deduction grouping, and post-once idempotency. Money is IDR,
//! 2dp, half-away-from-zero. These assert the DOMAIN math via a capturing sink; the REAL-ledger balance
//! and posting-source acceptance live in payroll_gl_seam.rs.

mod common;
use common::*;

use backbone_payroll::application::service::payroll_events::LoggingSink;
use backbone_payroll::application::service::payroll_write_service::*;
use rust_decimal::Decimal;
use uuid::Uuid;

fn earning(name: &str, amt: &str, acct: Uuid) -> NewComponent {
    NewComponent { name: name.into(), component_type: "earning".into(), amount: dec(amt), gl_account_id: acct }
}

/// Build the standard structure: Gaji Pokok 10,000,000 + Tunjangan 2,000,000 = 12,000,000 gross.
async fn standard_structure(svc: &PayrollWriteService, company: Uuid, expense: Uuid) -> Uuid {
    svc.create_structure(NewStructure {
        company_id: company,
        name: "Staff".into(),
        components: vec![
            earning("Gaji Pokok", "10000000", expense),
            earning("Tunjangan", "2000000", expense),
        ],
    })
    .await
    .expect("structure")
}

fn statutory(a: &PayrollAccounts) -> Vec<StatutoryLine> {
    vec![
        StatutoryLine { name: "BPJS".into(), amount: dec("240000"), gl_account_id: a.bpjs_payable },
        StatutoryLine { name: "PPh 21".into(), amount: dec("500000"), gl_account_id: a.pph21_payable },
    ]
}

// PGC-1 — full-month net pay: gross 12,000,000 − (BPJS 240,000 + PPh21 500,000) = 11,260,000.
#[tokio::test]
async fn pgc1_full_month_net_pay() {
    let pool = pool().await;
    let company = Uuid::new_v4();
    let a = payroll_accounts(&pool, company).await;
    let svc = PayrollWriteService::new(pool.clone());
    let structure = standard_structure(&svc, company, a.salary_expense).await;

    let run = svc.create_payroll_entry(NewPayrollEntry {
        company_id: company, period_year: 2026, period_month: 7,
        salary_expense_account_id: a.salary_expense, salary_payable_account_id: a.salary_payable,
    }).await.unwrap();

    svc.add_salary_slip(run, NewSalarySlip {
        employee_id: Uuid::new_v4(), structure_id: structure,
        working_days: dec("22"), unpaid_days: dec("0"), statutory: statutory(&a),
    }).await.unwrap();

    svc.process_payroll_entry(run).await.unwrap();

    let sink = CountingGl::new();
    let events = LoggingSink;
    let out = svc.post_payroll_entry(run, today(), &sink, &events).await.unwrap();
    assert!(!out.already);
    assert_eq!(out.total_net, dec("11260000"));

    let env = sink.last();
    assert!(env.is_balanced(), "salary journal must balance");
    // Dr salary expense (gross) = 12,000,000.
    let dr: Decimal = env.lines.iter().filter(|l| l.account_id == a.salary_expense).map(|l| l.debit).sum();
    assert_eq!(dr, dec("12000000"));
    // Cr net pay = 11,260,000.
    let net_cr: Decimal = env.lines.iter().filter(|l| l.account_id == a.salary_payable).map(|l| l.credit).sum();
    assert_eq!(net_cr, dec("11260000"));
    assert_eq!(env.source_type, "payroll");
    assert_eq!(env.source_id, run);
}

// PGC-2 — unpaid-day proration (the HR link): 2 unpaid of 22 working days scales earnings by 20/22.
// Gaji Pokok 9,090,909.09 + Tunjangan 1,818,181.82 = gross 10,909,090.91; net = gross − 740,000.
#[tokio::test]
async fn pgc2_unpaid_days_prorate_gross() {
    let pool = pool().await;
    let company = Uuid::new_v4();
    let a = payroll_accounts(&pool, company).await;
    let svc = PayrollWriteService::new(pool.clone());
    let structure = standard_structure(&svc, company, a.salary_expense).await;

    let run = svc.create_payroll_entry(NewPayrollEntry {
        company_id: company, period_year: 2026, period_month: 7,
        salary_expense_account_id: a.salary_expense, salary_payable_account_id: a.salary_payable,
    }).await.unwrap();

    let slip = svc.add_salary_slip(run, NewSalarySlip {
        employee_id: Uuid::new_v4(), structure_id: structure,
        working_days: dec("22"), unpaid_days: dec("2"), statutory: statutory(&a),
    }).await.unwrap();

    let row = sqlx::query_scalar::<_, Decimal>(
        "SELECT gross_pay FROM payroll.salary_slips WHERE id=$1")
        .bind(slip).fetch_one(&pool).await.unwrap();
    assert_eq!(row, dec("10909090.91"), "gross prorated by (22-2)/22");

    let net = sqlx::query_scalar::<_, Decimal>(
        "SELECT net_pay FROM payroll.salary_slips WHERE id=$1")
        .bind(slip).fetch_one(&pool).await.unwrap();
    assert_eq!(net, dec("10169090.91"), "net = prorated gross − 740,000 deductions");
}

// PGC-3 — multi-slip run rolls up: two employees' slips sum into the run totals, and the same deduction
// account across both slips is GROUPED into ONE credit line.
#[tokio::test]
async fn pgc3_run_rollup_and_deduction_grouping() {
    let pool = pool().await;
    let company = Uuid::new_v4();
    let a = payroll_accounts(&pool, company).await;
    let svc = PayrollWriteService::new(pool.clone());
    let structure = standard_structure(&svc, company, a.salary_expense).await;

    let run = svc.create_payroll_entry(NewPayrollEntry {
        company_id: company, period_year: 2026, period_month: 7,
        salary_expense_account_id: a.salary_expense, salary_payable_account_id: a.salary_payable,
    }).await.unwrap();

    for _ in 0..2 {
        svc.add_salary_slip(run, NewSalarySlip {
            employee_id: Uuid::new_v4(), structure_id: structure,
            working_days: dec("22"), unpaid_days: dec("0"), statutory: statutory(&a),
        }).await.unwrap();
    }
    svc.process_payroll_entry(run).await.unwrap();

    let (g, d, n) = sqlx::query_as::<_, (Decimal, Decimal, Decimal)>(
        "SELECT total_gross, total_deductions, total_net FROM payroll.payroll_entries WHERE id=$1")
        .bind(run).fetch_one(&pool).await.unwrap();
    assert_eq!(g, dec("24000000")); // 2 × 12,000,000
    assert_eq!(d, dec("1480000"));  // 2 × 740,000
    assert_eq!(n, dec("22520000")); // 2 × 11,260,000

    let sink = CountingGl::new();
    svc.post_payroll_entry(run, today(), &sink, &LoggingSink).await.unwrap();
    let env = sink.last();
    // BPJS from both slips grouped into a single credit line = 480,000.
    let bpjs_lines = env.lines.iter().filter(|l| l.account_id == a.bpjs_payable).count();
    assert_eq!(bpjs_lines, 1, "same deduction account grouped into one line");
    let bpjs_amt: Decimal = env.lines.iter().filter(|l| l.account_id == a.bpjs_payable).map(|l| l.credit).sum();
    assert_eq!(bpjs_amt, dec("480000"));
    assert!(env.is_balanced());
}

// PGC-4 — post-once idempotency: re-posting a posted run does NOT reach the sink again and returns the
// same journal with already=true.
#[tokio::test]
async fn pgc4_post_is_idempotent() {
    let pool = pool().await;
    let company = Uuid::new_v4();
    let a = payroll_accounts(&pool, company).await;
    let svc = PayrollWriteService::new(pool.clone());
    let structure = standard_structure(&svc, company, a.salary_expense).await;

    let run = svc.create_payroll_entry(NewPayrollEntry {
        company_id: company, period_year: 2026, period_month: 7,
        salary_expense_account_id: a.salary_expense, salary_payable_account_id: a.salary_payable,
    }).await.unwrap();
    svc.add_salary_slip(run, NewSalarySlip {
        employee_id: Uuid::new_v4(), structure_id: structure,
        working_days: dec("22"), unpaid_days: dec("0"), statutory: statutory(&a),
    }).await.unwrap();
    svc.process_payroll_entry(run).await.unwrap();

    let sink = CountingGl::new();
    let first = svc.post_payroll_entry(run, today(), &sink, &LoggingSink).await.unwrap();
    let second = svc.post_payroll_entry(run, today(), &sink, &LoggingSink).await.unwrap();

    assert!(!first.already);
    assert!(second.already);
    assert_eq!(first.journal_id, second.journal_id);
    assert_eq!(sink.count(), 1, "the ledger is hit exactly once");
}

// PGC-5 — settlement-facing output (completeness council 2026-07-08): PayrollPosted carries the payable
// breakdown backbone-payments settles — the net-pay payable account + each statutory payable by account —
// so the consumer never has to re-query payroll's private slip tables to split total_deductions.
#[tokio::test]
async fn pgc5_payroll_posted_carries_payable_breakdown() {
    let pool = pool().await;
    let company = Uuid::new_v4();
    let a = payroll_accounts(&pool, company).await;
    let svc = PayrollWriteService::new(pool.clone());
    let structure = standard_structure(&svc, company, a.salary_expense).await;

    let run = svc.create_payroll_entry(NewPayrollEntry {
        company_id: company, period_year: 2026, period_month: 7,
        salary_expense_account_id: a.salary_expense, salary_payable_account_id: a.salary_payable,
    }).await.unwrap();
    svc.add_salary_slip(run, NewSalarySlip {
        employee_id: Uuid::new_v4(), structure_id: structure,
        working_days: dec("22"), unpaid_days: dec("0"), statutory: statutory(&a),
    }).await.unwrap();
    svc.process_payroll_entry(run).await.unwrap();

    let events = CapturingEvents::new();
    svc.post_payroll_entry(run, today(), &CountingGl::new(), &events).await.unwrap();
    let posted = events.last_posted();

    // Net pay clears the salary-payable account for total_net.
    assert_eq!(posted.salary_payable_account_id, a.salary_payable);
    assert_eq!(posted.total_net, dec("11260000"));

    // Each statutory payable is remitted to its own account — settlement can iterate.
    let bpjs = posted.payables.iter().find(|p| p.gl_account_id == a.bpjs_payable).expect("BPJS payable");
    let pph = posted.payables.iter().find(|p| p.gl_account_id == a.pph21_payable).expect("PPh21 payable");
    assert_eq!(bpjs.amount, dec("240000"));
    assert!(bpjs.statutory, "BPJS is a statutory remittance");
    assert_eq!(pph.amount, dec("500000"));
    assert!(pph.statutory, "PPh 21 is a statutory remittance");
    // The breakdown reconciles to the lump control total.
    let sum: Decimal = posted.payables.iter().map(|p| p.amount).sum();
    assert_eq!(sum, posted.total_deductions, "payables reconcile total_deductions");
}
