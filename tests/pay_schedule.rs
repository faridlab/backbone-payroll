//! Changing a salary structure from a date: a run for a period before the date keeps what the
//! structure was, a run from the date reads the change, and the structure as it reads today moves
//! when the day comes. A change cannot reach a run already processed, and only a change still to
//! come can be withdrawn.

mod common;
use common::*;

use backbone_payroll::application::service::pay_schedule::{
    apply_due_changes, plan_change, withdraw_change, PlannedChange, PlannedItem,
};
use backbone_payroll::application::service::{
    NewComponent, NewPayrollEntry, NewSalarySlip, NewStructure, PayrollWriteService,
};
use chrono::NaiveDate;
use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

/// A year no other test writes runs in, so the run per period stays this test's own.
fn spare_year() -> i32 {
    2200 + (Uuid::new_v4().as_u128() % 700) as i32
}

async fn gross_in(svc: &PayrollWriteService, a: &PayrollAccounts, structure: Uuid, year: i32, month: i32) -> (Uuid, Decimal) {
    let run = svc
        .create_payroll_entry(NewPayrollEntry {
            period_year: year,
            period_month: month,
            period_start: None,
            period_end: None,
            salary_expense_account_id: a.salary_expense,
            salary_payable_account_id: a.salary_payable,
        })
        .await
        .unwrap();
    let slip = svc
        .add_salary_slip(run, NewSalarySlip {
            employee_id: Uuid::new_v4(),
            structure_id: structure,
            working_days: dec("22"),
            unpaid_days: dec("0"),
            statutory: vec![],
            overtime_hours: dec("0"),
            timesheet_approval_id: None,
            tax_method: None,
        })
        .await
        .unwrap();
    let pool = svc.read_pool();
    let gross: Decimal = sqlx::query_scalar("SELECT gross_pay FROM payroll.salary_slips WHERE id = $1")
        .bind(slip)
        .fetch_one(&pool)
        .await
        .unwrap();
    (run, gross)
}

async fn component(pool: &PgPool, structure: Uuid, name: &str) -> Option<(Uuid, Decimal, bool)> {
    sqlx::query_as::<_, (Uuid, Decimal, bool)>(
        "SELECT id, amount, (metadata->>'deleted_at') IS NOT NULL FROM payroll.salary_components \
          WHERE structure_id = $1 AND name = $2",
    )
    .bind(structure)
    .bind(name)
    .fetch_optional(pool)
    .await
    .unwrap()
}

fn item(id: Option<Uuid>, name: &str, amount: &str, account: Uuid) -> PlannedItem {
    PlannedItem {
        component_id: id,
        name: name.into(),
        component_type: "earning".into(),
        amount: dec(amount),
        gl_account_id: account,
        is_statutory: false,
        removed: false,
    }
}

#[tokio::test]
async fn a_change_from_a_date_reaches_runs_from_that_date_only() {
    let pool = pool().await;
    let a = payroll_accounts(&pool).await;
    let svc = PayrollWriteService::new(pool.clone());
    let y = spare_year();
    let structure = svc
        .create_structure(NewStructure {
            name: "Staff Bulanan".into(),
            components: vec![
                NewComponent { name: "Gaji Pokok".into(), component_type: "earning".into(), amount: dec("10000000"), gl_account_id: a.salary_expense },
                NewComponent { name: "Transport".into(), component_type: "earning".into(), amount: dec("1000000"), gl_account_id: a.salary_expense },
            ],
        })
        .await
        .unwrap();
    let (transport, _, _) = component(&pool, structure, "Transport").await.unwrap();

    // Planned before its date: transport rises and a meal allowance starts, from March.
    let today = day(y, 1, 15);
    let out = plan_change(&pool, PlannedChange {
        structure_id: structure,
        effective_from: day(y, 3, 1),
        reason: Some("Annual review".into()),
        items: vec![
            item(Some(transport), "Transport", "1150000", a.salary_expense),
            item(None, "Makan", "500000", a.salary_expense),
        ],
    }, today)
    .await
    .unwrap();
    assert!(!out.in_force);
    assert!(out.draft_runs_to_redo.is_empty());

    // Today the structure still reads what it was; the new item is not on it yet.
    assert_eq!(component(&pool, structure, "Transport").await.unwrap().1, dec("1000000"));
    assert!(component(&pool, structure, "Makan").await.unwrap().2, "a planned item waits for its day");

    // A run before the date keeps the old structure, one from the date reads the change, and a
    // run for a month before any change still reads what the items were from the start.
    assert_eq!(gross_in(&svc, &a, structure, y - 1, 12).await.1, dec("11000000"));
    assert_eq!(gross_in(&svc, &a, structure, y, 2).await.1, dec("11000000"));
    let (march, march_gross) = gross_in(&svc, &a, structure, y, 3).await;
    assert_eq!(march_gross, dec("11650000"));

    // The day comes: the structure as it reads today moves, once.
    assert!(apply_due_changes(&pool, day(y, 3, 1)).await.unwrap() >= 2);
    assert_eq!(component(&pool, structure, "Transport").await.unwrap().1, dec("1150000"));
    assert!(!component(&pool, structure, "Makan").await.unwrap().2, "the item is in force from its day");
    let stamps = |pool: PgPool| async move {
        sqlx::query_scalar::<_, Option<String>>(
            "SELECT metadata->>'updated_at' FROM payroll.salary_components WHERE structure_id = $1 ORDER BY id",
        )
        .bind(structure)
        .fetch_all(&pool)
        .await
        .unwrap()
    };
    let again = stamps(pool.clone()).await;
    apply_due_changes(&pool, day(y, 3, 2)).await.unwrap();
    let after = stamps(pool.clone()).await;
    assert_eq!(again, after, "running the pass again changes nothing");

    // An item no longer paid from June: June reads without it, May still with it.
    plan_change(&pool, PlannedChange {
        structure_id: structure,
        effective_from: day(y, 6, 1),
        reason: None,
        items: vec![PlannedItem { removed: true, ..item(Some(transport), "Transport", "1150000", a.salary_expense) }],
    }, day(y, 3, 2))
    .await
    .unwrap();
    assert_eq!(gross_in(&svc, &a, structure, y, 5).await.1, dec("11650000"));
    assert_eq!(gross_in(&svc, &a, structure, y, 6).await.1, dec("10500000"));

    // A draft run the change reaches is named, so its slips are generated again.
    let out = plan_change(&pool, PlannedChange {
        structure_id: structure,
        effective_from: day(y, 6, 1),
        reason: None,
        items: vec![PlannedItem { removed: true, ..item(Some(transport), "Transport", "1150000", a.salary_expense) }],
    }, day(y, 3, 2))
    .await
    .unwrap();
    assert_eq!(out.draft_runs_to_redo.len(), 1);
    assert_eq!(out.draft_runs_to_redo[0].period_month, 6);

    // Once March is processed, a change that would reach it is refused.
    svc.process_payroll_entry(march).await.unwrap();
    let refused = plan_change(&pool, PlannedChange {
        structure_id: structure,
        effective_from: day(y, 2, 1),
        reason: None,
        items: vec![item(Some(transport), "Transport", "1200000", a.salary_expense)],
    }, day(y, 3, 2))
    .await
    .unwrap_err();
    assert!(refused.to_string().contains("already processed"), "{refused}");
}

#[tokio::test]
async fn only_a_change_still_to_come_can_be_withdrawn() {
    let pool = pool().await;
    let a = payroll_accounts(&pool).await;
    let svc = PayrollWriteService::new(pool.clone());
    let y = spare_year();
    let structure = svc
        .create_structure(NewStructure {
            name: "Kontrak".into(),
            components: vec![NewComponent { name: "Gaji Pokok".into(), component_type: "earning".into(), amount: dec("8000000"), gl_account_id: a.salary_expense }],
        })
        .await
        .unwrap();
    let today = day(y, 1, 15);
    plan_change(&pool, PlannedChange {
        structure_id: structure,
        effective_from: day(y, 9, 1),
        reason: None,
        items: vec![item(None, "Bonus", "250000", a.salary_expense)],
    }, today)
    .await
    .unwrap();
    assert!(component(&pool, structure, "Bonus").await.is_some());

    // Dated today or earlier, a change is in force and stays.
    let in_force = withdraw_change(&pool, structure, day(y, 1, 10), today).await.unwrap_err();
    assert!(in_force.to_string().contains("already in force"), "{in_force}");

    // Withdrawn, the item it would have added goes with it; there is nothing left to withdraw.
    assert_eq!(withdraw_change(&pool, structure, day(y, 9, 1), today).await.unwrap(), 1);
    assert!(component(&pool, structure, "Bonus").await.is_none());
    assert!(withdraw_change(&pool, structure, day(y, 9, 1), today).await.is_err());

    // The structure reads as before, for every month.
    assert_eq!(gross_in(&svc, &a, structure, y, 10).await.1, dec("8000000"));
}
