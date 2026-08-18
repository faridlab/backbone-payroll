//! Overtime-hours input port — where a slip's overtime comes from.
//!
//! Attendance owns the `time_debt` JSON semantics (it is the only writer of the
//! `overtime_minutes` key); this port is payroll's single consumption point. The default
//! [`PoolOvertimeInputs`] mirrors attendance's exported read as plain SQL so payroll works
//! standalone (tests, module-only deployments) with no Cargo edge to the attendance module; a
//! composing host that DOES wire backbone-attendance should inject an adapter over its exported
//! `overtime_hours` instead, so the SQL lives in exactly one place. Both must agree: same table,
//! same key, same soft-delete predicate, same zero-floor.

use async_trait::async_trait;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

/// Reads one employee's overtime hours in `[from, to]` (inclusive) for a payroll period.
#[async_trait]
pub trait OvertimeInputs: Send + Sync {
    /// Overtime hours worked in the window. Zero when none — an ABSENCE of overtime is normal and
    /// must not error; only infrastructure failures do.
    async fn overtime_hours(
        &self,
        company_id: Uuid,
        employee_id: Uuid,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Decimal, sqlx::Error>;
}

/// Default pool-backed [`OvertimeInputs`] — the same read attendance exports
/// (`SUM((time_debt->>'overtime_minutes')::numeric)/60` over live daily rollups), expressed here as
/// plain SQL for standalone use. Company-scoped by predicate (attendance's rollup table is fenced);
/// the caller is expected to run inside a request/company scope or bind explicitly.
pub struct PoolOvertimeInputs {
    pool: PgPool,
}

impl PoolOvertimeInputs {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl OvertimeInputs for PoolOvertimeInputs {
    async fn overtime_hours(
        &self,
        company_id: Uuid,
        employee_id: Uuid,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Decimal, sqlx::Error> {
        let (hours,): (Decimal,) = sqlx::query_as(
            r#"SELECT GREATEST(
                   COALESCE(SUM((time_debt->>'overtime_minutes')::numeric), 0) / 60, 0)
               FROM attendance.attendances
               WHERE company_id = $1
                 AND employee_id = $2
                 AND date BETWEEN $3 AND $4
                 AND (metadata->>'deleted_at') IS NULL"#,
        )
        .bind(company_id)
        .bind(employee_id)
        .bind(from)
        .bind(to)
        .fetch_one(&self.pool)
        .await?;
        Ok(hours)
    }
}
