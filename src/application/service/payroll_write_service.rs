//! The hand-authored payroll write path (user-owned; survives regen).
//!
//! A salary run: assemble per-employee slips (earnings from a structure, prorated for HR unpaid days,
//! minus fixed + supplied statutory deductions), roll up the run totals, and post ONE balanced salary
//! journal to the GL — the **8th GL producer**: `Dr Salary Expense (gross) · Cr Salary Payable (net) ·
//! Cr statutory/other payables (grouped by account)`. Because `gross = net + Σ deductions`, it balances.
//! Idempotent per run (source_id = run id). Reads the HR employee via `period_summary`-style inputs;
//! the Indonesia statutory amounts (BPJS, PPh 21) are supplied by the deferred overlay. Money is IDR,
//! 2dp, half-away-from-zero.

use backbone_orm::org_scope;
use chrono::{Datelike, NaiveDate};
use rust_decimal::{Decimal, RoundingStrategy};
use sqlx::PgPool;
use uuid::Uuid;

use crate::infrastructure::persistence::{
    NewComponentRow, NewPayrollEntryRow, NewSalarySlipRow, NewSlipLineRow, NewStructureRow,
    PayrollEntryRepository, SalaryComponentRepository, SalarySlipLineRepository, SalarySlipRepository,
    SalaryStructureRepository, StatutoryParamsRepository,
};

use super::employee_inputs_port::{EmployeeStatutoryInputs, PoolEmployeeStatutoryInputs};
use super::overtime_port::{OvertimeInputs, PoolOvertimeInputs};
use super::payroll_events::*;
use super::payroll_gl::*;
use super::payroll_remittance::{
    RemitAck, RemittanceInstruction, RemittanceSeamError, RemittanceSink, UnwiredRemittance,
};
use super::statutory_calcs::{self, Pph21Method, PtkpTier};

fn money(v: Decimal) -> Decimal {
    v.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)
}

/// The legacy tenancy twin echo (ADR-0029): outbound contract shapes (the GL post envelope, the
/// `PayrollPosted` event, the remittance instruction) carry a `company_id` field for unstripped
/// consumers, but the stripped tables hold no company column. Echo the ambient org scope's legacy
/// company id when the composing service bound one; nil otherwise. Nothing keys a statement on it,
/// and an undecorated deployment is unfenced by design.
fn legacy_company_echo() -> Uuid {
    org_scope::current_org_scope()
        .and_then(|s| s.legacy_company_id())
        .unwrap_or(Uuid::nil())
}

#[derive(Debug, thiserror::Error)]
pub enum PayrollError {
    #[error("db: {0}")]
    Db(#[from] sqlx::Error),
    #[error("not found: {0}")]
    NotFound(&'static str),
    #[error("invalid state: {0}")]
    InvalidState(&'static str),
    #[error("invalid input: {0}")]
    Invalid(String),
    #[error("unbalanced posting")]
    Unbalanced,
    #[error("gl rejected: {0}")]
    GlRejected(String),
    /// The post/remit row landed but the event sink refused the event — re-run the post verb; the
    /// already-posted branch re-publishes (at-least-once), and consumers dedup by record id.
    #[error("event publish failed after the post landed — re-run the post verb to re-publish: {0}")]
    EventPublish(String),
    #[error(transparent)]
    Remittance(#[from] RemittanceSeamError),
    /// The statutory parameter resolution refused to compute (no effective rows for the period,
    /// an incomplete component set, …). Fail-closed by design: never a silent zero tax/pay.
    #[error(transparent)]
    Statutory(#[from] statutory_calcs::StatutoryError),
}

impl PayrollError {
    /// Stable machine code the HTTP layer surfaces.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Db(_) => "internal_error",
            Self::NotFound(_) => "not_found",
            Self::InvalidState(_) => "invalid_state",
            Self::Invalid(_) => "invalid_input",
            Self::Unbalanced => "unbalanced",
            Self::GlRejected(code) => match code.as_str() {
                "gl_seam_unwired" => "gl_seam_unwired",
                _ => "gl_rejected",
            },
            Self::EventPublish(_) => "event_publish_failed",
            Self::Remittance(seam) => match seam.code() {
                "remittance_seam_unwired" => "remittance_seam_unwired",
                "remittance_rejected" => "remittance_rejected",
                _ => "remittance_seam_error",
            },
            Self::Statutory(e) => match e {
                statutory_calcs::StatutoryError::NoParamsForPeriod(..) => {
                    "no_statutory_params_for_period"
                }
                statutory_calcs::StatutoryError::UnknownPtkpTier(_) => "unknown_ptkp_tier",
                statutory_calcs::StatutoryError::UnknownRiskClass(_) => "unknown_risk_class",
                statutory_calcs::StatutoryError::UnknownTerCategory(_) => "unknown_ter_category",
                statutory_calcs::StatutoryError::NoTerRates(_) => "no_ter_rates",
                statutory_calcs::StatutoryError::MissingOvertimeBands => "no_overtime_bands",
                _ => "statutory_calc_error",
            },
        }
    }

    /// The HTTP status the guarded surface maps this error to. Client-shaped failures (bad input,
    /// wrong state, unwired seams the operator must compose, missing effective parameters) are
    /// 4xx/422 so the caller can distinguish them from infrastructure faults.
    pub fn http_status(&self) -> u16 {
        match self {
            Self::Db(_) | Self::EventPublish(_) => 500,
            Self::NotFound(_) => 404,
            Self::InvalidState(_) | Self::Invalid(_) | Self::Unbalanced => 422,
            Self::GlRejected(_) | Self::Remittance(_) => 422,
            Self::Statutory(e) => match e {
                // Data-presence failures (an incomplete or non-covering effective set, an axis
                // value the set has no row for) are client-shaped: the operator seeds the missing
                // effective set; only parse/IO/db faults are infrastructure.
                statutory_calcs::StatutoryError::NoParamsForPeriod(..) => 422,
                statutory_calcs::StatutoryError::UnknownPtkpTier(_) => 422,
                statutory_calcs::StatutoryError::UnknownRiskClass(_) => 422,
                statutory_calcs::StatutoryError::UnknownTerCategory(_) => 422,
                statutory_calcs::StatutoryError::NoTerRates(_) => 422,
                statutory_calcs::StatutoryError::MissingOvertimeBands => 422,
                _ => 500,
            },
        }
    }
}

pub struct NewComponent {
    pub name: String,
    pub component_type: String, // earning | deduction
    pub amount: Decimal,
    pub gl_account_id: Uuid,
}
pub struct NewStructure {
    pub name: String,
    pub components: Vec<NewComponent>,
}

pub struct NewPayrollEntry {
    pub period_year: i32,
    pub period_month: i32,
    pub salary_expense_account_id: Uuid,
    pub salary_payable_account_id: Uuid,
}

/// A supplied Indonesia statutory component for a slip — PPh 21 / BPJS Kesehatan / BPJS
/// Ketenagakerjaan **deductions**, or a THR **earning**. Computed by the deferred statutory overlay
/// and supplied here like billing's tax lines.
///
/// `component_type` mirrors the structure-component vocabulary (`"earning"` | `"deduction"`): an
/// earning raises gross (un-prorated — THR carries its own tenure pro-rating), a deduction subtracts.
/// The slip-line marks either as `is_statutory: true` so the GL grouping can tell statutory payables
/// apart from structure deductions; the deduction grouping filters `component_type='deduction'`, so a
/// THR earning is never mis-routed to a payable account.
pub struct StatutoryLine {
    pub name: String,
    pub component_type: String, // "earning" | "deduction"
    pub amount: Decimal,
    pub gl_account_id: Uuid, // payable (deduction) or expense (earning) account
}
pub struct NewSalarySlip {
    pub employee_id: Uuid,
    pub structure_id: Uuid,
    /// Working days in the period (e.g. 22); earnings are prorated by (working − unpaid)/working.
    pub working_days: Decimal,
    /// Unpaid-leave + uncovered-absence days from `hr.period_summary` — reduce gross.
    pub unpaid_days: Decimal,
    pub statutory: Vec<StatutoryLine>,
    /// Overtime hours consumed while building this slip (0 when none) — stamped on the row as the
    /// audit snapshot. The PAY for these hours is an ordinary earning line the caller supplies in
    /// `statutory`/structure lines; the number here only records what the calculation used.
    pub overtime_hours: Decimal,
    /// The PPh-21 path dispatched for this slip (`npwp_brackets` | `ter_a` | `ter_b` | `ter_c`),
    /// stamped on the row for audit. None when no statutory tax path was computed.
    pub tax_method: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PostOutcome {
    pub payroll_entry_id: Uuid,
    pub journal_id: Uuid,
    pub post_id: Uuid,
    pub total_net: Decimal,
    pub already: bool,
}

/// What the remit verb sent — one (instruction, ack) pair per deduction payable, in send order.
#[derive(Debug, Clone, PartialEq)]
pub struct RemitOutcome {
    pub payroll_entry_id: Uuid,
    pub remitted: Vec<(RemittanceInstruction, RemitAck)>,
}

/// The GL payable accounts the computed-slip orchestrator books statutory deductions against —
/// supplied by the caller until the accounting composition resolves them itself (same stopgap
/// posture as a caller-supplied posting account set).
#[derive(Debug, Clone, Copy)]
pub struct StatutoryAccounts {
    pub pph21_payable: Uuid,
    pub bpjs_kesehatan_payable: Uuid,
    pub bpjs_ketenagakerjaan_payable: Uuid,
}

/// One computed slip: the orchestrator reads the run + the employee's statutory facts + the
/// effective parameter set, computes the statutory components and overtime pay, and delegates
/// the row writes to [`PayrollWriteService::add_salary_slip`].
pub struct ComputedSlipRequest {
    pub run_id: Uuid,
    pub employee_id: Uuid,
    pub structure_id: Uuid,
    pub working_days: Decimal,
    pub unpaid_days: Decimal,
    /// BPJS JKK risk class 1..=5 — caller-supplied until the HR master carries the field.
    pub risk_class: i32,
    /// Payable accounts for the statutory deductions (see [`StatutoryAccounts`]).
    pub accounts: StatutoryAccounts,
}

pub struct PayrollWriteService {
    pool: PgPool,
    structures: SalaryStructureRepository,
    components: SalaryComponentRepository,
    entries: PayrollEntryRepository,
    slips: SalarySlipRepository,
    slip_lines: SalarySlipLineRepository,
    params: StatutoryParamsRepository,
    overtime_inputs: Box<dyn OvertimeInputs>,
    employee_inputs: Box<dyn EmployeeStatutoryInputs>,
    gl_sink: std::sync::Arc<dyn GlPostSink>,
    event_sink: std::sync::Arc<dyn PayrollEventSink>,
    remit_sink: std::sync::Arc<dyn RemittanceSink>,
}

impl PayrollWriteService {
    pub fn new(pool: PgPool) -> Self {
        let structures = SalaryStructureRepository::new(pool.clone());
        let components = SalaryComponentRepository::new(pool.clone());
        let entries = PayrollEntryRepository::new(pool.clone());
        let slips = SalarySlipRepository::new(pool.clone());
        let slip_lines = SalarySlipLineRepository::new(pool.clone());
        let params = StatutoryParamsRepository::new(pool.clone());
        // Pool defaults so payroll computes standalone; a host composing the attendance or
        // employee modules overrides with an adapter over their exports (one SQL owner each).
        let overtime_inputs: Box<dyn OvertimeInputs> = Box::new(PoolOvertimeInputs::new(pool.clone()));
        let employee_inputs: Box<dyn EmployeeStatutoryInputs> =
            Box::new(PoolEmployeeStatutoryInputs::new(pool.clone()));
        Self {
            pool,
            structures,
            components,
            entries,
            slips,
            slip_lines,
            params,
            overtime_inputs,
            employee_inputs,
            // Module-held seams, fail-closed by default: an unwired deployment's post/remit verbs
            // refuse with the stable seam codes instead of pretending the effect happened.
            gl_sink: std::sync::Arc::new(UnwiredGlSink),
            event_sink: std::sync::Arc::new(LoggingSink),
            remit_sink: std::sync::Arc::new(UnwiredRemittance),
        }
    }

    /// Override where overtime hours come from (default: the pool read mirroring attendance's
    /// export).
    pub fn with_overtime_inputs(mut self, inputs: Box<dyn OvertimeInputs>) -> Self {
        self.overtime_inputs = inputs;
        self
    }

    /// Override where employee statutory facts come from (default: the pool read mirroring the
    /// employee module's export).
    pub fn with_employee_inputs(mut self, inputs: Box<dyn EmployeeStatutoryInputs>) -> Self {
        self.employee_inputs = inputs;
        self
    }

    /// Override the GL-posting seam (default [`UnwiredGlSink`] — post refuses with
    /// `gl_seam_unwired`).
    pub fn with_gl_sink(mut self, sink: std::sync::Arc<dyn GlPostSink>) -> Self {
        self.gl_sink = sink;
        self
    }

    /// Override the domain-event sink (default [`LoggingSink`]). A durable composition stages into
    /// an outbox here.
    pub fn with_event_sink(mut self, sink: std::sync::Arc<dyn PayrollEventSink>) -> Self {
        self.event_sink = sink;
        self
    }

    /// Override the remittance seam (default [`UnwiredRemittance`] — remit refuses with
    /// `remittance_seam_unwired`).
    pub fn with_remit_sink(mut self, sink: std::sync::Arc<dyn RemittanceSink>) -> Self {
        self.remit_sink = sink;
        self
    }

    /// Post through the module-held GL + event seams — the composition-root convenience over
    /// [`Self::post_payroll_entry`] (which stays public for callers supplying their own sinks,
    /// e.g. tests driving a real accounting adapter).
    pub async fn post_run(&self, run_id: Uuid, posting_date: NaiveDate) -> Result<PostOutcome, PayrollError> {
        self.post_payroll_entry(run_id, posting_date, &*self.gl_sink, &*self.event_sink).await
    }

    /// Remit through the module-held remittance seam — the composition-root convenience over
    /// [`Self::remit_payroll_entry`].
    pub async fn remit_run(&self, run_id: Uuid) -> Result<RemitOutcome, PayrollError> {
        self.remit_payroll_entry(run_id, &*self.remit_sink).await
    }

    /// Define a salary structure with its earning/deduction components.
    pub async fn create_structure(&self, s: NewStructure) -> Result<Uuid, PayrollError> {
        if s.name.trim().is_empty() {
            return Err(PayrollError::Invalid("structure needs a name".into()));
        }
        if s.components.is_empty() {
            return Err(PayrollError::Invalid("a structure needs at least one component".into()));
        }
        let id = Uuid::new_v4();
        // Tenancy (ADR-0029): the module is tenant-agnostic. Relay the ambient org request scope
        // onto our own transaction so the structure + component inserts pass the composing
        // service's tenancy RLS fence; an undecorated deployment is unfenced by design.
        let mut tx = self.pool.begin().await?;
        if let Some(scope) = org_scope::current_org_scope() {
            org_scope::bind_org_scope_on(&mut tx, &scope).await?;
        }
        self.structures.insert_structure(&mut tx, &NewStructureRow {
            id,
            name: &s.name,
        }).await?;
        for c in &s.components {
            if c.amount < Decimal::ZERO {
                return Err(PayrollError::Invalid("component amount must be non-negative".into()));
            }
            self.components.insert_component(&mut tx, &NewComponentRow {
                id: Uuid::new_v4(),
                structure_id: id,
                name: &c.name,
                component_type: &c.component_type,
                amount: money(c.amount),
                gl_account_id: c.gl_account_id,
            }).await?;
        }
        tx.commit().await?;
        Ok(id)
    }

    /// Open a payroll run for a period (draft). Unique per (org unit, year, month) once the
    /// composing service's tenancy decorator has re-declared the run unique org-scoped.
    pub async fn create_payroll_entry(&self, e: NewPayrollEntry) -> Result<Uuid, PayrollError> {
        if !(1..=12).contains(&e.period_month) {
            return Err(PayrollError::Invalid("period_month must be 1..12".into()));
        }
        let id = Uuid::new_v4();
        // Tenancy (ADR-0029): the insert rides the ambient org request scope — under HTTP the
        // request-dedicated connection already carries it; an undecorated deployment is unfenced
        // by design.
        let r = self
            .entries
            .insert_entry(&self.pool, &NewPayrollEntryRow {
                id,
                period_year: e.period_year,
                period_month: e.period_month,
                salary_expense_account_id: e.salary_expense_account_id,
                salary_payable_account_id: e.salary_payable_account_id,
            })
            .await;
        match r {
            Ok(_) => Ok(id),
            Err(err) if err.as_database_error().map(|d| d.is_unique_violation()).unwrap_or(false) =>
                Err(PayrollError::Invalid("a payroll run already exists for this period".into())),
            Err(err) => Err(err.into()),
        }
    }

    /// Add an employee's slip to a DRAFT run. Earnings come from the structure, prorated by unpaid days
    /// (`gross = Σ earning · (working − unpaid)/working`); fixed + supplied statutory deductions subtract.
    /// `net = gross − deductions` and must be non-negative.
    pub async fn add_salary_slip(&self, run_id: Uuid, s: NewSalarySlip) -> Result<Uuid, PayrollError> {
        // Tenancy (ADR-0029), ID-only pattern: identified by the run id alone. The lookup rides the
        // ambient org request scope — under HTTP the request-dedicated connection carries it, so
        // another unit's run simply isn't found; an undecorated deployment is unfenced by design.
        let run = self.entries.find_state_by_id(&self.pool, run_id).await?
            .ok_or(PayrollError::NotFound("payroll run"))?;
        if run.status != "draft" {
            return Err(PayrollError::InvalidState("run is not draft"));
        }
        if s.working_days <= Decimal::ZERO {
            return Err(PayrollError::Invalid("working_days must be positive".into()));
        }
        // Clamp unpaid days to [0, working_days] so the proration factor stays in [0, 1]. Without the
        // LOWER clamp a negative unpaid_days (a bad upstream hr.period_summary value) drives factor > 1
        // and inflates gross ABOVE the structure — a balanced-but-over-booked salary journal (maturity
        // council 2026-07-08). The DB CHECKs in 20260708000100_payroll_balance_guards backstop any writer.
        let unpaid = s.unpaid_days.clamp(Decimal::ZERO, s.working_days);
        let factor = (s.working_days - unpaid) / s.working_days; // proration for unpaid days

        // Load the structure components.
        let comps = self.components.list_by_structure(&self.pool, s.structure_id).await?;
        if comps.is_empty() {
            return Err(PayrollError::Invalid("salary structure has no components".into()));
        }

        struct Line { name: String, ct: String, is_statutory: bool, amount: Decimal, account: Uuid }
        let mut lines: Vec<Line> = Vec::new();
        let (mut gross, mut deductions) = (Decimal::ZERO, Decimal::ZERO);
        for c in &comps {
            let ct = c.component_type.clone();
            let base = c.amount;
            let account = c.gl_account_id;
            if ct == "earning" {
                let amt = money(base * factor);
                gross += amt;
                lines.push(Line { name: c.name.clone(), ct, is_statutory: false, amount: amt, account });
            } else {
                deductions += base;
                lines.push(Line { name: c.name.clone(), ct, is_statutory: false, amount: base, account });
            }
        }
        for st in &s.statutory {
            if st.amount < Decimal::ZERO {
                return Err(PayrollError::Invalid("statutory amount must be non-negative".into()));
            }
            let amt = money(st.amount);
            // Route by component_type: a THR earning raises gross (un-prorated — THR already carries
            // its own tenure pro-rating); a deduction subtracts. The slip-line keeps the caller's
            // component_type so the GL deduction grouping (`component_type='deduction'`) excludes THR.
            let is_earning = st.component_type == "earning";
            if is_earning {
                gross += amt;
            } else {
                deductions += amt;
            }
            lines.push(Line {
                name: st.name.clone(),
                ct: st.component_type.clone(),
                is_statutory: true,
                amount: amt,
                account: st.gl_account_id,
            });
        }
        let net = gross - deductions;
        if net < Decimal::ZERO {
            return Err(PayrollError::Invalid("deductions exceed gross — net pay would be negative".into()));
        }

        let slip_id = Uuid::new_v4();
        let mut tx = self.pool.begin().await?;
        // Relay the ambient org request scope onto our own transaction so the slip + line inserts
        // pass the composing service's tenancy RLS fence (ADR-0029); an undecorated deployment is
        // unfenced by design.
        if let Some(scope) = org_scope::current_org_scope() {
            org_scope::bind_org_scope_on(&mut tx, &scope).await?;
        }
        let ins = self.slips.insert_slip(&mut tx, &NewSalarySlipRow {
            id: slip_id,
            payroll_entry_id: run_id,
            employee_id: s.employee_id,
            structure_id: s.structure_id,
            working_days: s.working_days,
            unpaid_days: unpaid,
            gross_pay: gross,
            total_deductions: deductions,
            net_pay: net,
            overtime_hours: Some(s.overtime_hours.round_dp(2)),
            tax_method: s.tax_method.clone(),
        }).await;
        if let Err(err) = ins {
            return Err(if err.as_database_error().map(|d| d.is_unique_violation()).unwrap_or(false) {
                PayrollError::Invalid("this employee already has a slip in this run".into())
            } else { err.into() });
        }
        for l in &lines {
            self.slip_lines.insert_line(&mut tx, &NewSlipLineRow {
                id: Uuid::new_v4(),
                salary_slip_id: slip_id,
                name: &l.name,
                component_type: &l.ct,
                is_statutory: l.is_statutory,
                amount: l.amount,
                gl_account_id: l.account,
            }).await?;
        }
        tx.commit().await?;
        Ok(slip_id)
    }

    /// Build one employee's slip end-to-end: period → effective statutory params → employee facts →
    /// TER/bracket dispatch → overtime pay → the same [`Self::add_salary_slip`] write path a manual
    /// caller uses. The statutory base is the structure's un-prorated monthly earning total (the
    /// salary being paid); overtime pay rides in as an ordinary earning line so gross stays balanced
    /// through the existing journal.
    pub async fn add_computed_salary_slip(&self, r: ComputedSlipRequest) -> Result<Uuid, PayrollError> {
        let risk_class = u8::try_from(r.risk_class)
            .ok()
            .filter(|rc| (1..=5).contains(rc))
            .ok_or_else(|| PayrollError::Invalid("risk_class must be 1..=5".into()))?;
        // ID-only read under the request scope (same fence posture as add_salary_slip).
        let run = self.entries.find_period_by_id(&self.pool, r.run_id).await?
            .ok_or(PayrollError::NotFound("payroll run"))?;
        if run.status != "draft" {
            return Err(PayrollError::InvalidState("run is not draft"));
        }
        let month = u32::try_from(run.period_month)
            .map_err(|_| PayrollError::Invalid("period_month is not a valid month".into()))?;
        let period_start = NaiveDate::from_ymd_opt(run.period_year, month, 1)
            .ok_or(PayrollError::Invalid("run period is not a real calendar month".into()))?;
        // Period end = day before the next month's first (year-rollover safe).
        let (ny, nm) = if month == 12 { (run.period_year + 1, 1) } else { (run.period_year, month + 1) };
        let period_end = NaiveDate::from_ymd_opt(ny, nm, 1)
            .and_then(|d| d.pred_opt())
            .ok_or(PayrollError::Invalid("run period end is not a real calendar date".into()))?;

        // Fail-closed parameter resolution: the effective set as of the period's first day. A period
        // before any seed date (or a table an operator emptied) refuses rather than zeroing tax.
        let cfg = self.params.resolve_as_of("ID", period_start).await?;

        // Employee facts (PTKP/NPWP/TER/tenure anchor) — None means no such live employee in scope.
        let inputs = self
            .employee_inputs
            .statutory_inputs(r.employee_id)
            .await?
            .ok_or(PayrollError::NotFound("employee statutory inputs"))?;

        // Statutory base: the structure's monthly earning total (un-prorated — the salary being
        // paid; proration is a slip-line concern the earnings factor already applies).
        let comps = self.components.list_by_structure(&self.pool, r.structure_id).await?;
        let gross_monthly: Decimal = comps
            .iter()
            .filter(|c| c.component_type == "earning")
            .map(|c| c.amount)
            .sum();
        if gross_monthly <= Decimal::ZERO {
            return Err(PayrollError::Invalid("salary structure has no earning components".into()));
        }

        // Overtime stretches over the period, each DAY priced on the same monthly base (statutory
        // 173 divisor) — the 1.5× first hour resets daily, so the days are priced separately and
        // summed — landing as an ordinary earning line so it flows through the balanced journal.
        let stretches = self
            .overtime_inputs
            .overtime_stretches(r.employee_id, period_start, period_end)
            .await?;
        let overtime_hours: Decimal = stretches.iter().map(|(_, h)| *h).sum();
        let salary_expense = run.salary_expense_account_id
            .ok_or(PayrollError::Invalid("run has no salary expense account".into()))?;
        let mut statutory: Vec<StatutoryLine> = Vec::new();
        if overtime_hours > Decimal::ZERO {
            let mut pay = Decimal::ZERO;
            for (_, day_hours) in &stretches {
                pay += statutory_calcs::overtime_pay(*day_hours, gross_monthly, &cfg.overtime)?;
            }
            statutory.push(StatutoryLine {
                name: "Lembur/Overtime".into(),
                component_type: "earning".into(),
                amount: pay,
                gl_account_id: salary_expense,
            });
        }

        // Dispatch: the employee's TER category when set, else the progressive-bracket path.
        let ptkp: PtkpTier = inputs
            .ptkp
            .parse()
            .map_err(|_| PayrollError::Invalid(format!("unknown ptkp tier '{}'", inputs.ptkp)))?;
        let method = match inputs.ter_category.as_deref() {
            None => Pph21Method::NpwpBrackets,
            Some(s) => Pph21Method::Ter(
                s.parse()
                    .map_err(|_| PayrollError::Invalid(format!("unknown ter category '{s}'")))?,
            ),
        };
        // THR tenure: whole months from join to the pay period; unknown join date → 0 (no THR).
        let tenure_months = Decimal::from(
            inputs
                .join_date
                .map(|j| (run.period_year - j.year()) * 12 + (month as i32 - j.month() as i32))
                .unwrap_or(0),
        );

        let components = statutory_calcs::compute_statutory(
            method,
            ptkp,
            inputs.has_npwp,
            gross_monthly,
            risk_class,
            tenure_months,
            &cfg,
        )?;
        for c in components {
            let gl = if c.component_type == "earning" {
                salary_expense // THR earning — the journal debits salary expense for the whole gross
            } else {
                match c.name.as_str() {
                    "PPh 21" => r.accounts.pph21_payable,
                    "BPJS Kesehatan" => r.accounts.bpjs_kesehatan_payable,
                    "BPJS Ketenagakerjaan" => r.accounts.bpjs_ketenagakerjaan_payable,
                    other => return Err(PayrollError::Invalid(format!("unroutable statutory component '{other}'"))),
                }
            };
            statutory.push(StatutoryLine {
                name: c.name,
                component_type: c.component_type,
                amount: c.amount,
                gl_account_id: gl,
            });
        }

        self.add_salary_slip(
            r.run_id,
            NewSalarySlip {
                employee_id: r.employee_id,
                structure_id: r.structure_id,
                working_days: r.working_days,
                unpaid_days: r.unpaid_days,
                statutory,
                overtime_hours,
                tax_method: Some(method.label().to_string()),
            },
        )
        .await
    }

    /// Roll the run's slips up into its totals and move `draft → processed` (ready to post).
    pub async fn process_payroll_entry(&self, run_id: Uuid) -> Result<(), PayrollError> {
        // Tenancy (ADR-0029), ID-only pattern: the run id alone identifies the work, so the reads
        // and the transition ride the ambient org request scope — under HTTP the request-dedicated
        // connection carries it; an undecorated deployment is unfenced by design.
        let totals = self.slips.sum_totals_by_run(&self.pool, run_id).await?;
        if totals.count == 0 {
            return Err(PayrollError::Invalid("a run needs at least one salary slip".into()));
        }
        let (g, d, n) = (totals.total_gross, totals.total_deductions, totals.total_net);
        let moved = self.entries.mark_processed(&self.pool, run_id, g, d, n).await?;
        if moved != 1 {
            return Err(PayrollError::InvalidState("run is not draft"));
        }
        Ok(())
    }

    /// Post the processed run to the GL — the 8th producer. Builds ONE balanced posting
    /// (`Dr Salary Expense (gross) · Cr Salary Payable (net) · Cr Σ deduction-account`), drives the
    /// `GlPostSink` (idempotent per run), then transition-gates `processed → posted` with the journal.
    /// Posts **at most once**. Emits `PayrollPosted`.
    pub async fn post_payroll_entry(
        &self,
        run_id: Uuid,
        posting_date: chrono::NaiveDate,
        sink: &dyn GlPostSink,
        events: &dyn PayrollEventSink,
    ) -> Result<PostOutcome, PayrollError> {
        // Tenancy (ADR-0029), ID-only pattern: identified by the run id alone. The reads ride the
        // ambient org request scope — under HTTP the request-dedicated connection carries it; an
        // undecorated deployment is unfenced by design.
        let run = self.entries.find_for_posting(&self.pool, run_id).await?
            .ok_or(PayrollError::NotFound("payroll run"))?;
        let status = run.status.as_str();
        let total_net = run.total_net;
        if status == "posted" {
            let j: Uuid = run.journal_id.ok_or(PayrollError::InvalidState("posted without a journal"))?;
            let p: Uuid = run.accounting_post_id.unwrap_or(j);
            // At-least-once delivery: a retried post re-publishes (the first attempt surfaced a
            // publish failure as an error even though its row landed). Consumers dedup by record
            // id, so a re-stage after a partial delivery is absorbed, never duplicated downstream.
            let payables = self.payables_for_run(run_id).await?;
            events
                .publish(&PayrollEvent::PayrollPosted(PayrollPosted {
                    payroll_entry_id: run_id,
                    company_id: legacy_company_echo(),
                    journal_id: j,
                    post_id: p,
                    total_gross: run.total_gross,
                    total_deductions: run.total_deductions,
                    total_net,
                    salary_payable_account_id: run.salary_payable_account_id
                        .ok_or(PayrollError::InvalidState("posted without a salary payable account"))?,
                    payables,
                }))
                .await
                .map_err(|e| PayrollError::EventPublish(e.to_string()))?;
            return Ok(PostOutcome { payroll_entry_id: run_id, journal_id: j, post_id: p, total_net, already: true });
        }
        if status != "processed" {
            return Err(PayrollError::InvalidState("run is not processed"));
        }
        let total_gross = run.total_gross;
        let total_deductions = run.total_deductions;
        let salary_expense: Uuid = run.salary_expense_account_id
            .ok_or(PayrollError::Invalid("run has no salary expense account".into()))?;
        let salary_payable: Uuid = run.salary_payable_account_id
            .ok_or(PayrollError::Invalid("run has no salary payable account".into()))?;

        // Deductions grouped by their payable account across every slip, carrying whether the account is
        // a statutory payable (routes the settlement consumer's remittance to the right authority).
        let ded_rows = self.slip_lines.group_deductions_by_account(&self.pool, run_id).await?;

        // Build the balanced posting: Dr Expense (gross) · Cr Payable (net) · Cr each deduction account.
        // The same grouping becomes the payable breakdown on PayrollPosted (settlement's input).
        let mut lines = vec![
            GlPostLine::debit(salary_expense, total_gross).with_description("Salary expense"),
            GlPostLine::credit(salary_payable, total_net).with_description("Net pay payable"),
        ];
        let mut payables: Vec<PayrollPayable> = Vec::new();
        for r in &ded_rows {
            let acct = r.gl_account_id;
            let amt = r.amount;
            if amt > Decimal::ZERO {
                lines.push(GlPostLine::credit(acct, amt).with_description("Payroll deduction payable"));
                payables.push(PayrollPayable { gl_account_id: acct, amount: amt, statutory: r.statutory });
            }
        }
        let env = AccountingPostEnvelope {
            idempotency_key: format!("payroll:{run_id}"),
            company_id: legacy_company_echo(),
            branch_id: None, source_type: "payroll".into(), source_id: run_id,
            source_reference: None, posting_date, currency: "IDR".into(), posting_type: "original".into(),
            description: Some("Payroll run".into()), lines,
        };
        if !env.is_balanced() {
            return Err(PayrollError::Unbalanced);
        }

        let ack = sink.post(&env).await.map_err(|r| PayrollError::GlRejected(r.code))?;

        let posted_at = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(
            posting_date
                .and_hms_opt(0, 0, 0)
                .ok_or(PayrollError::Invalid("posting date is not a real calendar date".into()))?,
            chrono::Utc,
        );
        let moved = self
            .entries
            .mark_posted(&self.pool, run_id, posted_at, ack.journal_id, ack.post_id)
            .await?;
        if moved != 1 {
            // Raced — the winner posted; return its journal.
            let j: Uuid = self.entries.fetch_journal_id(&self.pool, run_id).await?;
            return Ok(PostOutcome { payroll_entry_id: run_id, journal_id: j, post_id: ack.post_id, total_net, already: true });
        }
        events
            .publish(&PayrollEvent::PayrollPosted(PayrollPosted {
                payroll_entry_id: run_id, company_id: legacy_company_echo(), journal_id: ack.journal_id, post_id: ack.post_id,
                total_gross, total_deductions, total_net,
                salary_payable_account_id: salary_payable, payables,
            }))
            .await
            .map_err(|e| PayrollError::EventPublish(e.to_string()))?;
        Ok(PostOutcome { payroll_entry_id: run_id, journal_id: ack.journal_id, post_id: ack.post_id, total_net, already: false })
    }

    /// The run's deduction payables, grouped by account exactly as the post verb grouped them —
    /// the shared source for the already-posted re-publish and the remit verb, so both describe
    /// the SAME obligations the posted journal credited.
    async fn payables_for_run(&self, run_id: Uuid) -> Result<Vec<PayrollPayable>, PayrollError> {
        let ded_rows = self.slip_lines.group_deductions_by_account(&self.pool, run_id).await?;
        Ok(ded_rows
            .into_iter()
            .filter(|r| r.amount > Decimal::ZERO)
            .map(|r| PayrollPayable { gl_account_id: r.gl_account_id, amount: r.amount, statutory: r.statutory })
            .collect())
    }

    /// Remit a posted run's payables — one instruction per deduction account, each carrying the
    /// stable `payroll_remittance:{company}:{run}:{account}` idempotency key so retries dedup at
    /// the sink (the company segment is the legacy tenancy twin echo, ADR-0029 — stable per unit
    /// under a composing service, nil undecorated). Requires `posted` (an unposted run has no
    /// settled obligations to pay). Payee resolution is the composing host's adapter, never
    /// payroll's.
    pub async fn remit_payroll_entry(
        &self,
        run_id: Uuid,
        sink: &dyn RemittanceSink,
    ) -> Result<RemitOutcome, PayrollError> {
        // Tenancy (ADR-0029), ID-only pattern — see post_payroll_entry.
        let run = self.entries.find_for_posting(&self.pool, run_id).await?
            .ok_or(PayrollError::NotFound("payroll run"))?;
        if run.status.as_str() != "posted" {
            return Err(PayrollError::InvalidState("run is not posted"));
        }
        let payables = self.payables_for_run(run_id).await?;
        let mut remitted = Vec::with_capacity(payables.len());
        for p in payables {
            let instruction =
                RemittanceInstruction::new(legacy_company_echo(), run_id, p.gl_account_id, p.amount, p.statutory);
            let ack: RemitAck = sink.remit(&instruction).await?;
            remitted.push((instruction, ack));
        }
        Ok(RemitOutcome { payroll_entry_id: run_id, remitted })
    }
}
