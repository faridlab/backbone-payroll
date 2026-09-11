//! Employee statutory-input port — where a slip's employee-side tax facts come from.
//!
//! Payroll needs five facts to compute statutory pay (PTKP tier, NPWP presence, Kesehatan family
//! size, join date for THR tenure, TER category). They live in the employee module's tables — but
//! shipped modules keep zero normal Cargo edges on each other, so this port mirrors the employee
//! module's exported `StatutoryInputs` bundle as a plain struct (the serialized-port pattern, same
//! as the GL envelope): a composing host that wires backbone-employee injects an adapter over its
//! export, so the SQL lives in exactly one place. The default [`PoolEmployeeStatutoryInputs`]
//! re-expresses that same read as plain SQL so payroll works standalone (tests, module-only
//! deployments). Both must agree: same tables, same soft-delete predicates, same override-wins
//! PTKP derivation.

use async_trait::async_trait;
use chrono::NaiveDate;
use sqlx::PgPool;
use uuid::Uuid;

/// Payroll's mirror of the employee module's statutory bundle. Field semantics match the export:
/// `ptkp` is the override-else-derived tier key (`"tk0".."k3"`); `ter_category` `None` means the
/// progressive-bracket path; `join_date` `None` means tenure unknown (THR yields zero).
#[derive(Debug, Clone, PartialEq)]
pub struct EmployeeStatutory {
    pub ptkp: String,
    pub has_npwp: bool,
    pub bpjs_kesehatan_family: Option<i32>,
    pub join_date: Option<NaiveDate>,
    pub ter_category: Option<String>,
}

/// Reads one employee's statutory facts. `Ok(None)` = no such (live) employee in scope; missing
/// tax/BPJS rows degrade to defaults exactly like the employee module's own read.
#[async_trait]
pub trait EmployeeStatutoryInputs: Send + Sync {
    async fn statutory_inputs(&self, employee_id: Uuid) -> Result<Option<EmployeeStatutory>, sqlx::Error>;
}

/// Default pool-backed [`EmployeeStatutoryInputs`] — the employee module's exported read
/// (`statutory_row_for` + the PTKP override/derive rule), expressed as one plain SQL statement for
/// standalone use. Both modules are tenant-agnostic (ADR-0029): the employee table carries no
/// company column; the read is org-scoped only by the composing service's tenancy RLS fence.
pub struct PoolEmployeeStatutoryInputs {
    pool: PgPool,
}

impl PoolEmployeeStatutoryInputs {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// The raw projection of [`PoolEmployeeStatutoryInputs`]'s SQL — every employee-side row is
/// optional, so every tax/BPJS field is nullable.
#[derive(sqlx::FromRow)]
struct StatutoryRaw {
    ptkp_override: Option<String>,
    has_npwp: Option<bool>,
    ter_category: Option<String>,
    bpjs_kesehatan_family: Option<i32>,
    join_date: Option<NaiveDate>,
    has_spouse: bool,
    children: i64,
}

#[async_trait]
impl EmployeeStatutoryInputs for PoolEmployeeStatutoryInputs {
    async fn statutory_inputs(&self, employee_id: Uuid) -> Result<Option<EmployeeStatutory>, sqlx::Error> {
        let row: Option<StatutoryRaw> = sqlx::query_as(
            r#"SELECT
                 (SELECT t.ptkp_override::text
                    FROM employee.employee_taxes t
                   WHERE t.employee_id = e.id AND (t.metadata->>'deleted_at') IS NULL
                   LIMIT 1) AS ptkp_override,
                 (SELECT NULLIF(TRIM(t.npwp_number), '') IS NOT NULL
                    FROM employee.employee_taxes t
                   WHERE t.employee_id = e.id AND (t.metadata->>'deleted_at') IS NULL
                   LIMIT 1) AS has_npwp,
                 (SELECT t.ter_category::text
                    FROM employee.employee_taxes t
                   WHERE t.employee_id = e.id AND (t.metadata->>'deleted_at') IS NULL
                   LIMIT 1) AS ter_category,
                 (SELECT b.bpjs_kesehatan_family
                    FROM employee.employee_bpjs b
                   WHERE b.employee_id = e.id AND (b.metadata->>'deleted_at') IS NULL
                   LIMIT 1) AS bpjs_kesehatan_family,
                 (SELECT em.join_date
                    FROM employee.employments em
                   WHERE em.employee_id = e.id AND (em.metadata->>'deleted_at') IS NULL
                   ORDER BY em.join_date
                   LIMIT 1) AS join_date,
                 EXISTS (SELECT 1
                           FROM employee.employee_families f
                          WHERE f.employee_id = e.id
                            AND f.relationship = 'spouse'::family_relationship
                            AND (f.metadata->>'deleted_at') IS NULL) AS has_spouse,
                 (SELECT COUNT(*)
                    FROM employee.employee_families f
                   WHERE f.employee_id = e.id
                     AND f.relationship = 'child'::family_relationship
                     AND (f.metadata->>'deleted_at') IS NULL) AS children
               FROM employee.employees e
              WHERE e.id = $1
                AND (e.metadata->>'deleted_at') IS NULL
              LIMIT 1"#,
        )
        .bind(employee_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|r| {
            // Override wins, else derive from dependents: K/k when a spouse exists, TK/tk without;
            // dependent children cap at 3 (the law's K3/TK3 ceiling).
            let dependents = r.children.min(3);
            let ptkp = r.ptkp_override.unwrap_or_else(|| {
                if r.has_spouse {
                    format!("k{dependents}")
                } else {
                    format!("tk{dependents}")
                }
            });
            EmployeeStatutory {
                ptkp,
                has_npwp: r.has_npwp.unwrap_or(false),
                bpjs_kesehatan_family: r.bpjs_kesehatan_family,
                join_date: r.join_date,
                ter_category: r.ter_category,
            }
        }))
    }
}
