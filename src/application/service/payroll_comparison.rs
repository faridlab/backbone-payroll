//! A payroll run beside the run before it — the read behind "is this run right?".
//!
//! Hand-authored (user-owned; see `metaphor.codegen.yaml`). A run is approved by someone checking
//! it against the last one, so this answers in one read: both runs' totals, who joined and who
//! left, how much each pay component moved, and each person's pay this run against last run,
//! largest change first. The previous run is the latest processed or posted run of an earlier
//! period; a run with none has nothing to compare with, and says so (`previous: None`).
//!
//! Read-only. Every query runs through `fetch_all_rows_scoped`, so the composing service's
//! tenancy fence applies (ADR-0029) and the module invents no scope of its own.

use backbone_orm::org_scope::fetch_all_rows_scoped;
use rust_decimal::Decimal;
use serde::Serialize;
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::application::service::PayrollError;

/// One run's identity and its slips' totals.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RunTotals {
    pub id: Uuid,
    pub period_year: i32,
    pub period_month: i32,
    pub status: String,
    pub headcount: i64,
    pub gross: Decimal,
    pub net: Decimal,
    pub deductions: Decimal,
    pub overtime_hours: Decimal,
}

/// One pay component (a slip line's name) in both runs.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ComponentChange {
    pub name: String,
    pub component_type: String,
    pub current: Decimal,
    pub previous: Decimal,
    pub change: Decimal,
}

/// One person's pay this run against last run; `None` where they were not in that run.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PersonChange {
    pub employee_id: Uuid,
    pub gross: Option<Decimal>,
    pub previous_gross: Option<Decimal>,
    pub net: Option<Decimal>,
    pub previous_net: Option<Decimal>,
    pub overtime_hours: Option<Decimal>,
    pub previous_overtime_hours: Option<Decimal>,
    pub change: Decimal,
}

/// The whole comparison.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RunComparison {
    pub current: RunTotals,
    pub previous: Option<RunTotals>,
    /// In this run and not the previous one.
    pub joined: i64,
    /// In the previous run and not this one.
    pub left: i64,
    /// People whose gross moved by more than a tenth, either way.
    pub changed_over_tenth: i64,
    /// Largest movement first.
    pub components: Vec<ComponentChange>,
    /// Largest change first, `limit` of them from `offset`.
    pub people: Vec<PersonChange>,
}

const LIVE: &str = "(metadata->>'deleted_at') IS NULL";

/// Compare run `id` with the run before it.
pub async fn compare_runs(
    pool: &PgPool,
    id: Uuid,
    limit: i64,
    offset: i64,
) -> Result<RunComparison, PayrollError> {
    let run = run_header(pool, id).await?.ok_or(PayrollError::NotFound("payroll run"))?;
    let previous_id = previous_run(pool, run.1, run.2).await?;
    let ids: Vec<Uuid> = std::iter::once(id).chain(previous_id).collect();
    let totals = totals_of(pool, &ids).await?;
    let current = totals
        .iter()
        .find(|t| t.id == id)
        .cloned()
        .unwrap_or_else(|| empty_totals(id, run.1, run.2, run.3.clone()));
    let previous = previous_id.map(|p| {
        totals.iter().find(|t| t.id == p).cloned().unwrap_or_else(|| empty_totals(p, 0, 0, String::new()))
    });
    let Some(prev) = previous_id else {
        return Ok(RunComparison {
            current,
            previous: None,
            joined: 0,
            left: 0,
            changed_over_tenth: 0,
            components: Vec::new(),
            people: Vec::new(),
        });
    };
    let (joined, left, changed_over_tenth) = movement(pool, id, prev).await?;
    Ok(RunComparison {
        current,
        previous,
        joined,
        left,
        changed_over_tenth,
        components: components(pool, id, prev).await?,
        people: people(pool, id, prev, limit.clamp(1, 200), offset.max(0)).await?,
    })
}

fn empty_totals(id: Uuid, year: i32, month: i32, status: String) -> RunTotals {
    RunTotals {
        id,
        period_year: year,
        period_month: month,
        status,
        headcount: 0,
        gross: Decimal::ZERO,
        net: Decimal::ZERO,
        deductions: Decimal::ZERO,
        overtime_hours: Decimal::ZERO,
    }
}

async fn run_header(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<(Uuid, i32, i32, String)>, sqlx::Error> {
    let sql = format!(
        "SELECT id, period_year, period_month, status::text AS status \
           FROM payroll.payroll_entries WHERE id = $1 AND {LIVE}"
    );
    let rows = fetch_all_rows_scoped(pool, sqlx::query(&sql).bind(id)).await?;
    Ok(rows.first().map(|r| (r.get("id"), r.get("period_year"), r.get("period_month"), r.get("status"))))
}

/// The latest processed or posted run of an earlier period: the one a person checks against.
async fn previous_run(pool: &PgPool, year: i32, month: i32) -> Result<Option<Uuid>, sqlx::Error> {
    let sql = format!(
        "SELECT id FROM payroll.payroll_entries \
          WHERE status::text IN ('processed', 'posted') AND {LIVE} \
            AND (period_year, period_month) < ($1, $2) \
          ORDER BY period_year DESC, period_month DESC LIMIT 1"
    );
    let rows = fetch_all_rows_scoped(pool, sqlx::query(&sql).bind(year).bind(month)).await?;
    Ok(rows.first().map(|r| r.get("id")))
}

async fn totals_of(pool: &PgPool, ids: &[Uuid]) -> Result<Vec<RunTotals>, sqlx::Error> {
    let sql = format!(
        "SELECT e.id, e.period_year, e.period_month, e.status::text AS status, \
                count(s.id) AS headcount, \
                coalesce(sum(s.gross_pay), 0) AS gross, coalesce(sum(s.net_pay), 0) AS net, \
                coalesce(sum(s.total_deductions), 0) AS deductions, \
                coalesce(sum(s.overtime_hours), 0) AS overtime_hours \
           FROM payroll.payroll_entries e \
           LEFT JOIN payroll.salary_slips s \
                  ON s.payroll_entry_id = e.id AND (s.metadata->>'deleted_at') IS NULL \
          WHERE e.id = ANY($1) \
          GROUP BY e.id, e.period_year, e.period_month, e.status"
    );
    let rows = fetch_all_rows_scoped(pool, sqlx::query(&sql).bind(ids)).await?;
    Ok(rows
        .iter()
        .map(|r| RunTotals {
            id: r.get("id"),
            period_year: r.get("period_year"),
            period_month: r.get("period_month"),
            status: r.get("status"),
            headcount: r.get("headcount"),
            gross: r.get("gross"),
            net: r.get("net"),
            deductions: r.get("deductions"),
            overtime_hours: r.get("overtime_hours"),
        })
        .collect())
}

/// Joined, left, and how many people's gross moved by more than a tenth.
async fn movement(pool: &PgPool, current: Uuid, previous: Uuid) -> Result<(i64, i64, i64), sqlx::Error> {
    let sql = format!(
        "WITH c AS (SELECT employee_id, gross_pay FROM payroll.salary_slips \
                     WHERE payroll_entry_id = $1 AND {LIVE}), \
              p AS (SELECT employee_id, gross_pay FROM payroll.salary_slips \
                     WHERE payroll_entry_id = $2 AND {LIVE}) \
         SELECT count(*) FILTER (WHERE p.employee_id IS NULL) AS joined, \
                count(*) FILTER (WHERE c.employee_id IS NULL) AS left_, \
                count(*) FILTER (WHERE c.employee_id IS NOT NULL AND p.employee_id IS NOT NULL \
                                   AND abs(c.gross_pay - p.gross_pay) > abs(p.gross_pay) / 10) AS changed \
           FROM c FULL OUTER JOIN p ON p.employee_id = c.employee_id"
    );
    let rows = fetch_all_rows_scoped(pool, sqlx::query(&sql).bind(current).bind(previous)).await?;
    let r = rows.first();
    Ok((
        r.map(|r| r.get("joined")).unwrap_or(0),
        r.map(|r| r.get("left_")).unwrap_or(0),
        r.map(|r| r.get("changed")).unwrap_or(0),
    ))
}

async fn components(
    pool: &PgPool,
    current: Uuid,
    previous: Uuid,
) -> Result<Vec<ComponentChange>, sqlx::Error> {
    let sql = "SELECT l.name, l.component_type::text AS component_type, \
                      coalesce(sum(l.amount) FILTER (WHERE s.payroll_entry_id = $1), 0) AS current, \
                      coalesce(sum(l.amount) FILTER (WHERE s.payroll_entry_id = $2), 0) AS previous \
                 FROM payroll.salary_slip_lines l \
                 JOIN payroll.salary_slips s ON s.id = l.salary_slip_id \
                WHERE s.payroll_entry_id IN ($1, $2) \
                  AND (s.metadata->>'deleted_at') IS NULL AND (l.metadata->>'deleted_at') IS NULL \
                GROUP BY l.name, l.component_type";
    let rows = fetch_all_rows_scoped(pool, sqlx::query(sql).bind(current).bind(previous)).await?;
    let mut out: Vec<ComponentChange> = rows
        .iter()
        .map(|r| {
            let current: Decimal = r.get("current");
            let previous: Decimal = r.get("previous");
            ComponentChange {
                name: r.get("name"),
                component_type: r.get("component_type"),
                current,
                previous,
                change: current - previous,
            }
        })
        .collect();
    out.sort_by(|a, b| b.change.abs().cmp(&a.change.abs()).then_with(|| a.name.cmp(&b.name)));
    Ok(out)
}

async fn people(
    pool: &PgPool,
    current: Uuid,
    previous: Uuid,
    limit: i64,
    offset: i64,
) -> Result<Vec<PersonChange>, sqlx::Error> {
    let sql = format!(
        "WITH c AS (SELECT employee_id, gross_pay, net_pay, overtime_hours FROM payroll.salary_slips \
                     WHERE payroll_entry_id = $1 AND {LIVE}), \
              p AS (SELECT employee_id, gross_pay, net_pay, overtime_hours FROM payroll.salary_slips \
                     WHERE payroll_entry_id = $2 AND {LIVE}) \
         SELECT coalesce(c.employee_id, p.employee_id) AS employee_id, \
                c.gross_pay AS gross, p.gross_pay AS previous_gross, \
                c.net_pay AS net, p.net_pay AS previous_net, \
                c.overtime_hours, p.overtime_hours AS previous_overtime_hours, \
                coalesce(c.gross_pay, 0) - coalesce(p.gross_pay, 0) AS change \
           FROM c FULL OUTER JOIN p ON p.employee_id = c.employee_id \
          ORDER BY abs(coalesce(c.gross_pay, 0) - coalesce(p.gross_pay, 0)) DESC, 1 \
          LIMIT $3 OFFSET $4"
    );
    let rows = fetch_all_rows_scoped(
        pool,
        sqlx::query(&sql).bind(current).bind(previous).bind(limit).bind(offset),
    )
    .await?;
    Ok(rows
        .iter()
        .map(|r| PersonChange {
            employee_id: r.get("employee_id"),
            gross: r.get("gross"),
            previous_gross: r.get("previous_gross"),
            net: r.get("net"),
            previous_net: r.get("previous_net"),
            overtime_hours: r.get("overtime_hours"),
            previous_overtime_hours: r.get("previous_overtime_hours"),
            change: r.get("change"),
        })
        .collect())
}
