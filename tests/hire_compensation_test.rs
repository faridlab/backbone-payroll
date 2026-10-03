//! A hire's first salary: the `recruitment.hired` consumer records the offered salary as the new
//! employee's first compensation change, effective on the first day, so payroll and severance
//! read a current salary from the moment of the hire. Completing the onboarding afterwards adds
//! no second initial row, and a joiner hired outside recruitment still gets theirs at completion,
//! read from the tenant's database the relay delivers on.
//!
//! Each test runs in PRIVATE scratch databases (`payroll_hire_comp_test_{suffix}…`) created from
//! the server named by `DATABASE_URL` (the role must be allowed to create databases), holding
//! just the tables the consumers touch. When that server cannot be reached the test skips.

use std::sync::Arc;

use backbone_messaging::{IntegrationEventEnvelope, IntegrationEventHandler};
use backbone_outbox::outbox;
use backbone_payroll::application::service::{
    HireCompensationHandler, HiredEmployeeResolver, OnboardingEnrolledHandler,
};
use chrono::{NaiveDate, Utc};
use rust_decimal::Decimal;
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

async fn scratch(name: &str) -> Option<PgPool> {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost:5433/postgres".into());
    let (prefix, _) = url.trim_end_matches('/').rsplit_once('/')?;
    let admin = match PgPool::connect(&format!("{prefix}/postgres")).await {
        Ok(p) => p,
        Err(e) => {
            eprintln!("skip hire_compensation_test: cannot reach `{prefix}/postgres` ({e})");
            return None;
        }
    };
    let db = format!("payroll_hire_comp_test_{name}");
    let _ = sqlx::query(&format!(r#"DROP DATABASE IF EXISTS "{db}" WITH (FORCE)"#))
        .execute(&admin)
        .await;
    sqlx::query(&format!(r#"CREATE DATABASE "{db}""#))
        .execute(&admin)
        .await
        .expect("create scratch database");
    admin.close().await;
    let pool = PgPool::connect(&format!("{prefix}/{db}"))
        .await
        .expect("connect scratch database");
    for stmt in [
        "CREATE TYPE compensation_change_type AS ENUM ('hire','promotion','transfer','adjustment','offboarding')",
        "CREATE SCHEMA employee",
        "CREATE SCHEMA payroll",
        r#"CREATE TABLE employee.employees (
               id UUID PRIMARY KEY,
               base_salary NUMERIC(18,2),
               metadata JSONB NOT NULL DEFAULT '{}'::jsonb
           )"#,
        r#"CREATE TABLE payroll.compensation_changes (
               id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
               employee_id UUID NOT NULL,
               change_type compensation_change_type NOT NULL,
               new_amount NUMERIC(18,2),
               effective_date DATE,
               reference_id UUID,
               note TEXT,
               metadata JSONB NOT NULL DEFAULT '{}'::jsonb,
               org_unit_id UUID NOT NULL
           )"#,
    ] {
        sqlx::query(stmt).execute(&pool).await.expect("setup ddl");
    }
    outbox::migrate(&pool, "payroll").await.expect("payroll inbox");
    Some(pool)
}

/// The composer's resolver stand-in: the employee id the test minted for the offer.
struct Fixed(Uuid);

impl HiredEmployeeResolver for Fixed {
    fn hired_employee_id(&self, _envelope: &IntegrationEventEnvelope) -> Option<Uuid> {
        Some(self.0)
    }
}

fn envelope(event_type: &str, payload: serde_json::Value) -> IntegrationEventEnvelope {
    IntegrationEventEnvelope {
        id: Uuid::new_v4().to_string(),
        event_type: event_type.into(),
        source_context: "test".into(),
        aggregate_id: "agg".into(),
        occurred_at: Utc::now(),
        published_at: Utc::now(),
        version: 1,
        correlation_id: None,
        causation_id: None,
        payload,
    }
}

fn hired(company: Uuid, offer: Uuid, salary: Option<&str>, start: Option<&str>) -> IntegrationEventEnvelope {
    envelope(
        "recruitment.hired",
        json!({
            "offer_id": offer,
            "company_id": company,
            "first_name": "Rina",
            "proposed_salary": salary,
            "join_date": "2026-10-02",
            "start_date": start,
        }),
    )
}

async fn add_employee(pool: &PgPool, id: Uuid, base_salary: Option<&str>) {
    sqlx::query("INSERT INTO employee.employees (id, base_salary) VALUES ($1, $2::numeric)")
        .bind(id)
        .bind(base_salary)
        .execute(pool)
        .await
        .expect("seed employee");
}

async fn changes(pool: &PgPool, employee: Uuid) -> Vec<(String, Decimal, NaiveDate, Option<Uuid>, Uuid)> {
    sqlx::query_as(
        r#"SELECT change_type::text, new_amount, effective_date, reference_id, org_unit_id
             FROM payroll.compensation_changes WHERE employee_id = $1 ORDER BY effective_date"#,
    )
    .bind(employee)
    .fetch_all(pool)
    .await
    .expect("read compensation changes")
}

fn day(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
}

/// The offered salary becomes the first compensation change, effective on the proposed first
/// day and placed in the hiring company; a redelivery adds nothing, and the later onboarding
/// completion adds no second initial row.
#[tokio::test]
async fn a_hire_records_the_offered_salary_from_the_first_day() {
    let Some(pool) = scratch("records").await else { return };
    let (company, offer, employee) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    add_employee(&pool, employee, Some("9000000")).await;

    let handler = HireCompensationHandler::new(pool.clone(), Arc::new(Fixed(employee)));
    let event = hired(company, offer, Some("9000000"), Some("2026-11-02"));
    handler.handle(event.clone()).await.expect("hire compensation applies");
    handler.handle(event).await.expect("a redelivery is a no-op");

    let rows = changes(&pool, employee).await;
    assert_eq!(
        rows,
        vec![("hire".to_string(), Decimal::from(9_000_000), day("2026-11-02"), Some(offer), company)]
    );

    OnboardingEnrolledHandler::new(pool.clone())
        .handle(envelope(
            "onboarding.completed",
            json!({ "employee_id": employee, "onboarding_id": Uuid::new_v4(), "company_id": company }),
        ))
        .await
        .expect("completion applies");
    assert_eq!(changes(&pool, employee).await.len(), 1, "no second initial row at completion");
}

/// Without a proposed start date the salary is effective from the hire day.
#[tokio::test]
async fn a_hire_without_a_start_date_is_effective_from_the_hire_day() {
    let Some(pool) = scratch("hireday").await else { return };
    let (company, offer, employee) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    add_employee(&pool, employee, None).await;
    HireCompensationHandler::new(pool.clone(), Arc::new(Fixed(employee)))
        .handle(hired(company, offer, Some("8500000.50"), None))
        .await
        .expect("applies");
    let rows = changes(&pool, employee).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].2, day("2026-10-02"));
    assert_eq!(rows[0].1, "8500000.50".parse::<Decimal>().unwrap());
}

/// Until the employee consumer's row exists the handler refuses and keeps no claim, so the retry
/// that follows records the salary; an offer without a salary claims and records nothing.
#[tokio::test]
async fn a_hire_before_its_employee_exists_is_retried_and_one_without_salary_is_skipped() {
    let Some(pool) = scratch("retry").await else { return };
    let (company, employee) = (Uuid::new_v4(), Uuid::new_v4());
    let handler = HireCompensationHandler::new(pool.clone(), Arc::new(Fixed(employee)));
    let event = hired(company, Uuid::new_v4(), Some("9000000"), Some("2026-11-02"));

    assert!(handler.handle(event.clone()).await.is_err(), "no employee yet → refused");
    add_employee(&pool, employee, None).await;
    handler.handle(event).await.expect("the retry applies");
    assert_eq!(changes(&pool, employee).await.len(), 1);

    let other = Uuid::new_v4();
    add_employee(&pool, other, None).await;
    HireCompensationHandler::new(pool.clone(), Arc::new(Fixed(other)))
        .handle(hired(company, Uuid::new_v4(), None, Some("2026-11-02")))
        .await
        .expect("an offer without salary claims and skips");
    assert!(changes(&pool, other).await.is_empty());
}

/// A joiner hired outside recruitment gets their initial row at onboarding completion — with the
/// starting salary read from the database the relay delivers on, not the composed pool's.
#[tokio::test]
async fn completion_reads_the_starting_salary_from_the_delivering_tenant() {
    let Some(tenant) = scratch("tenant").await else { return };
    let Some(main) = scratch("main").await else { return };
    let (company, employee) = (Uuid::new_v4(), Uuid::new_v4());
    add_employee(&tenant, employee, Some("7000000")).await;

    // Composed on the main database; the relay binds the tenant's pool for the delivery.
    let handler = OnboardingEnrolledHandler::new(main.clone());
    backbone_payroll::request_pool::with_pool_scope(
        tenant.clone(),
        handler.handle(envelope(
            "onboarding.completed",
            json!({ "employee_id": employee, "onboarding_id": Uuid::new_v4(), "company_id": company }),
        )),
    )
    .await
    .expect("completion applies");

    let rows = changes(&tenant, employee).await;
    assert_eq!(rows.len(), 1, "the joiner's initial row lands in the tenant");
    assert_eq!(rows[0].1, Decimal::from(7_000_000));
    assert_eq!(rows[0].4, company);
}
