//! Integrity probes — the run engine's invariants: net pay never goes negative, no duplicate slip per
//! employee, the draft→processed→posted transition gates hold against out-of-order calls, the
//! fail-closed seams refuse with stable codes, and remittance is idempotent-keyed per payable.

mod common;
use common::*;

use std::sync::{Arc, Mutex};

use backbone_payroll::application::service::payroll_events::LoggingSink;
use backbone_payroll::application::service::payroll_remittance::{
    RemitAck, RemittanceInstruction, RemittanceSink,
};
use backbone_payroll::application::service::payroll_write_service::*;
use uuid::Uuid;

/// Records every remittance instruction it receives (and acks each) so probes can assert the
/// payable set + the idempotency keys payroll derived.
#[derive(Clone, Default)]
struct CapturingRemit {
    seen: Arc<Mutex<Vec<RemittanceInstruction>>>,
}
impl CapturingRemit {
    fn new() -> Self {
        Self::default()
    }
}
#[async_trait::async_trait]
impl RemittanceSink for CapturingRemit {
    async fn remit(&self, i: &RemittanceInstruction) -> Result<RemitAck, backbone_payroll::application::service::payroll_remittance::RemittanceSeamError> {
        self.seen.lock().unwrap().push(i.clone());
        Ok(RemitAck { payment_id: Some(Uuid::new_v4()), duplicate: false })
    }
}

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
        overtime_hours: dec("0"),
        tax_method: None,
        statutory: vec![StatutoryLine { name: "Loan".into(), component_type: "deduction".into(), amount: dec("6000000"), gl_account_id: a.bpjs_payable }],
    }).await;
    assert!(matches!(r, Err(PayrollError::Invalid(_))), "deductions > gross must be rejected");
}

// PIP-2 — an employee can appear at most once in a run.
#[tokio::test]
async fn pip2_no_duplicate_slip_per_employee() {
    let (_pool, company, a, svc, structure) = setup().await;
    let run = svc.create_payroll_entry(new_run(company, &a)).await.unwrap();
    let emp = Uuid::new_v4();
    let slip = NewSalarySlip { employee_id: emp, structure_id: structure, working_days: dec("22"), unpaid_days: dec("0"), overtime_hours: dec("0"), tax_method: None, statutory: vec![] };
    svc.add_salary_slip(run, NewSalarySlip { ..clone_slip(&slip) }).await.unwrap();
    let dup = svc.add_salary_slip(run, slip).await;
    assert!(matches!(dup, Err(PayrollError::Invalid(_))), "duplicate employee in a run must be rejected");
}

fn clone_slip(s: &NewSalarySlip) -> NewSalarySlip {
    NewSalarySlip { employee_id: s.employee_id, structure_id: s.structure_id, working_days: s.working_days, unpaid_days: s.unpaid_days, overtime_hours: dec("0"), tax_method: None, statutory: vec![] }
}

// PIP-3 — cannot post a run that has not been processed (still draft).
#[tokio::test]
async fn pip3_cannot_post_unprocessed_run() {
    let (_pool, company, a, svc, structure) = setup().await;
    let run = svc.create_payroll_entry(new_run(company, &a)).await.unwrap();
    svc.add_salary_slip(run, NewSalarySlip {
        employee_id: Uuid::new_v4(), structure_id: structure, working_days: dec("22"), unpaid_days: dec("0"), overtime_hours: dec("0"), tax_method: None, statutory: vec![],
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
        employee_id: Uuid::new_v4(), structure_id: structure, working_days: dec("22"), unpaid_days: dec("0"), overtime_hours: dec("0"), tax_method: None, statutory: vec![],
    }).await.unwrap();
    svc.process_payroll_entry(run).await.unwrap();

    let reprocess = svc.process_payroll_entry(run).await;
    assert!(matches!(reprocess, Err(PayrollError::InvalidState(_))), "cannot re-process");

    let late_slip = svc.add_salary_slip(run, NewSalarySlip {
        employee_id: Uuid::new_v4(), structure_id: structure, working_days: dec("22"), unpaid_days: dec("0"), overtime_hours: dec("0"), tax_method: None, statutory: vec![],
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
        working_days: dec("22"), unpaid_days: dec("-5"), overtime_hours: dec("0"), tax_method: None, statutory: vec![],
    }).await.expect("negative unpaid days must be clamped, not rejected mid-insert");
    let gross = sqlx::query_scalar::<_, rust_decimal::Decimal>(
        "SELECT gross_pay FROM payroll.salary_slips WHERE id=$1")
        .bind(slip).fetch_one(&pool).await.unwrap();
    assert!(gross <= dec("5000000"), "gross must never exceed the structure base (got {gross})");
}

// PIP-8 — remittance is a POSTED-run verb: draft and processed both refuse (there is nothing final
// to pay yet — deductions may still change).
#[tokio::test]
async fn pip8_remit_requires_a_posted_run() {
    let (_pool, company, a, svc, structure) = setup().await;
    let run = svc.create_payroll_entry(new_run(company, &a)).await.unwrap();
    let draft = svc.remit_payroll_entry(run, &CapturingRemit::new()).await;
    assert!(matches!(draft, Err(PayrollError::InvalidState(_))), "draft run cannot remit");

    svc.add_salary_slip(run, NewSalarySlip {
        employee_id: Uuid::new_v4(), structure_id: structure, working_days: dec("22"), unpaid_days: dec("0"), overtime_hours: dec("0"), tax_method: None,
        statutory: vec![StatutoryLine { name: "BPJS".into(), component_type: "deduction".into(), amount: dec("240000"), gl_account_id: a.bpjs_payable }],
    }).await.unwrap();
    svc.process_payroll_entry(run).await.unwrap();
    let processed = svc.remit_payroll_entry(run, &CapturingRemit::new()).await;
    assert!(matches!(processed, Err(PayrollError::InvalidState(_))), "processed-but-unposted run cannot remit");
}

// PIP-9 — the module-held seams fail closed with their stable codes (and the post verb leaves the
// run processed/retryable): an unwired GL post refuses `gl_seam_unwired`, an unwired remittance
// refuses `remittance_seam_unwired` — both 422, never a silent no-op effect.
#[tokio::test]
async fn pip9_unwired_seams_refuse_with_stable_codes() {
    let (pool, company, a, svc, structure) = setup().await;

    // GL: post through the module-held (default-Unwired) sink.
    let run = svc.create_payroll_entry(new_run(company, &a)).await.unwrap();
    svc.add_salary_slip(run, NewSalarySlip {
        employee_id: Uuid::new_v4(), structure_id: structure, working_days: dec("22"), unpaid_days: dec("0"), overtime_hours: dec("0"), tax_method: None,
        statutory: vec![StatutoryLine { name: "BPJS".into(), component_type: "deduction".into(), amount: dec("240000"), gl_account_id: a.bpjs_payable }],
    }).await.unwrap();
    svc.process_payroll_entry(run).await.unwrap();
    let gl = svc.post_run(run, today()).await;
    match &gl {
        Err(e) => {
            assert_eq!(e.code(), "gl_seam_unwired", "stable GL seam code");
            assert_eq!(e.http_status(), 422);
        }
        Ok(_) => panic!("an unwired GL sink must refuse the post"),
    }
    let status: String = sqlx::query_scalar("SELECT status::text FROM payroll.payroll_entries WHERE id=$1")
        .bind(run).fetch_one(&pool).await.unwrap();
    assert_eq!(status, "processed", "a refused post leaves the run processed and retryable");

    // Remittance: post for real through explicit sinks, then remit through the module-held
    // (default-Unwired) seam.
    svc.post_payroll_entry(run, today(), &CountingGl::new(), &LoggingSink).await.unwrap();
    let remit = svc.remit_run(run).await;
    match &remit {
        Err(e) => {
            assert_eq!(e.code(), "remittance_seam_unwired", "stable remittance seam code");
            assert_eq!(e.http_status(), 422);
        }
        Ok(_) => panic!("an unwired remittance sink must refuse the remit"),
    }
}

// PIP-10 — remittance derives one instruction per statutory payable with a stable idempotency key
// (`payroll_remittance:{company}:{run}:{account}`) so a payment adapter can dedup a re-driven remit;
// the net-pay account is NOT a remittance payable (settlement pays it separately).
#[tokio::test]
async fn pip10_remit_idempotency_key_covers_each_payable() {
    let (_pool, company, a, svc, structure) = setup().await;
    let run = svc.create_payroll_entry(new_run(company, &a)).await.unwrap();
    svc.add_salary_slip(run, NewSalarySlip {
        employee_id: Uuid::new_v4(), structure_id: structure, working_days: dec("22"), unpaid_days: dec("0"), overtime_hours: dec("0"), tax_method: None,
        statutory: vec![
            StatutoryLine { name: "BPJS".into(), component_type: "deduction".into(), amount: dec("240000"), gl_account_id: a.bpjs_payable },
            StatutoryLine { name: "PPh 21".into(), component_type: "deduction".into(), amount: dec("500000"), gl_account_id: a.pph21_payable },
        ],
    }).await.unwrap();
    svc.process_payroll_entry(run).await.unwrap();
    svc.post_payroll_entry(run, today(), &CountingGl::new(), &LoggingSink).await.unwrap();

    let sink = CapturingRemit::new();
    let out = svc.remit_payroll_entry(run, &sink).await.unwrap();
    assert_eq!(out.payroll_entry_id, run);
    assert_eq!(out.remitted.len(), 2, "one instruction per statutory payable");

    let seen = sink.seen.lock().unwrap();
    for (i, _ack) in &out.remitted {
        let instr = seen.iter().find(|s| s.idempotency_key == i.idempotency_key).expect("acked instruction was seen");
        assert_eq!(
            instr.idempotency_key,
            format!("payroll_remittance:{company}:{run}:{}", instr.gl_account_id),
            "stable per-payable key a payment adapter can dedup on"
        );
        assert!(instr.statutory, "deduction payables are statutory remittances");
        assert_ne!(instr.gl_account_id, a.salary_payable, "the net-pay account is never a remittance payable");
    }
    let total: rust_decimal::Decimal = seen.iter().map(|i| i.amount).sum();
    assert_eq!(total, dec("740000"), "the payable set is the run's grouped deductions");
}

// PIP-11 — re-posting a POSTED run re-publishes the event (at-least-once: a sink hiccup after the
// post landed is recoverable by re-running the verb) while the GL sink still sees exactly ONE post.
#[tokio::test]
async fn pip11_already_posted_run_republishes_the_event() {
    let (_pool, company, a, svc, structure) = setup().await;
    let run = svc.create_payroll_entry(new_run(company, &a)).await.unwrap();
    svc.add_salary_slip(run, NewSalarySlip {
        employee_id: Uuid::new_v4(), structure_id: structure, working_days: dec("22"), unpaid_days: dec("0"), overtime_hours: dec("0"), tax_method: None, statutory: vec![],
    }).await.unwrap();
    svc.process_payroll_entry(run).await.unwrap();

    let gl = CountingGl::new();
    let events = CapturingEvents::new();
    let first = svc.post_payroll_entry(run, today(), &gl, &events).await.unwrap();
    let second = svc.post_payroll_entry(run, today(), &gl, &events).await.unwrap();

    assert!(!first.already && second.already, "second call takes the already-posted path");
    assert_eq!(gl.count(), 1, "the ledger is hit exactly once");
    assert_eq!(events.events.lock().unwrap().len(), 2, "the already path RE-publishes the posted event");
}

// PIP-12 — the computed-slip verb's employee read is company-scoped: an employee id belonging to
// ANOTHER company yields 404 (the port's live-employee predicate carries the tenant), so a
// cross-tenant employee reference cannot produce a slip in this company's run even before the DB
// fence is considered.
#[tokio::test]
async fn pip12_cross_tenant_employee_reference_is_not_found() {
    let (pool, company, a, svc, structure) = setup().await;
    let run = svc.create_payroll_entry(new_run(company, &a)).await.unwrap();

    // A real, live employee row — in a DIFFERENT company.
    let other_company = Uuid::new_v4();
    let employee_svc = backbone_employee::EmployeeService::with_repository(Arc::new(
        backbone_employee::EmployeeRepository::new(pool.clone()),
    ));
    let outsider = employee_svc
        .create(backbone_employee::presentation::dto::CreateEmployeeDto {
            company_id: other_company,
            employee_number: format!("E-{}", &Uuid::new_v4().to_string()[..8]),
            user_id: None,
            first_name: "Other".into(),
            last_name: None,
            email: None,
            mobile_phone: None,
            phone: None,
            birth_place: None,
            birth_date: None,
            gender: None,
            marital_status: None,
            blood_type: None,
            religion_id: None,
        })
        .await
        .unwrap();

    let r = svc
        .add_computed_salary_slip(ComputedSlipRequest {
            run_id: run,
            employee_id: outsider.id,
            structure_id: structure,
            working_days: dec("22"),
            unpaid_days: dec("0"),
            risk_class: 3,
            accounts: StatutoryAccounts {
                pph21_payable: a.pph21_payable,
                bpjs_kesehatan_payable: a.bpjs_payable,
                bpjs_ketenagakerjaan_payable: a.bpjs_payable,
            },
        })
        .await;
    match r {
        Err(e) => {
            assert_eq!(e.code(), "not_found", "a cross-tenant employee reads as absent");
            assert_eq!(e.http_status(), 404);
        }
        Ok(_) => panic!("a cross-tenant employee must not produce a slip"),
    }
}
