//! A run beside the run before it: what a person checks before approving a run. The previous run
//! is the latest processed or posted one of an earlier period (a draft is not a run anyone paid);
//! the comparison names who joined and left, how far each pay component moved, and each person's
//! pay against last run, largest change first.

mod common;
use common::*;

use backbone_payroll::application::service::compare_runs;
use uuid::Uuid;

async fn run(pool: &sqlx::PgPool, year: i32, month: i32, status: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO payroll.payroll_entries (id, period_year, period_month, status, metadata) \
         VALUES ($1, $2, $3, $4::payroll_status, jsonb_build_object('created_at', now()))",
    )
    .bind(id)
    .bind(year)
    .bind(month)
    .bind(status)
    .execute(pool)
    .await
    .expect("run");
    id
}

/// A slip with its earning lines; gross is their sum, net a tenth less.
async fn slip(pool: &sqlx::PgPool, run: Uuid, employee: Uuid, overtime: &str, lines: &[(&str, &str)]) {
    let id = Uuid::new_v4();
    let gross: rust_decimal::Decimal = lines.iter().map(|(_, a)| dec(a)).sum();
    sqlx::query(
        "INSERT INTO payroll.salary_slips \
           (id, payroll_entry_id, employee_id, gross_pay, total_deductions, net_pay, overtime_hours, metadata) \
         VALUES ($1, $2, $3, $4, $4 / 10, $4 - $4 / 10, $5, jsonb_build_object('created_at', now()))",
    )
    .bind(id)
    .bind(run)
    .bind(employee)
    .bind(gross)
    .bind(dec(overtime))
    .execute(pool)
    .await
    .expect("slip");
    for (name, amount) in lines {
        sqlx::query(
            "INSERT INTO payroll.salary_slip_lines \
               (id, salary_slip_id, name, component_type, amount, gl_account_id, metadata) \
             VALUES ($1, $2, $3, 'earning'::component_type, $4, $5, jsonb_build_object('created_at', now()))",
        )
        .bind(Uuid::new_v4())
        .bind(id)
        .bind(*name)
        .bind(dec(amount))
        .bind(Uuid::new_v4())
        .execute(pool)
        .await
        .expect("line");
    }
}

#[tokio::test]
async fn a_run_is_compared_with_the_last_run_paid_before_it() {
    let pool = pool().await;
    // A year of its own, so no other test's runs can be the previous one.
    let year = 2500 + (Uuid::new_v4().as_u128() % 5000) as i32;
    let july = run(&pool, year, 7, "posted").await;
    let _draft = run(&pool, year, 7, "draft").await;
    let august = run(&pool, year, 8, "processed").await;
    let (stays_less, stays_more, leaves, joins) =
        (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());

    slip(&pool, july, stays_less, "20", &[("Gaji Pokok", "8000000"), ("Lembur", "2000000")]).await;
    slip(&pool, july, stays_more, "0", &[("Gaji Pokok", "5000000")]).await;
    slip(&pool, july, leaves, "0", &[("Gaji Pokok", "4000000")]).await;
    slip(&pool, august, stays_less, "0", &[("Gaji Pokok", "8000000")]).await;
    slip(&pool, august, stays_more, "6", &[("Gaji Pokok", "5000000"), ("Lembur", "600000")]).await;
    slip(&pool, august, joins, "0", &[("Gaji Pokok", "3000000")]).await;

    let c = compare_runs(&pool, august, 50, 0).await.expect("compare");
    let prev = c.previous.expect("a previous run");
    assert_eq!(prev.id, july, "the posted July run, not the July draft");
    assert_eq!((c.current.headcount, c.current.gross), (3, dec("16600000")));
    assert_eq!((prev.headcount, prev.gross), (3, dec("19000000")));
    assert_eq!((c.current.overtime_hours, prev.overtime_hours), (dec("6"), dec("20")));
    assert_eq!((c.joined, c.left), (1, 1));
    // Both people who stayed moved by more than a tenth: −20% and +12%.
    assert_eq!(c.changed_over_tenth, 2);

    let parts: Vec<_> = c.components.iter().map(|p| (p.name.as_str(), p.change)).collect();
    assert_eq!(parts, vec![("Lembur", dec("-1400000")), ("Gaji Pokok", dec("-1000000"))]);

    let order: Vec<_> = c.people.iter().map(|p| (p.employee_id, p.change)).collect();
    assert_eq!(
        order,
        vec![
            (leaves, dec("-4000000")),
            (joins, dec("3000000")),
            (stays_less, dec("-2000000")),
            (stays_more, dec("600000")),
        ]
    );
    assert_eq!(c.people[0].gross, None, "who left has no pay this run");
    assert_eq!(c.people[1].previous_gross, None, "who joined had none last run");

    // The first run has nothing to compare with, and says so rather than showing zeros.
    let first = compare_runs(&pool, july, 50, 0).await.expect("compare first");
    assert!(first.previous.is_none());
    assert!(first.components.is_empty() && first.people.is_empty());
}

#[tokio::test]
async fn an_unknown_run_is_not_found() {
    let pool = pool().await;
    let err = compare_runs(&pool, Uuid::new_v4(), 50, 0).await.expect_err("no such run");
    assert_eq!(err.code(), "not_found");
}
