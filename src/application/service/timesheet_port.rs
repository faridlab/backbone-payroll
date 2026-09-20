//! Approved-timesheet input port — where a slip's APPROVED hours come from.
//!
//! The attendance port prices what the CLOCK saw (time-debt); this port prices
//! what the EMPLOYEE claimed and an APPROVER approved — the timesheet period's
//! overtime rows, day by day, plus the approval row that vouches for them. A
//! slip line minted from this port carries its provenance (`source_kind =
//! 'timesheet_approved'`, `source_ref` = the approval row), and the run row
//! stamps the same approval: the join both sides of the handoff were missing.
//!
//! The default [`PoolApprovedTimesheet`] mirrors the timesheet tables as plain
//! SQL (same standalone posture as `PoolOvertimeInputs` — no Cargo edge); a
//! composing host wires an adapter over its own exports when it wants the SQL
//! to live in one place. Both modules are tenant-agnostic (ADR-0029): the read
//! is org-scoped only by the composing service's fence.
//!
//! Day grain matters exactly as it does for the clock port: the statutory
//! 1.5x-first-hour band resets per day, so a window-summed figure would
//! over-pay every one-hour-per-day pattern.

use async_trait::async_trait;
use chrono::{Datelike, NaiveDate};
use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

/// One employee's approved timesheet overtime for a period, with the approval
/// that vouches for it.
#[derive(Debug, Clone)]
pub struct ApprovedOvertime {
    /// The `timesheet_approvals` row (status approved) that vouches for the
    /// hours — the provenance ref stamped on the line and the run.
    pub approval_id: Uuid,
    /// `(date, hours)` pairs — one per date with overtime rows, 2dp.
    pub stretches: Vec<(NaiveDate, Decimal)>,
}

#[async_trait]
pub trait ApprovedTimesheetInputs: Send + Sync {
    /// `None` when the period has no APPROVED timesheet (an unsubmitted or
    /// pending period is normal and must not error — it simply contributes
    /// nothing); `Err` only for infrastructure failures.
    async fn approved_overtime(
        &self,
        employee_id: Uuid,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Option<ApprovedOvertime>, sqlx::Error>;
}

/// Default pool-backed port: live overtime rows under an approved period row.
pub struct PoolApprovedTimesheet {
    pool: PgPool,
}

impl PoolApprovedTimesheet {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ApprovedTimesheetInputs for PoolApprovedTimesheet {
    async fn approved_overtime(
        &self,
        employee_id: Uuid,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Option<ApprovedOvertime>, sqlx::Error> {
        let approval: Option<Uuid> = sqlx::query_scalar(
            r#"SELECT a.id
                 FROM timesheet.timesheet_approvals a
                WHERE a.employee_id = $1
                  AND ((a.year = $2 AND a.month >= $3) OR (a.year = $4 AND a.month <= $5))
                  AND a.status = 'approved'
                ORDER BY a.year DESC, a.month DESC
                LIMIT 1"#,
        )
        .bind(employee_id)
        .bind(from.year())
        .bind(from.month() as i32)
        .bind(to.year())
        .bind(to.month() as i32)
        .fetch_optional(&self.pool)
        .await?;
        let Some(approval_id) = approval else {
            return Ok(None);
        };
        let stretches: Vec<(NaiveDate, Decimal)> = sqlx::query_as(
            r#"SELECT t.date, SUM(t.unit_amount)
                 FROM timesheet.timesheets t
                WHERE t.employee_id = $1
                  AND t.date BETWEEN $2 AND $3
                  AND t.entry_type = 'overtime'
                  AND (t.metadata->>'deleted_at') IS NULL
                GROUP BY t.date
                ORDER BY t.date"#,
        )
        .bind(employee_id)
        .bind(from)
        .bind(to)
        .fetch_all(&self.pool)
        .await?;
        Ok(Some(ApprovedOvertime { approval_id, stretches }))
    }
}
