//! Changing a salary structure from a date (hand-written; user-owned).
//!
//! A change names the date it takes effect and the items it sets from then: an amount, a name, a
//! new item, or an item no longer paid. Payslips for a period before the date keep what the
//! structure was; a run for a period starting on or after it reads the change. A change dated today
//! or earlier is in force at once; a later one waits, and the daily pass brings the structure as it
//! reads today up to it when its day comes.
//!
//! The rules the service decides, so every screen states the same thing:
//! - a change cannot reach a run that is already processed or posted — those slips are final, and
//!   back pay is not a structure change;
//! - a change can be withdrawn only while its date is still to come;
//! - the first dated change of an item first records what the item was, from the start, so a run
//!   for an earlier month still reads it.

use backbone_orm::org_scope;
use chrono::NaiveDate;
use rust_decimal::{Decimal, RoundingStrategy};
use sqlx::PgPool;
use uuid::Uuid;

use crate::infrastructure::persistence::{
    PayScheduleRepository, RunReached, StructureVersion, VersionRow, BASELINE_FROM,
};

use super::payroll_write_service::PayrollError;

/// One item a change sets from its date.
#[derive(Debug, Clone)]
pub struct PlannedItem {
    /// The item changed or removed; `None` adds a new one.
    pub component_id: Option<Uuid>,
    pub name: String,
    pub component_type: String,
    pub amount: Decimal,
    pub gl_account_id: Uuid,
    pub is_statutory: bool,
    /// The item is no longer paid from the date.
    pub removed: bool,
}

/// A change to a structure from a date.
#[derive(Debug, Clone)]
pub struct PlannedChange {
    pub structure_id: Uuid,
    pub effective_from: NaiveDate,
    pub reason: Option<String>,
    pub items: Vec<PlannedItem>,
}

/// What planning a change did.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanOutcome {
    pub structure_id: Uuid,
    pub effective_from: NaiveDate,
    /// Items the change sets.
    pub items: usize,
    /// The date has come: the structure reads the change today.
    pub in_force: bool,
    /// Draft runs from the date on whose slips were worked out before the change: generate their
    /// slips again for them to read it.
    pub draft_runs_to_redo: Vec<RunReached>,
}

fn money(v: Decimal) -> Decimal {
    v.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)
}

const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September",
    "October", "November", "December",
];

fn run_label(r: &RunReached) -> String {
    let m = usize::try_from(r.period_month - 1).ok().and_then(|i| MONTHS.get(i)).copied().unwrap_or("?");
    format!("{m} {}", r.period_year)
}

fn check_item(it: &PlannedItem) -> Result<(), PayrollError> {
    if it.name.trim().is_empty() {
        return Err(PayrollError::Invalid("every pay item needs a name".into()));
    }
    if it.name.chars().count() > 140 {
        return Err(PayrollError::Invalid("a pay item name is at most 140 characters".into()));
    }
    if it.component_type != "earning" && it.component_type != "deduction" {
        return Err(PayrollError::Invalid(format!(
            "a pay item is an earning or a deduction, not '{}'",
            it.component_type
        )));
    }
    if it.amount < Decimal::ZERO {
        return Err(PayrollError::Invalid("a pay item amount cannot be negative".into()));
    }
    if it.removed && it.component_id.is_none() {
        return Err(PayrollError::Invalid("only an existing pay item can be removed".into()));
    }
    Ok(())
}

/// Plan a change from its date, as one unit. `today` decides whether it is in force at once.
pub async fn plan_change(
    pool: &PgPool,
    c: PlannedChange,
    today: NaiveDate,
) -> Result<PlanOutcome, PayrollError> {
    if c.items.is_empty() {
        return Err(PayrollError::Invalid("a change sets at least one pay item".into()));
    }
    if c.effective_from <= BASELINE_FROM {
        return Err(PayrollError::Invalid("a change needs a real date to take effect from".into()));
    }
    if c.reason.as_deref().map(|r| r.chars().count() > 500).unwrap_or(false) {
        return Err(PayrollError::Invalid("the reason is at most 500 characters".into()));
    }
    let mut seen = std::collections::HashSet::new();
    for it in &c.items {
        check_item(it)?;
        if let Some(id) = it.component_id {
            if !seen.insert(id) {
                return Err(PayrollError::Invalid("a change sets each pay item once".into()));
            }
        }
    }
    let reason = c.reason.as_deref().map(str::trim).filter(|r| !r.is_empty());

    // Tenancy (ADR-0029): relay the ambient org request scope onto our own transaction so every
    // write passes the composing service's fence; an undecorated deployment is unfenced by design.
    let mut tx = pool.begin().await?;
    if let Some(scope) = org_scope::current_org_scope() {
        org_scope::bind_org_scope_on(&mut tx, &scope).await?;
    }
    if !PayScheduleRepository::structure_exists(&mut tx, c.structure_id).await? {
        return Err(PayrollError::NotFound("salary structure"));
    }
    let standing = PayScheduleRepository::lock_components(&mut tx, c.structure_id).await?;

    let reached = PayScheduleRepository::runs_reached(&mut tx, c.structure_id, c.effective_from).await?;
    if let Some(done) = reached.iter().find(|r| r.status != "draft") {
        return Err(PayrollError::Invalid(format!(
            "the {} payroll run is already {}; plan the change from after its period",
            run_label(done),
            done.status
        )));
    }

    for it in &c.items {
        let component_id = match it.component_id {
            Some(id) => {
                let cur = standing
                    .iter()
                    .find(|s| s.id == id)
                    .ok_or(PayrollError::NotFound("pay item on this structure"))?;
                if !cur.has_versions {
                    if cur.deleted {
                        return Err(PayrollError::Invalid(format!(
                            "{} was removed from the structure; add it again as a new item",
                            cur.name
                        )));
                    }
                    // What the item was until now, from the start: a run for an earlier month
                    // still reads it once the item carries versions.
                    PayScheduleRepository::upsert_version(&mut tx, &VersionRow {
                        component_id: id,
                        effective_from: BASELINE_FROM,
                        name: &cur.name,
                        component_type: &cur.component_type,
                        amount: cur.amount,
                        gl_account_id: cur.gl_account_id,
                        is_statutory: cur.is_statutory,
                        removed: false,
                        reason: None,
                    })
                    .await?;
                }
                id
            }
            None => {
                let id = Uuid::new_v4();
                PayScheduleRepository::insert_added_component(
                    &mut tx,
                    id,
                    c.structure_id,
                    it.name.trim(),
                    &it.component_type,
                    money(it.amount),
                    it.gl_account_id,
                )
                .await?;
                id
            }
        };
        PayScheduleRepository::upsert_version(&mut tx, &VersionRow {
            component_id,
            effective_from: c.effective_from,
            name: it.name.trim(),
            component_type: &it.component_type,
            amount: money(it.amount),
            gl_account_id: it.gl_account_id,
            is_statutory: it.is_statutory,
            removed: it.removed,
            reason,
        })
        .await?;
    }

    let in_force = c.effective_from <= today;
    if in_force {
        PayScheduleRepository::apply_due(&mut tx, Some(c.structure_id), today).await?;
    }
    tx.commit().await?;
    Ok(PlanOutcome {
        structure_id: c.structure_id,
        effective_from: c.effective_from,
        items: c.items.len(),
        in_force,
        draft_runs_to_redo: reached,
    })
}

/// Withdraw the change planned for a date still to come. Returns how many items it set.
pub async fn withdraw_change(
    pool: &PgPool,
    structure_id: Uuid,
    effective_from: NaiveDate,
    today: NaiveDate,
) -> Result<u64, PayrollError> {
    if effective_from <= today {
        return Err(PayrollError::Invalid(
            "that change is already in force; plan another change from a later date instead".into(),
        ));
    }
    let mut tx = pool.begin().await?;
    if let Some(scope) = org_scope::current_org_scope() {
        org_scope::bind_org_scope_on(&mut tx, &scope).await?;
    }
    if !PayScheduleRepository::structure_exists(&mut tx, structure_id).await? {
        return Err(PayrollError::NotFound("salary structure"));
    }
    PayScheduleRepository::lock_components(&mut tx, structure_id).await?;
    let gone = PayScheduleRepository::cancel_planned(&mut tx, structure_id, effective_from).await?;
    if gone == 0 {
        return Err(PayrollError::NotFound("change planned for that date"));
    }
    tx.commit().await?;
    Ok(gone)
}

/// Bring every structure as it reads today up to the changes whose day has come. Running it again
/// changes nothing. Returns how many pay items moved.
pub async fn apply_due_changes(pool: &PgPool, today: NaiveDate) -> Result<u64, PayrollError> {
    let mut tx = pool.begin().await?;
    if let Some(scope) = org_scope::current_org_scope() {
        org_scope::bind_org_scope_on(&mut tx, &scope).await?;
    }
    let moved = PayScheduleRepository::apply_due(&mut tx, None, today).await?;
    tx.commit().await?;
    Ok(moved)
}

/// Every version of a structure's items, oldest first: what it was, is, and will be.
pub async fn structure_versions(
    pool: &PgPool,
    structure_id: Uuid,
) -> Result<Vec<StructureVersion>, PayrollError> {
    Ok(PayScheduleRepository::structure_versions(pool, structure_id).await?)
}
