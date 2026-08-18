//! Overtime-hours input port — where a slip's overtime comes from.
//!
//! Attendance owns the `time_debt` JSON semantics (it is the only writer of the
//! `overtime_minutes` key); this port is payroll's single consumption point. The default
//! [`PoolOvertimeInputs`] mirrors attendance's exported read as plain SQL so payroll works
//! standalone (tests, module-only deployments) with no Cargo edge to the attendance module; a
//! composing host that DOES wire backbone-attendance should inject an adapter over its exported
//! `overtime_hours` instead, so the SQL lives in exactly one place. Both must agree: same table,
//! same key, same soft-delete predicate, same zero-floor.
//!
//! The contract is PER DAY: the workday band schedule (including the 1.5× first hour) resets
//! with each day's overtime stretch, so the port returns one hours figure per date and the slip
//! builder prices each day separately. A window-summed single figure would price hour 1 of the
//! whole period at 1.5× and over-pay every one-hour-per-day pattern.

use async_trait::async_trait;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

/// Reads one employee's daily overtime stretches in `[from, to]` (inclusive) for a payroll period.
#[async_trait]
pub trait OvertimeInputs: Send + Sync {
    /// `(date, hours)` pairs — one per date with overtime, hours = minutes/60 at 2dp. Empty when
    /// none — an ABSENCE of overtime is normal and must not error; only infrastructure failures do.
    async fn overtime_stretches(
        &self,
        company_id: Uuid,
        employee_id: Uuid,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<(NaiveDate, Decimal)>, sqlx::Error>;
}

/// Default pool-backed [`OvertimeInputs`] — the same read attendance exports
/// (`SUM((time_debt->>'overtime_minutes')::numeric)/60` over live daily rollups, grouped by date),
/// expressed here as plain SQL for standalone use. Company-scoped by predicate (attendance's
/// rollup table is fenced); the caller is expected to run inside a request/company scope or bind
/// explicitly.
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
    async fn overtime_stretches(
        &self,
        company_id: Uuid,
        employee_id: Uuid,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<(NaiveDate, Decimal)>, sqlx::Error> {
        let rows: Vec<(NaiveDate, Decimal)> = sqlx::query_as(
            r#"SELECT date,
                      GREATEST(SUM((time_debt->>'overtime_minutes')::numeric) / 60, 0)
               FROM attendance.attendances
               WHERE company_id = $1
                 AND employee_id = $2
                 AND date BETWEEN $3 AND $4
                 AND (metadata->>'deleted_at') IS NULL
               GROUP BY date"#,
        )
        .bind(company_id)
        .bind(employee_id)
        .bind(from)
        .bind(to)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .filter(|(_, hours)| *hours > Decimal::ZERO)
            .collect())
    }
}
