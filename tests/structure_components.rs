//! A slip is built from a structure's earnings and fixed deductions only. A component marked
//! statutory (BPJS, PPh 21) is a marker that the rule applies: the amount comes from the statutory
//! lines the run computes, never from the structure, so it is not charged a second time. A removed
//! (soft-deleted) component is not paid at all.

mod common;
use common::*;

use backbone_payroll::application::service::payroll_write_service::*;
use rust_decimal::Decimal;
use uuid::Uuid;

#[allow(clippy::too_many_arguments)]
async fn add_component(
    pool: &sqlx::PgPool,
    structure: Uuid,
    name: &str,
    kind: &str,
    amount: &str,
    account: Uuid,
    statutory: bool,
    deleted: bool,
) {
    sqlx::query(
        r#"INSERT INTO payroll.salary_components
             (id, structure_id, name, component_type, amount, gl_account_id, is_statutory, metadata)
           VALUES ($1, $2, $3, $4::component_type, $5, $6, $7,
                   jsonb_build_object('created_at', now(), 'deleted_at',
                                      CASE WHEN $8 THEN now() ELSE NULL END))"#,
    )
    .bind(Uuid::new_v4())
    .bind(structure)
    .bind(name)
    .bind(kind)
    .bind(dec(amount))
    .bind(account)
    .bind(statutory)
    .bind(deleted)
    .execute(pool)
    .await
    .expect("component");
}

#[tokio::test]
async fn statutory_and_removed_components_are_never_charged_from_the_structure() {
    let pool = pool().await;
    let a = payroll_accounts(&pool).await;
    let svc = PayrollWriteService::new(pool.clone());
    let structure = svc
        .create_structure(NewStructure {
            name: "Staff".into(),
            components: vec![NewComponent {
                name: "Gaji Pokok".into(),
                component_type: "earning".into(),
                amount: dec("10000000"),
                gl_account_id: a.salary_expense,
            }],
        })
        .await
        .expect("structure");
    // A statutory marker with a typed amount, as the console lets an operator enter it.
    add_component(
        &pool,
        structure,
        "BPJS Kesehatan",
        "deduction",
        "160000",
        a.bpjs_payable,
        true,
        false,
    )
    .await;
    // Removed items: still rows, never paid.
    add_component(
        &pool,
        structure,
        "Koperasi",
        "deduction",
        "50000",
        a.salary_payable,
        false,
        true,
    )
    .await;
    add_component(
        &pool,
        structure,
        "Tunjangan Lama",
        "earning",
        "1000000",
        a.salary_expense,
        false,
        true,
    )
    .await;

    let run = svc
        .create_payroll_entry(NewPayrollEntry {
            period_start: None,
            period_end: None,
            period_year: 2026,
            period_month: 7,
            salary_expense_account_id: a.salary_expense,
            salary_payable_account_id: a.salary_payable,
        })
        .await
        .unwrap();
    let slip = svc
        .add_salary_slip(
            run,
            NewSalarySlip {
                timesheet_approval_id: None,
                employee_id: Uuid::new_v4(),
                structure_id: structure,
                working_days: dec("22"),
                unpaid_days: dec("0"),
                overtime_hours: dec("0"),
                tax_method: None,
                statutory: vec![
                    StatutoryLine {
                        source_kind: None,
                        source_ref: None,
                        name: "BPJS Kesehatan".into(),
                        component_type: "deduction".into(),
                        amount: dec("100000"),
                        gl_account_id: a.bpjs_payable,
                    },
                    StatutoryLine {
                        source_kind: None,
                        source_ref: None,
                        name: "PPh 21".into(),
                        component_type: "deduction".into(),
                        amount: dec("500000"),
                        gl_account_id: a.pph21_payable,
                    },
                ],
            },
        )
        .await
        .unwrap();

    let lines: Vec<(String, Decimal, bool)> = sqlx::query_as(
        "SELECT name, amount, is_statutory FROM payroll.salary_slip_lines
          WHERE salary_slip_id=$1 ORDER BY name, is_statutory",
    )
    .bind(slip)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        lines,
        vec![
            ("BPJS Kesehatan".into(), dec("100000.00"), true),
            ("Gaji Pokok".into(), dec("10000000.00"), false),
            ("PPh 21".into(), dec("500000.00"), true),
        ],
        "one BPJS line, the computed one; no removed item"
    );
    let (gross, net): (Decimal, Decimal) =
        sqlx::query_as("SELECT gross_pay, net_pay FROM payroll.salary_slips WHERE id=$1")
            .bind(slip)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(gross, dec("10000000.00"));
    assert_eq!(
        net,
        dec("9400000.00"),
        "net = 10,000,000 − 100,000 − 500,000"
    );
}
