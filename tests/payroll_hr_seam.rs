//! The HR read seam against the REAL backbone-hr module — payroll's distinctive upstream. HR onboards an
//! employee, approves UNPAID leave, and reconciles it via `period_summary`; payroll consumes those
//! unpaid days to prorate gross. Proves the HR→payroll link end-to-end. ZERO normal Cargo edge to HR —
//! HR is a dev-dependency only, reached through its public service API (the ACL boundary).

mod common;
use common::*;

use backbone_hr::application::service::hr_events::{HrEvent, HrEventSink};
use backbone_hr::application::service::hr_ports::{DepartmentRef, HrRejected, OrgPort};
use backbone_hr::application::service::hr_write_service::*;
use backbone_payroll::application::service::payroll_write_service as pay;
use rust_decimal::Decimal;
use uuid::Uuid;

struct FakeOrg;
#[async_trait::async_trait]
impl OrgPort for FakeOrg {
    async fn resolve_department(&self, department_id: Uuid) -> Result<DepartmentRef, HrRejected> {
        Ok(DepartmentRef { department_id, company_id: Uuid::new_v4(), name: "Ops".into() })
    }
}
struct NoopHrSink;
impl HrEventSink for NoopHrSink {
    fn publish(&self, _event: &HrEvent) {}
}

fn ts(y: i32, m: u32, d: u32) -> chrono::DateTime<chrono::Utc> {
    chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, y, m, d, 0, 0, 0).unwrap()
}

// PHRSEAM-1 — approved unpaid leave in HR flows into payroll as prorated gross. An employee with a full
// 12,000,000 structure takes 2 unpaid days of a 22-day month; HR's period_summary reports 2 unpaid days;
// payroll prorates gross to 12,000,000 × 20/22 = 10,909,090.91.
#[tokio::test]
async fn phrseam1_unpaid_leave_prorates_payroll_gross() {
    let pool = pool().await;
    let company = Uuid::new_v4();
    let hr = HrWriteService::new(pool.clone());

    // Onboard a real employee in HR.
    let emp = hr.onboard_employee(
        NewEmployee {
            company_id: company, employee_number: format!("E-{}", &Uuid::new_v4().to_string()[..8]),
            user_id: None, department_id: None, first_name: "Budi".into(), last_name: Some("Santoso".into()),
            designation: Some("Staff".into()), employment_type: "permanent".into(),
            date_of_joining: ts(2020, 1, 1), nik: None, npwp: Some(npwp()), tax_status: "tk0".into(),
            bank_account_no: None, base_salary: dec("12000000"),
        },
        &FakeOrg, &NoopHrSink,
    ).await.expect("onboard");

    // Unpaid leave type, allocate, apply 2 days, approve → HR draws the balance.
    let unpaid_type = hr.create_leave_type(NewLeaveType {
        company_id: company, name: "Cuti Tanpa Gaji".into(), is_paid: false,
        annual_quota_days: dec("30"), allow_carry_forward: false,
    }).await.unwrap();
    hr.allocate_leave(company, emp, unpaid_type, 2026, dec("30")).await.unwrap();
    let app = hr.apply_leave(NewLeaveApplication {
        employee_id: emp, leave_type_id: unpaid_type,
        from_date: ts(2026, 7, 10), to_date: ts(2026, 7, 11), reason: None,
    }).await.unwrap();
    hr.approve_leave(app, None, ts(2026, 7, 5), &NoopHrSink).await.unwrap();

    // HR reconciles the period — the payroll-facing read.
    let summary = hr.period_summary(emp, ts(2026, 7, 1), ts(2026, 7, 31)).await.unwrap();
    assert_eq!(summary.unpaid_leave_days, dec("2"), "HR reports 2 unpaid days");
    let unpaid_days = summary.unpaid_leave_days + Decimal::from(summary.absent_days);

    // Payroll consumes the unpaid days to prorate gross.
    let a = payroll_accounts(&pool, company).await;
    let svc = pay::PayrollWriteService::new(pool.clone());
    let structure = svc.create_structure(pay::NewStructure {
        company_id: company, name: "Staff".into(),
        components: vec![pay::NewComponent {
            name: "Gaji Pokok".into(), component_type: "earning".into(),
            amount: dec("12000000"), gl_account_id: a.salary_expense,
        }],
    }).await.unwrap();
    let run = svc.create_payroll_entry(pay::NewPayrollEntry {
        company_id: company, period_year: 2026, period_month: 7,
        salary_expense_account_id: a.salary_expense, salary_payable_account_id: a.salary_payable,
    }).await.unwrap();
    let slip = svc.add_salary_slip(run, pay::NewSalarySlip {
        employee_id: emp, structure_id: structure, working_days: dec("22"), unpaid_days, statutory: vec![],
    }).await.unwrap();

    let gross = sqlx::query_scalar::<_, Decimal>("SELECT gross_pay FROM payroll.salary_slips WHERE id=$1")
        .bind(slip).fetch_one(&pool).await.unwrap();
    assert_eq!(gross, dec("10909090.91"), "HR unpaid days prorate payroll gross");
}
