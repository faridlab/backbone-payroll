//! The SQL behind changing a salary structure from a date (user-owned; declared in
//! `metaphor.codegen.yaml`).
//!
//! A component's versions say what it is from each date on. The component row carries the version
//! in force today, so every plain read of a structure keeps working; a run reads the version in
//! force on its period's first day (see `SalaryComponentRepository::list_by_structure`).
//!
//! Every statement takes the caller's connection so a scheduled change commits as one unit. The
//! caller relays the ambient org request scope onto it first (ADR-0029).

use chrono::NaiveDate;
use rust_decimal::Decimal;
use sqlx::{PgConnection, PgPool, Row};
use uuid::Uuid;

use backbone_orm::org_scope::fetch_all_rows_scoped;

/// The date a component's first version starts from when it existed before any dated change: it
/// covers every period before the first change, so a run for an earlier month still reads what the
/// component was then.
pub const BASELINE_FROM: NaiveDate = match NaiveDate::from_ymd_opt(1900, 1, 1) {
    Some(d) => d,
    None => panic!("1900-01-01 is a date"),
};

/// One component of a structure as it stands, locked for the change.
#[derive(Debug, Clone)]
pub struct StandingComponent {
    pub id: Uuid,
    pub name: String,
    pub component_type: String,
    pub amount: Decimal,
    pub gl_account_id: Uuid,
    pub is_statutory: bool,
    /// The row is soft-deleted: removed, or added from a date still to come.
    pub deleted: bool,
    pub has_versions: bool,
}

/// One version as written.
pub struct VersionRow<'a> {
    pub component_id: Uuid,
    pub effective_from: NaiveDate,
    pub name: &'a str,
    pub component_type: &'a str,
    pub amount: Decimal,
    pub gl_account_id: Uuid,
    pub is_statutory: bool,
    pub removed: bool,
    pub reason: Option<&'a str>,
}

/// One version as the structure's history reads it.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StructureVersion {
    pub id: Uuid,
    pub component_id: Uuid,
    pub effective_from: NaiveDate,
    pub name: String,
    pub component_type: String,
    pub amount: Decimal,
    pub gl_account_id: Uuid,
    pub is_statutory: bool,
    pub removed: bool,
    pub reason: Option<String>,
    /// The version that holds from the start, written when the first dated change was planned.
    pub baseline: bool,
    pub created_at: Option<String>,
    pub created_by: Option<String>,
}

/// A run a dated change reaches.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunReached {
    pub id: Uuid,
    pub status: String,
    pub period_year: i32,
    pub period_month: i32,
}

pub struct PayScheduleRepository;

impl PayScheduleRepository {
    /// The structure's components, removed ones included, locked until the change commits so two
    /// planners cannot interleave their versions.
    pub async fn lock_components(
        conn: &mut PgConnection,
        structure_id: Uuid,
    ) -> Result<Vec<StandingComponent>, sqlx::Error> {
        let rows = sqlx::query(
            r#"SELECT c.id, c.name, c.component_type::text AS ct, c.amount, c.gl_account_id, c.is_statutory,
                      (c.metadata->>'deleted_at') IS NOT NULL AS deleted,
                      EXISTS (SELECT 1 FROM payroll.salary_component_versions v
                               WHERE v.component_id = c.id AND (v.metadata->>'deleted_at') IS NULL) AS has_versions
                 FROM payroll.salary_components c
                WHERE c.structure_id = $1
                ORDER BY c.id
                  FOR UPDATE OF c"#,
        )
        .bind(structure_id)
        .fetch_all(conn)
        .await?;
        Ok(rows
            .iter()
            .map(|r| StandingComponent {
                id: r.get("id"),
                name: r.get("name"),
                component_type: r.get("ct"),
                amount: r.get("amount"),
                gl_account_id: r.get("gl_account_id"),
                is_statutory: r.get("is_statutory"),
                deleted: r.get("deleted"),
                has_versions: r.get("has_versions"),
            })
            .collect())
    }

    /// Write a version; a second change planned for the same component and date replaces the first.
    pub async fn upsert_version(conn: &mut PgConnection, v: &VersionRow<'_>) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"INSERT INTO payroll.salary_component_versions
                 (id, component_id, effective_from, name, component_type, amount, gl_account_id,
                  is_statutory, removed, reason)
               VALUES ($1,$2,$3,$4,$5::component_type,$6,$7,$8,$9,$10)
               ON CONFLICT (component_id, effective_from) DO UPDATE
                  SET name = EXCLUDED.name, component_type = EXCLUDED.component_type,
                      amount = EXCLUDED.amount, gl_account_id = EXCLUDED.gl_account_id,
                      is_statutory = EXCLUDED.is_statutory, removed = EXCLUDED.removed,
                      reason = EXCLUDED.reason"#,
        )
        .bind(Uuid::new_v4())
        .bind(v.component_id)
        .bind(v.effective_from)
        .bind(v.name)
        .bind(v.component_type)
        .bind(v.amount)
        .bind(v.gl_account_id)
        .bind(v.is_statutory)
        .bind(v.removed)
        .bind(v.reason)
        .execute(conn)
        .await?;
        Ok(())
    }

    /// Insert a component a change adds. It goes in soft-deleted, so the structure as it reads
    /// today does not show it; [`Self::apply_due`] brings it in once its date arrives.
    pub async fn insert_added_component(
        conn: &mut PgConnection,
        id: Uuid,
        structure_id: Uuid,
        name: &str,
        component_type: &str,
        amount: Decimal,
        gl_account_id: Uuid,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"INSERT INTO payroll.salary_components
                 (id, structure_id, name, component_type, amount, gl_account_id)
               VALUES ($1,$2,$3,$4::component_type,$5,$6)"#,
        )
        .bind(id)
        .bind(structure_id)
        .bind(name)
        .bind(component_type)
        .bind(amount)
        .bind(gl_account_id)
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "UPDATE payroll.salary_components SET metadata = jsonb_set(metadata, '{deleted_at}', to_jsonb(now())) WHERE id = $1",
        )
        .bind(id)
        .execute(&mut *conn)
        .await?;
        Ok(())
    }

    /// Bring each component row to its version in force on `today`: its values, and removed or not.
    /// Rows already there are not touched, so running it again changes nothing. `structure_id`
    /// narrows it to one structure; `None` walks every structure in scope. Returns the rows moved.
    pub async fn apply_due(
        conn: &mut PgConnection,
        structure_id: Option<Uuid>,
        today: NaiveDate,
    ) -> Result<u64, sqlx::Error> {
        let r = sqlx::query(
            r#"WITH due AS (
                   SELECT DISTINCT ON (v.component_id)
                          v.component_id, v.name, v.component_type, v.amount, v.gl_account_id,
                          v.is_statutory, v.removed
                     FROM payroll.salary_component_versions v
                     JOIN payroll.salary_components c ON c.id = v.component_id
                    WHERE v.effective_from <= $1
                      AND (v.metadata->>'deleted_at') IS NULL
                      AND ($2::uuid IS NULL OR c.structure_id = $2)
                    ORDER BY v.component_id, v.effective_from DESC)
               UPDATE payroll.salary_components c
                  SET name = d.name, component_type = d.component_type, amount = d.amount,
                      gl_account_id = d.gl_account_id, is_statutory = d.is_statutory,
                      metadata = CASE WHEN d.removed
                                      THEN jsonb_set(c.metadata, '{deleted_at}', to_jsonb(now()))
                                      ELSE jsonb_set(c.metadata, '{deleted_at}', 'null'::jsonb) END
                 FROM due d
                WHERE c.id = d.component_id
                  AND ((c.name, c.component_type, c.amount, c.gl_account_id, c.is_statutory)
                         IS DISTINCT FROM (d.name, d.component_type, d.amount, d.gl_account_id, d.is_statutory)
                       OR d.removed <> ((c.metadata->>'deleted_at') IS NOT NULL))"#,
        )
        .bind(today)
        .bind(structure_id)
        .execute(conn)
        .await?;
        Ok(r.rows_affected())
    }

    /// Withdraw the change planned for a date still to come: its versions go, and a component it
    /// would have added goes with them. Returns how many versions were withdrawn.
    pub async fn cancel_planned(
        conn: &mut PgConnection,
        structure_id: Uuid,
        effective_from: NaiveDate,
    ) -> Result<u64, sqlx::Error> {
        // A component whose every version starts on that date was only ever planned: it was never
        // paid, so nothing refers to it once its versions are gone.
        let added: Vec<Uuid> = sqlx::query_scalar(
            r#"SELECT c.id FROM payroll.salary_components c
                WHERE c.structure_id = $1
                  AND EXISTS (SELECT 1 FROM payroll.salary_component_versions v
                               WHERE v.component_id = c.id AND v.effective_from = $2)
                  AND NOT EXISTS (SELECT 1 FROM payroll.salary_component_versions v
                                   WHERE v.component_id = c.id AND v.effective_from <> $2)"#,
        )
        .bind(structure_id)
        .bind(effective_from)
        .fetch_all(&mut *conn)
        .await?;
        let gone = sqlx::query(
            r#"DELETE FROM payroll.salary_component_versions v
                USING payroll.salary_components c
                WHERE c.id = v.component_id AND c.structure_id = $1 AND v.effective_from = $2"#,
        )
        .bind(structure_id)
        .bind(effective_from)
        .execute(&mut *conn)
        .await?
        .rows_affected();
        if !added.is_empty() {
            sqlx::query("DELETE FROM payroll.salary_components WHERE id = ANY($1)")
                .bind(&added)
                .execute(&mut *conn)
                .await?;
        }
        Ok(gone)
    }

    /// Whether the structure exists in scope (the fence hides another unit's).
    pub async fn structure_exists(conn: &mut PgConnection, structure_id: Uuid) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM payroll.salary_structures WHERE id = $1 AND (metadata->>'deleted_at') IS NULL)",
        )
        .bind(structure_id)
        .fetch_one(conn)
        .await
    }

    /// The runs a change from `effective_from` reaches: every run not cancelled whose period starts
    /// on or after that day and that already holds a slip on the structure, earliest first.
    pub async fn runs_reached(
        conn: &mut PgConnection,
        structure_id: Uuid,
        effective_from: NaiveDate,
    ) -> Result<Vec<RunReached>, sqlx::Error> {
        let rows = sqlx::query(
            r#"SELECT e.id, e.status::text AS status, e.period_year, e.period_month
                 FROM payroll.payroll_entries e
                WHERE e.status::text <> 'cancelled'
                  AND COALESCE(e.period_start, make_date(e.period_year, e.period_month, 1)) >= $2
                  AND EXISTS (SELECT 1 FROM payroll.salary_slips s
                               WHERE s.payroll_entry_id = e.id AND s.structure_id = $1)
                ORDER BY e.period_year, e.period_month"#,
        )
        .bind(structure_id)
        .bind(effective_from)
        .fetch_all(conn)
        .await?;
        Ok(rows
            .iter()
            .map(|r| RunReached {
                id: r.get("id"),
                status: r.get("status"),
                period_year: r.get("period_year"),
                period_month: r.get("period_month"),
            })
            .collect())
    }

    /// Every version of the structure's components, oldest first: what it was, what it is, and
    /// what is planned.
    pub async fn structure_versions(
        pool: &PgPool,
        structure_id: Uuid,
    ) -> Result<Vec<StructureVersion>, sqlx::Error> {
        let rows = fetch_all_rows_scoped(
            pool,
            sqlx::query(
                r#"SELECT v.id, v.component_id, v.effective_from, v.name, v.component_type::text AS ct,
                          v.amount, v.gl_account_id, v.is_statutory, v.removed, v.reason,
                          v.metadata->>'created_at' AS created_at, v.metadata->>'created_by' AS created_by
                     FROM payroll.salary_component_versions v
                     JOIN payroll.salary_components c ON c.id = v.component_id
                    WHERE c.structure_id = $1 AND (v.metadata->>'deleted_at') IS NULL
                    ORDER BY v.effective_from, v.name"#,
            )
            .bind(structure_id),
        )
        .await?;
        Ok(rows
            .iter()
            .map(|r| {
                let effective_from: NaiveDate = r.get("effective_from");
                StructureVersion {
                    id: r.get("id"),
                    component_id: r.get("component_id"),
                    effective_from,
                    name: r.get("name"),
                    component_type: r.get("ct"),
                    amount: r.get("amount"),
                    gl_account_id: r.get("gl_account_id"),
                    is_statutory: r.get("is_statutory"),
                    removed: r.get("removed"),
                    reason: r.get("reason"),
                    baseline: effective_from == BASELINE_FROM,
                    created_at: r.get("created_at"),
                    created_by: r.get("created_by"),
                }
            })
            .collect())
    }
}
