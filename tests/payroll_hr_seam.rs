//! The HR→payroll seam against the SIX decomposed workforce modules — payroll's distinctive upstream.
//!
//! backbone-hr used to own the whole write side + the `period_summary` read; ADR-004 split it into
//! backbone-employee (onboard), backbone-timeoff (leave drawdown), backbone-attendance (presence),
//! and backbone-calendar (working-day denominator). This test rewires `phrseam1` against those four
//! modules + the three read-ports they now expose, derives `unpaid_days` from them, and proves payroll
//! gross still prorates correctly. A green run is the end-to-end proof that the decomposition works.
//!
//! The derived-absence formula (the new semantics — there is no single `period_summary` anymore):
//! ```text
//! unpaid_days = working_days − present_days − paid_leave_days
//! ```
//! Attendance owns "presence" (has a row = was present); absences are derived by subtraction, so the
//! test MUST seed attendance on every working day the employee was present, else those days count as
//! unpaid. ZERO normal Cargo edge to any workforce module — they are dev-dependencies only, reached
//! through their public service + query-port APIs (the ACL boundary).

mod common;
use common::*;

use std::sync::Arc;

use backbone_attendance::exports::AttendanceQueryService;
use backbone_calendar::exports::CalendarQueryService;
use backbone_timeoff::exports::TimeoffQueryService;

use backbone_employee::presentation::dto::{
    CreateEmployeeBpjsDto, CreateEmployeeDto, CreateEmploymentDto, CreateEmployeeTaxDto,
};
use backbone_employee::{
    EmployeeBpjsRepository, EmployeeBpjsService, EmployeeRepository, EmployeeService,
    EmployeeTaxRepository, EmployeeTaxService, EmploymentRepository, EmploymentService,
};

use backbone_attendance::presentation::dto::CreateAttendanceDto;
use backbone_attendance::{AttendanceRepository, AttendanceService};

use backbone_timeoff::presentation::dto::{
    CreateTimeoffBalanceDto, CreateTimeoffRequestDto, CreateTimeoffTypeDto,
};
use backbone_timeoff::application::service::TimeoffRequestWriteService;
use backbone_timeoff::{
    TimeoffBalanceRepository, TimeoffBalanceService, TimeoffRequestRepository, TimeoffRequestService,
    TimeoffTypeRepository, TimeoffTypeService,
};

use backbone_payroll::application::service::payroll_write_service as pay;

use chrono::{Datelike, NaiveDate};
use rust_decimal::{Decimal, RoundingStrategy};
use uuid::Uuid;

// PHRSEAM-1 — UNPAID leave flows into payroll as prorated gross, derived from the three decomposed
// read-ports. An employee with a 12,000,000 structure takes 2 UNPAID days on two consecutive working
// days of a 23-working-day July 2026 (Mon 2026-07-13 + Tue 2026-07-14); attendance is seeded on every
// OTHER working day (the new formula uses derived absence — no attendance row on a working day =
// unpaid), so working_days=23, present_days=21, paid_leave_days=0 → unpaid_days=2; payroll prorates
// gross to 12,000,000 × 21/23 = 10,956,521.74.
#[tokio::test]
async fn phrseam1_unpaid_leave_prorates_payroll_gross() {
    let pool = pool().await;
    let company = Uuid::new_v4();

    // ── Wire the three read-port modules. The query-port traits are implemented on the `Module`
    //    structs themselves, so the module instances ARE the read-side handles. (Module service
    //    fields are `pub(crate)`, so writes below use standalone CRUD services over the same pool.)
    let attendance = backbone_attendance::AttendanceModule::builder()
        .with_database(pool.clone())
        .build()
        .expect("attendance module");
    let timeoff = backbone_timeoff::TimeoffModule::builder()
        .with_database(pool.clone())
        .build()
        .expect("timeoff module");
    let calendar = backbone_calendar::CalendarModule::builder()
        .with_database(pool.clone())
        .build()
        .expect("calendar module");

    // ── Standalone CRUD services for the write side (the 4-layer write seam into each module).
    let employee_svc =
        EmployeeService::with_repository(Arc::new(EmployeeRepository::new(pool.clone())));
    let employment_svc =
        EmploymentService::with_repository(Arc::new(EmploymentRepository::new(pool.clone())));
    let attendance_svc =
        AttendanceService::with_repository(Arc::new(AttendanceRepository::new(pool.clone())));
    let ttype_svc =
        TimeoffTypeService::with_repository(Arc::new(TimeoffTypeRepository::new(pool.clone())));
    let tbalance_svc =
        TimeoffBalanceService::with_repository(Arc::new(TimeoffBalanceRepository::new(pool.clone())));
    let trequest_svc =
        TimeoffRequestService::with_repository(Arc::new(TimeoffRequestRepository::new(pool.clone())));

    // ── 1. Onboard one employee (via backbone-employee) + an employment row.
    let emp = employee_svc
        .create(CreateEmployeeDto {
            company_id: company,
            employee_number: format!("E-{}", &Uuid::new_v4().to_string()[..8]),
            user_id: None,
            first_name: "Budi".into(),
            last_name: Some("Santoso".into()),
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
        .expect("create employee");
    let _employment = employment_svc
        .create(CreateEmploymentDto {
            company_id: company,
            employee_id: emp.id,
            employment_status: Default::default(),
            join_date: NaiveDate::from_ymd_opt(2020, 1, 1).unwrap(),
            end_join_date: None,
            department_id: None,
            level_id: None,
            position_id: None,
            direct_manager_id: None,
            status: Default::default(),
        })
        .await
        .expect("create employment");

    // ── 2. Two consecutive WORKING days of UNPAID leave. Verified Mon 2026-07-13 + Tue 2026-07-14
    //    with chrono; asserted at runtime so a future date change can't silently break the scenario.
    let from = NaiveDate::from_ymd_opt(2026, 7, 1).unwrap();
    let to = NaiveDate::from_ymd_opt(2026, 7, 31).unwrap();
    let leave_start = NaiveDate::from_ymd_opt(2026, 7, 13).unwrap();
    let leave_end = NaiveDate::from_ymd_opt(2026, 7, 14).unwrap();
    assert_eq!(leave_start.weekday().num_days_from_monday(), 0, "leave_start is Monday");
    assert_eq!(leave_end.weekday().num_days_from_monday(), 1, "leave_end is Tuesday");

    // Unpaid leave type (is_paid=false), allocate 30d for 2026, request 2 days pending.
    let unpaid_type = ttype_svc
        .create(CreateTimeoffTypeDto {
            company_id: company,
            name: "Cuti Tanpa Gaji".into(),
            code: None,
            is_paid: false,
            allow_carry_forward: false,
        })
        .await
        .expect("create timeoff type");
    let _balance = tbalance_svc
        .create(CreateTimeoffBalanceDto {
            company_id: company,
            timeoff_type_id: unpaid_type.id,
            employee_id: emp.id,
            period: "2026".into(),
            allocated: dec("30"),
            used: dec("0"),
        })
        .await
        .expect("allocate timeoff balance");
    let request = trequest_svc
        .create(CreateTimeoffRequestDto {
            company_id: company,
            timeoff_type_id: unpaid_type.id,
            employee_id: emp.id,
            date_start: leave_start,
            date_end: leave_end,
            note: None,
            approval_employee_id: None,
            note_reject: None,
            status: backbone_timeoff::TimeoffRequestStatus::Pending,
        })
        .await
        .expect("create timeoff request");
    // Approve → draws down the balance in the same tx as pending→approved (gated so used ≤ allocated).
    TimeoffRequestWriteService::new(pool.clone())
        .approve_request(request.id, None)
        .await
        .expect("approve timeoff request");

    // ── 3. Seed attendance on every OTHER working day. This is the key semantic point: the new
    //    formula uses DERIVED absence (no attendance row on a working day = absent/unpaid), so the
    //    employee MUST have attendance records on the days they were present. Skip weekends and the
    //    two leave days → 23 working days − 2 leave days = 21 present rows.
    let mut cursor = from;
    while cursor <= to {
        let working_day = cursor.weekday().num_days_from_monday() < 5; // Mon=0 .. Fri=4
        let on_leave = cursor == leave_start || cursor == leave_end;
        if working_day && !on_leave {
            attendance_svc
                .create(CreateAttendanceDto {
                    company_id: company,
                    employee_id: emp.id,
                    date: cursor,
                    schedule: None,
                    clockin: None,
                    clockout: None,
                    time_debt: None,
                    timeoff: None,
                })
                .await
                .expect("seed attendance");
        }
        cursor = cursor.succ_opt().unwrap();
    }

    // ── 4. Derive unpaid_days from the THREE read-ports (the decomposed period_summary).
    let working_days = calendar.working_days(company, from, to).await.expect("working_days");
    let present = attendance
        .present_days(company, emp.id, from, to)
        .await
        .expect("present_days");
    let paid_leave = timeoff
        .paid_leave_days(company, emp.id, from, to)
        .await
        .expect("paid_leave_days");
    let unpaid_days =
        working_days as i64 - present.len() as i64 - paid_leave.len() as i64;

    assert_eq!(working_days, 23, "July 2026 Mon–Fri count (no holidays)");
    assert_eq!(present.len(), 21, "present on every working day except the 2 leave days");
    assert_eq!(paid_leave.len(), 0, "leave is UNPAID → 0 paid-leave days");
    assert_eq!(unpaid_days, 2, "unpaid_days = working_days − present_days − paid_leave_days");

    // ── 5. Drive payroll exactly as the seam contract does, prorating gross by the DERIVED
    //    unpaid_days. `working_days` is the calendar's actual count (23), not a hardcoded 22.
    let a = payroll_accounts(&pool, company).await;
    let svc = pay::PayrollWriteService::new(pool.clone());
    let structure = svc
        .create_structure(pay::NewStructure {
            company_id: company,
            name: "Staff".into(),
            components: vec![pay::NewComponent {
                name: "Gaji Pokok".into(),
                component_type: "earning".into(),
                amount: dec("12000000"),
                gl_account_id: a.salary_expense,
            }],
        })
        .await
        .unwrap();
    let run = svc
        .create_payroll_entry(pay::NewPayrollEntry {
            company_id: company,
            period_year: 2026,
            period_month: 7,
            salary_expense_account_id: a.salary_expense,
            salary_payable_account_id: a.salary_payable,
        })
        .await
        .unwrap();
    let working_days_dec = Decimal::from(working_days);
    let unpaid_days_dec = Decimal::from(unpaid_days);
    let slip = svc
        .add_salary_slip(
            run,
            pay::NewSalarySlip {
                employee_id: emp.id,
                structure_id: structure,
                working_days: working_days_dec,
                unpaid_days: unpaid_days_dec,
                statutory: vec![],
            },
        )
        .await
        .unwrap();

    let gross = sqlx::query_scalar::<_, Decimal>("SELECT gross_pay FROM payroll.salary_slips WHERE id=$1")
        .bind(slip)
        .fetch_one(&pool)
        .await
        .unwrap();

    // Expected = base × (working_days − unpaid_days) / working_days, rounded the same way payroll
    // rounds (2 dp, MidpointAwayFromZero). Computed from the ACTUAL working_days, not hardcoded.
    let factor = (working_days_dec - unpaid_days_dec) / working_days_dec;
    let expected_gross =
        (dec("12000000") * factor).round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero);
    assert_eq!(
        gross, expected_gross,
        "derived unpaid_days prorates payroll gross (12,000,000 × 21/23 = 10,956,521.74)"
    );
}

// PHRSEAM-2 — Indonesia statutory calcs (PPh 21 / BPJS / THR) flow through the employee read-port
// into the slip's `statutory` vec and produce real net pay. Onboard an employee WITH an NPWP + a BPJS
// row + a 12,000,000 salary structure; the EmployeeModule's `statutory_inputs` port returns the bundle
// (PTKP, has_npwp, join_date); `compute_statutory` turns it + the structure gross into the four
// statutory components (THR earning, PPh 21 / BPJS Kesehatan / BPJS TK deductions); the slip assembly
// adds THR to gross and subtracts the deductions → net = gross − Σ statutory deductions.
//
// Hand-computed (TK0 derived — no spouse/children, no override; has_npwp=true; gross 12M; risk class
// 3; tenure 78 months from 2020-01-01 to 2026-07 ≥ 12 → full THR):
//   base gross (no unpaid proration) = 12,000,000
//   THR earning                     = 12,000,000            (1× month, full tenure)
//   slip gross = base + THR         = 24,000,000
//   PPh 21 deduction                =    625,000            (TK0 + NPWP + 12M, verified in calcs)
//   BPJS Kesehatan deduction        =    120,000            (1% × min(12M, 12M cap))
//   BPJS TK employee deduction      =    345,474            (JHT 2%×12M=240,000 + JP 1%×10,547,400=105,474)
//   total statutory deductions      =  1,090,474
//   net_pay                         = 24,000,000 − 1,090,474 = 22,909,526.
#[tokio::test]
async fn phrseam2_statutory_drives_indonesian_net_pay() {
    let pool = pool().await;
    let company = Uuid::new_v4();

    // ── Employee module (the read-port host). Its query-port trait is impl'd on the Module itself.
    use backbone_employee::exports::EmployeeQueryService;
    let employee_module = backbone_employee::EmployeeModule::builder()
        .with_database(pool.clone())
        .build()
        .expect("employee module");

    // ── Standalone CRUD services for the write side (the 4-layer write seam into employee).
    let employee_svc =
        EmployeeService::with_repository(Arc::new(EmployeeRepository::new(pool.clone())));
    let employment_svc =
        EmploymentService::with_repository(Arc::new(EmploymentRepository::new(pool.clone())));
    let tax_svc =
        EmployeeTaxService::with_repository(Arc::new(EmployeeTaxRepository::new(pool.clone())));
    let bpjs_svc =
        EmployeeBpjsService::with_repository(Arc::new(EmployeeBpjsRepository::new(pool.clone())));

    // ── 1. Onboard the employee + an employment row (joined 2020-01-01 → ≥12mo tenure → full THR).
    let emp = employee_svc
        .create(CreateEmployeeDto {
            company_id: company,
            employee_number: format!("E-{}", &Uuid::new_v4().to_string()[..8]),
            user_id: None,
            first_name: "Siti".into(),
            last_name: Some("Wati".into()),
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
        .expect("create employee");
    let join_date = NaiveDate::from_ymd_opt(2020, 1, 1).unwrap();
    let _employment = employment_svc
        .create(CreateEmploymentDto {
            company_id: company,
            employee_id: emp.id,
            employment_status: Default::default(),
            join_date,
            end_join_date: None,
            department_id: None,
            level_id: None,
            position_id: None,
            direct_manager_id: None,
            status: Default::default(),
        })
        .await
        .expect("create employment");

    // ── 2. Tax row WITH an NPWP (has_npwp=true), no ptkp_override (PTKP derives to TK0: no family).
    let _tax = tax_svc
        .create(CreateEmployeeTaxDto {
            company_id: company,
            employee_id: emp.id,
            npwp_number: Some(npwp()),
            ptkp_override: None,
            tax_method: backbone_employee::TaxMethod::default(),
            tax_salary: backbone_employee::TaxSalary::default(),
            taxable_date: None,
            beginning_netto: None,
            pph21_paid: None,
        })
        .await
        .expect("create employee tax");

    // ── 3. BPJS row (Kesehatan family count = 1 — informational for the employee share).
    let _bpjs = bpjs_svc
        .create(CreateEmployeeBpjsDto {
            company_id: company,
            employee_id: emp.id,
            bpjs_ketenagakerjaan_number: None,
            npp_bpjs_ketenagakerjaan: None,
            bpjs_ketenagakerjaan_date: None,
            bpjs_kesehatan_number: None,
            bpjs_kesehatan_family: Some(1),
            bpjs_kesehatan_date: None,
            jaminan_pensiun_date: None,
        })
        .await
        .expect("create employee bpjs");

    // ── 4. Read the statutory bundle from the employee module. The port's read is RLS task-local
    //    scoped (it queries by employee_id alone), so wrap it in the company scope — correct for a
    //    production deployment and a harmless no-op when the test role bypasses RLS.
    use backbone_employee::exports::StatutoryInputs;
    let inputs: StatutoryInputs = backbone_orm::company_scope::with_company_scope(
        Some(company),
        async { employee_module.statutory_inputs(emp.id).await },
    )
    .await
    .expect("statutory_inputs read-port");

    assert_eq!(
        inputs.ptkp.to_string(),
        "tk0",
        "no spouse/children + no override → TK0 (interops with payroll's local PtkpTier via string)"
    );
    assert!(inputs.has_npwp, "npwp_number is set → has_npwp");
    assert_eq!(inputs.bpjs_kesehatan_family, Some(1));
    assert_eq!(inputs.join_date, Some(join_date), "join_date flows through the port");

    // ── 5. Compute the statutory components. `gross_monthly` is the structure's monthly earning
    //    (the salary actually being paid) — the employee module stores no queryable salary. Tenure is
    //    derived from join_date → the pay period (2026-07): (2026−2020)×12 + (7−1) = 78 months.
    let ptkp = inputs
        .ptkp
        .to_string()
        .parse::<backbone_payroll::application::service::PtkpTier>()
        .expect("employee PtkpTier interops with payroll's local PtkpTier");
    let (period_year, period_month) = (2026i32, 7u32);
    let join = inputs.join_date.expect("join_date present");
    let tenure_months = Decimal::from(
        (period_year - join.year()) * 12 + (period_month as i32 - join.month() as i32),
    );
    let gross_monthly = dec("12000000");
    let cfg = backbone_payroll::application::service::StatutoryConfig::default();
    let components = backbone_payroll::application::service::compute_statutory(
        ptkp,
        inputs.has_npwp,
        gross_monthly,
        3, // JKK risk class 3
        tenure_months,
        &cfg,
    )
    .expect("compute_statutory");

    // ── 6. Drive payroll: structure + slip with the statutory vec populated. Map each neutral
    //    StatutoryComponent to a StatutoryLine by attaching the GL account (payable for deductions,
    //    expense for the THR earning — the GL post debits salary_expense for the whole gross, so the
    //    earning line's account is not load-bearing for the journal).
    let a = payroll_accounts(&pool, company).await;
    let svc = pay::PayrollWriteService::new(pool.clone());
    let structure = svc
        .create_structure(pay::NewStructure {
            company_id: company,
            name: "Staff".into(),
            components: vec![pay::NewComponent {
                name: "Gaji Pokok".into(),
                component_type: "earning".into(),
                amount: gross_monthly,
                gl_account_id: a.salary_expense,
            }],
        })
        .await
        .unwrap();
    let run = svc
        .create_payroll_entry(pay::NewPayrollEntry {
            company_id: company,
            period_year: 2026,
            period_month: 7,
            salary_expense_account_id: a.salary_expense,
            salary_payable_account_id: a.salary_payable,
        })
        .await
        .unwrap();

    let gl_for = |name: &str| -> Uuid {
        match name {
            "PPh 21" => a.pph21_payable,
            _ => a.bpjs_payable, // BPJS Kesehatan + BPJS Ketenagakerjaan share the BPJS payable
        }
    };
    let statutory_lines: Vec<pay::StatutoryLine> = components
        .iter()
        .map(|c| pay::StatutoryLine {
            name: c.name.clone(),
            component_type: c.component_type.clone(),
            amount: c.amount,
            gl_account_id: if c.component_type == "earning" {
                a.salary_expense
            } else {
                gl_for(&c.name)
            },
        })
        .collect();

    let slip = svc
        .add_salary_slip(
            run,
            pay::NewSalarySlip {
                employee_id: emp.id,
                structure_id: structure,
                working_days: Decimal::from(23),
                unpaid_days: Decimal::ZERO, // no proration — isolates the statutory effect on net
                statutory: statutory_lines,
            },
        )
        .await
        .unwrap();

    // ── 7. Assert net_pay == gross − Σ statutory deductions, with the hand-computed values.
    #[derive(sqlx::FromRow)]
    struct SlipTotals {
        gross_pay: Decimal,
        total_deductions: Decimal,
        net_pay: Decimal,
    }
    let t: SlipTotals = sqlx::query_as(
        "SELECT gross_pay, total_deductions, net_pay FROM payroll.salary_slips WHERE id=$1",
    )
    .bind(slip)
    .fetch_one(&pool)
    .await
    .unwrap();

    let thr = dec("12000000");
    let pph21_expected = dec("625000");
    let bpjs_kes = dec("120000");
    let bpjs_tk = dec("345474");
    let total_statutory_deductions = pph21_expected + bpjs_kes + bpjs_tk; // 1,090,474
    let expected_gross = gross_monthly + thr; // base + THR earning = 24,000,000
    let expected_net = expected_gross - total_statutory_deductions; // 22,909,526

    assert_eq!(t.gross_pay, expected_gross, "gross = base 12M + THR 12M (unpaid=0, no proration)");
    assert_eq!(t.total_deductions, total_statutory_deductions, "Σ statutory deductions");
    assert_eq!(
        t.net_pay, expected_net,
        "net_pay == gross − Σ statutory deductions (24,000,000 − 1,090,474 = 22,909,526)"
    );
    assert_eq!(expected_net, dec("22909526"), "net pay is the Indonesian take-home");

    // ── 8. Spot-check the PPh 21 slip line == 625,000 (the task's named oracle for TK0+NPWP+12M).
    let pph21_line: Option<Decimal> = sqlx::query_scalar(
        "SELECT amount FROM payroll.salary_slip_lines \
         WHERE salary_slip_id=$1 AND name='PPh 21' AND is_statutory=true",
    )
    .bind(slip)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pph21_line, Some(pph21_expected), "PPh 21 monthly withholding for TK0+NPWP+12M");

    // THR is an earning slip-line with is_statutory=true (not grouped as a deduction payable).
    let thr_is_earning: (bool, bool) = sqlx::query_as(
        "SELECT component_type='earning'::component_type, is_statutory FROM payroll.salary_slip_lines \
         WHERE salary_slip_id=$1 AND name='THR'",
    )
    .bind(slip)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(thr_is_earning, (true, true), "THR is a statutory earning line");
}
