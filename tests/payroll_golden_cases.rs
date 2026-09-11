//! Golden cases — the manufactured oracle for the salary run: exact gross→deductions→net math,
//! HR unpaid-day proration, run roll-up, deduction grouping, and post-once idempotency. Money is IDR,
//! 2dp, half-away-from-zero. These assert the DOMAIN math via a capturing sink; the REAL-ledger balance
//! and posting-source acceptance live in payroll_gl_seam.rs.

mod common;
use common::*;

use backbone_payroll::application::service::payroll_events::LoggingSink;
use backbone_payroll::application::service::payroll_write_service::*;
use backbone_payroll::application::service::statutory_calcs::StatutoryError;
use backbone_payroll::infrastructure::persistence::StatutoryParamsRepository;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use uuid::Uuid;

fn earning(name: &str, amt: &str, acct: Uuid) -> NewComponent {
    NewComponent { name: name.into(), component_type: "earning".into(), amount: dec(amt), gl_account_id: acct }
}

/// Build the standard structure: Gaji Pokok 10,000,000 + Tunjangan 2,000,000 = 12,000,000 gross.
async fn standard_structure(svc: &PayrollWriteService, expense: Uuid) -> Uuid {
    svc.create_structure(NewStructure {
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
        StatutoryLine { name: "BPJS".into(), component_type: "deduction".into(), amount: dec("240000"), gl_account_id: a.bpjs_payable },
        StatutoryLine { name: "PPh 21".into(), component_type: "deduction".into(), amount: dec("500000"), gl_account_id: a.pph21_payable },
    ]
}

// PGC-1 — full-month net pay: gross 12,000,000 − (BPJS 240,000 + PPh21 500,000) = 11,260,000.
#[tokio::test]
async fn pgc1_full_month_net_pay() {
    let pool = pool().await;
    let a = payroll_accounts(&pool).await;
    let svc = PayrollWriteService::new(pool.clone());
    let structure = standard_structure(&svc, a.salary_expense).await;

    let run = svc.create_payroll_entry(NewPayrollEntry {
        period_year: 2026, period_month: 7,
        salary_expense_account_id: a.salary_expense, salary_payable_account_id: a.salary_payable,
    }).await.unwrap();

    svc.add_salary_slip(run, NewSalarySlip {
        employee_id: Uuid::new_v4(), structure_id: structure,
        working_days: dec("22"), unpaid_days: dec("0"), overtime_hours: dec("0"), tax_method: None, statutory: statutory(&a),
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
    let a = payroll_accounts(&pool).await;
    let svc = PayrollWriteService::new(pool.clone());
    let structure = standard_structure(&svc, a.salary_expense).await;

    let run = svc.create_payroll_entry(NewPayrollEntry {
        period_year: 2026, period_month: 7,
        salary_expense_account_id: a.salary_expense, salary_payable_account_id: a.salary_payable,
    }).await.unwrap();

    let slip = svc.add_salary_slip(run, NewSalarySlip {
        employee_id: Uuid::new_v4(), structure_id: structure,
        working_days: dec("22"), unpaid_days: dec("2"), overtime_hours: dec("0"), tax_method: None, statutory: statutory(&a),
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
    let a = payroll_accounts(&pool).await;
    let svc = PayrollWriteService::new(pool.clone());
    let structure = standard_structure(&svc, a.salary_expense).await;

    let run = svc.create_payroll_entry(NewPayrollEntry {
        period_year: 2026, period_month: 7,
        salary_expense_account_id: a.salary_expense, salary_payable_account_id: a.salary_payable,
    }).await.unwrap();

    for _ in 0..2 {
        svc.add_salary_slip(run, NewSalarySlip {
            employee_id: Uuid::new_v4(), structure_id: structure,
            working_days: dec("22"), unpaid_days: dec("0"), overtime_hours: dec("0"), tax_method: None, statutory: statutory(&a),
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
    let a = payroll_accounts(&pool).await;
    let svc = PayrollWriteService::new(pool.clone());
    let structure = standard_structure(&svc, a.salary_expense).await;

    let run = svc.create_payroll_entry(NewPayrollEntry {
        period_year: 2026, period_month: 7,
        salary_expense_account_id: a.salary_expense, salary_payable_account_id: a.salary_payable,
    }).await.unwrap();
    svc.add_salary_slip(run, NewSalarySlip {
        employee_id: Uuid::new_v4(), structure_id: structure,
        working_days: dec("22"), unpaid_days: dec("0"), overtime_hours: dec("0"), tax_method: None, statutory: statutory(&a),
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
    let a = payroll_accounts(&pool).await;
    let svc = PayrollWriteService::new(pool.clone());
    let structure = standard_structure(&svc, a.salary_expense).await;

    let run = svc.create_payroll_entry(NewPayrollEntry {
        period_year: 2026, period_month: 7,
        salary_expense_account_id: a.salary_expense, salary_payable_account_id: a.salary_payable,
    }).await.unwrap();
    svc.add_salary_slip(run, NewSalarySlip {
        employee_id: Uuid::new_v4(), structure_id: structure,
        working_days: dec("22"), unpaid_days: dec("0"), overtime_hours: dec("0"), tax_method: None, statutory: statutory(&a),
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

/// Seed one complete, internally-consistent parameter set for `cc` at `effective` — complete in
/// the resolver's sense: brackets opening at zero and closing open-ended, ALL eight PTKP tiers,
/// ALL three TER categories each opening at zero, the BPJS matrix the calcs read, two workday
/// overtime bands. A per-test fictional country code isolates the rows from the seeded "ID" data
/// and from parallel tests.
async fn seed_params(pool: &sqlx::PgPool, cc: &str, effective: NaiveDate) {
    sqlx::query("INSERT INTO payroll.pph21_brackets (country_code, effective_from, seq, lower_bound, upper_bound, rate) VALUES ($1,$2,1,0,NULL,0.05)")
        .bind(cc).bind(effective).execute(pool).await.unwrap();
    for (tier, amount) in [
        ("tk0", "54000000"), ("tk1", "58500000"), ("tk2", "63000000"), ("tk3", "67500000"),
        ("k0", "58500000"), ("k1", "63000000"), ("k2", "67500000"), ("k3", "72000000"),
    ] {
        sqlx::query("INSERT INTO payroll.pph21_ptkp (country_code, effective_from, tier, annual_amount) VALUES ($1,$2,$3,$4)")
            .bind(cc).bind(effective).bind(tier).bind(amount.parse::<Decimal>().unwrap())
            .execute(pool).await.unwrap();
    }
    for category in ["ter_a", "ter_b", "ter_c"] {
        sqlx::query("INSERT INTO payroll.pph21_ter_rates (country_code, effective_from, category, seq, lower_bound, rate) VALUES ($1,$2,$3,1,0,0.01)")
            .bind(cc).bind(effective).bind(category).execute(pool).await.unwrap();
    }
    for (component, side, rate, cap) in [
        ("kes", "employee", dec("0.01"), Some(dec("12000000"))),
        ("kes", "employer", dec("0.04"), Some(dec("12000000"))),
        ("jht", "employee", dec("0.02"), None),
        ("jht", "employer", dec("0.037"), None),
        ("jp", "employee", dec("0.01"), Some(dec("10547400"))),
        ("jp", "employer", dec("0.02"), Some(dec("10547400"))),
        ("jkk_3", "employer", dec("0.0024"), None),
        ("jkm", "employer", dec("0.003"), None),
    ] {
        sqlx::query("INSERT INTO payroll.bpjs_params (country_code, effective_from, component, side, rate, wage_cap) VALUES ($1,$2,$3,$4,$5,$6)")
            .bind(cc).bind(effective).bind(component).bind(side).bind(rate)
            .bind(cap)
            .execute(pool).await.unwrap();
    }
    for (hour_from, hour_to, multiplier) in [(1, Some(1), dec("1.5")), (2, None, dec("2.0"))] {
        sqlx::query("INSERT INTO payroll.overtime_params (country_code, effective_from, day_kind, hour_from, hour_to, multiplier) VALUES ($1,$2,'workday',$3,$4,$5)")
            .bind(cc).bind(effective).bind(hour_from).bind(hour_to).bind(multiplier)
            .execute(pool).await.unwrap();
    }
}

// PGC-6 — as-of resolution edges: a period before ANY effective row refuses (fail-closed, never a
// silent zero tax); the set in force at `as_of` is the greatest effective_from <= it; a NEW
// effective set is invisible until its own date — so a mid-year law change lands on the first run
// of the month it takes effect, never retroactively.
#[tokio::test]
async fn pgc6_params_resolve_as_of_effective_from() {
    let pool = pool().await;
    let cc = format!("T{}", &Uuid::new_v4().to_string()[..8]);
    let repo = StatutoryParamsRepository::new(pool.clone());
    let d = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();

    seed_params(&pool, &cc, d(2025, 1, 1)).await;

    // Before any effective row → fail closed.
    let pre = repo.resolve_as_of(&cc, d(2024, 12, 31)).await;
    assert!(
        matches!(pre, Err(StatutoryError::NoParamsForPeriod(..))),
        "a period before any effective row must refuse, not zero the tax"
    );

    // On/after the effective date → the set resolves and drives the calcs.
    let cfg = repo.resolve_as_of(&cc, d(2025, 1, 1)).await.expect("resolve at effective date");
    let mults: Vec<Decimal> = cfg.overtime.workday.iter().map(|b| b.multiplier).collect();
    assert_eq!(mults, vec![dec("1.5"), dec("2.0")], "the 2025-01-01 workday bands");
    assert!(cfg.pph21.ter.contains_key("ter_a"), "TER bands keyed by category");

    // A new effective overtime set from 2026-06-01: invisible the day before, in force from its
    // own date (a June run prices overtime under it; a May run never does).
    let d2 = d(2026, 6, 1);
    for (hour_from, hour_to, multiplier) in [(1, Some(1), dec("2.0")), (2, None, dec("3.0"))] {
        sqlx::query("INSERT INTO payroll.overtime_params (country_code, effective_from, day_kind, hour_from, hour_to, multiplier) VALUES ($1,$2,'workday',$3,$4,$5)")
            .bind(&cc).bind(d2).bind(hour_from).bind(hour_to).bind(multiplier)
            .execute(&pool).await.unwrap();
    }
    let may = repo.resolve_as_of(&cc, d(2026, 5, 31)).await.expect("may resolve");
    let june = repo.resolve_as_of(&cc, d(2026, 6, 1)).await.expect("june resolve");
    let m: Vec<Decimal> = may.overtime.workday.iter().map(|b| b.multiplier).collect();
    let j: Vec<Decimal> = june.overtime.workday.iter().map(|b| b.multiplier).collect();
    assert_eq!(m, vec![dec("1.5"), dec("2.0")], "the day before a new effective set: old law");
    assert_eq!(j, vec![dec("2.0"), dec("3.0")], "from its effective date: the new set applies");
}

// PGC-7 — the computed-slip orchestrator refuses a period with NO effective parameters (a run
// dated before the seeded law tables) with the stable 422 `no_statutory_params_for_period` code —
// the fail-closed contract the guarded surface surfaces. Parameter resolution precedes the
// employee lookup, so no employee fixture is needed to reach the refusal.
#[tokio::test]
async fn pgc7_pre_effective_period_run_refuses_to_compute() {
    let pool = pool().await;
    let a = payroll_accounts(&pool).await;
    let svc = PayrollWriteService::new(pool.clone());
    let structure = standard_structure(&svc, a.salary_expense).await;

    let run = svc.create_payroll_entry(NewPayrollEntry {
        period_year: 2021, period_month: 12, // before the 2022-01-01 seeds
        salary_expense_account_id: a.salary_expense, salary_payable_account_id: a.salary_payable,
    }).await.unwrap();

    let r = svc.add_computed_salary_slip(ComputedSlipRequest {
        run_id: run,
        employee_id: Uuid::new_v4(),
        structure_id: structure,
        working_days: dec("22"),
        unpaid_days: dec("0"),
        risk_class: 3,
        accounts: StatutoryAccounts {
            pph21_payable: a.pph21_payable,
            bpjs_kesehatan_payable: a.bpjs_payable,
            bpjs_ketenagakerjaan_payable: a.bpjs_payable,
        },
    }).await;

    match r {
        Err(e) => {
            assert_eq!(e.code(), "no_statutory_params_for_period", "stable refusal code");
            assert_eq!(e.http_status(), 422, "client-shaped refusal, not a 500");
        }
        Ok(_) => panic!("a pre-effective period must refuse to compute a slip"),
    }
}

// PGC-8 — a LONE correction row refuses the period instead of applying: the correction protocol
// restates the COMPLETE set at a new effective date, so seeding only the changed row leaves the
// set at that date incomplete. The dangerous alternative — a lone top bracket silently zero-taxing
// everyone below it via the progressive walk's break — is exactly what the completeness checks
// turn back into the fail-closed refusal.
#[tokio::test]
async fn pgc8_lone_correction_rows_refuse_the_period() {
    let pool = pool().await;
    let repo = StatutoryParamsRepository::new(pool.clone());
    let d = |y, m, d| NaiveDate::from_ymd_opt(y, m, d).unwrap();
    let fresh = || format!("T{}", &Uuid::new_v4().to_string()[..8]);
    let t1 = d(2025, 1, 1);
    let t2 = d(2026, 1, 1);

    // A lone TOP-bracket correction at t2 — the set no longer opens at income zero.
    let cc = fresh();
    seed_params(&pool, &cc, t1).await;
    sqlx::query("INSERT INTO payroll.pph21_brackets (country_code, effective_from, seq, lower_bound, upper_bound, rate) VALUES ($1,$2,5,5000000000,NULL,0.35)")
        .bind(&cc).bind(t2).execute(&pool).await.unwrap();
    assert!(
        matches!(repo.resolve_as_of(&cc, t2).await, Err(StatutoryError::NoParamsForPeriod(..))),
        "a lone top-bracket correction must refuse, never zero-tax below it"
    );

    // A lone PTKP tier at t2 — seven of the eight canonical tiers are missing.
    let cc = fresh();
    seed_params(&pool, &cc, t1).await;
    sqlx::query("INSERT INTO payroll.pph21_ptkp (country_code, effective_from, tier, annual_amount) VALUES ($1,$2,'tk1',58500000)")
        .bind(&cc).bind(t2).execute(&pool).await.unwrap();
    assert!(
        matches!(repo.resolve_as_of(&cc, t2).await, Err(StatutoryError::NoParamsForPeriod(..))),
        "a lone PTKP tier must refuse rather than 500 on the first affected employee"
    );

    // A lone TER category at t2 — the other two categories are absent at that date.
    let cc = fresh();
    seed_params(&pool, &cc, t1).await;
    sqlx::query("INSERT INTO payroll.pph21_ter_rates (country_code, effective_from, category, seq, lower_bound, rate) VALUES ($1,$2,'ter_b',1,0,0.05)")
        .bind(&cc).bind(t2).execute(&pool).await.unwrap();
    assert!(
        matches!(repo.resolve_as_of(&cc, t2).await, Err(StatutoryError::NoParamsForPeriod(..))),
        "a lone TER category must refuse the period"
    );
}
