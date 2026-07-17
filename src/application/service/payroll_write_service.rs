//! The hand-authored payroll write path (user-owned; survives regen).
//!
//! A salary run: assemble per-employee slips (earnings from a structure, prorated for HR unpaid days,
//! minus fixed + supplied statutory deductions), roll up the run totals, and post ONE balanced salary
//! journal to the GL — the **8th GL producer**: `Dr Salary Expense (gross) · Cr Salary Payable (net) ·
//! Cr statutory/other payables (grouped by account)`. Because `gross = net + Σ deductions`, it balances.
//! Idempotent per run (source_id = run id). Reads the HR employee via `period_summary`-style inputs;
//! the Indonesia statutory amounts (BPJS, PPh 21) are supplied by the deferred overlay. Money is IDR,
//! 2dp, half-away-from-zero.

use backbone_orm::company_scope;
use rust_decimal::{Decimal, RoundingStrategy};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use super::payroll_events::*;
use super::payroll_gl::*;

fn money(v: Decimal) -> Decimal {
    v.round_dp_with_strategy(2, RoundingStrategy::MidpointAwayFromZero)
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
}

pub struct NewComponent {
    pub name: String,
    pub component_type: String, // earning | deduction
    pub amount: Decimal,
    pub gl_account_id: Uuid,
}
pub struct NewStructure {
    pub company_id: Uuid,
    pub name: String,
    pub components: Vec<NewComponent>,
}

pub struct NewPayrollEntry {
    pub company_id: Uuid,
    pub period_year: i32,
    pub period_month: i32,
    pub salary_expense_account_id: Uuid,
    pub salary_payable_account_id: Uuid,
}

/// A supplied Indonesia statutory deduction (BPJS Kesehatan/Ketenagakerjaan, PPh 21) — computed by the
/// deferred overlay, supplied here like billing's tax lines.
pub struct StatutoryLine {
    pub name: String,
    pub amount: Decimal,
    pub gl_account_id: Uuid, // the payable account (BPJS Payable, PPh 21 Payable)
}
pub struct NewSalarySlip {
    pub employee_id: Uuid,
    pub structure_id: Uuid,
    /// Working days in the period (e.g. 22); earnings are prorated by (working − unpaid)/working.
    pub working_days: Decimal,
    /// Unpaid-leave + uncovered-absence days from `hr.period_summary` — reduce gross.
    pub unpaid_days: Decimal,
    pub statutory: Vec<StatutoryLine>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PostOutcome {
    pub payroll_entry_id: Uuid,
    pub journal_id: Uuid,
    pub post_id: Uuid,
    pub total_net: Decimal,
    pub already: bool,
}

pub struct PayrollWriteService {
    pool: PgPool,
}

impl PayrollWriteService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
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
        // RLS scope (ADR-0008): company is on the DTO — bind it onto our own transaction so the
        // structure + component inserts pass the `app.company_id` WITH CHECK fence.
        let mut tx = self.pool.begin().await?;
        company_scope::bind_company_on(&mut tx, s.company_id).await?;
        sqlx::query(
            r#"INSERT INTO payroll.salary_structures (id, company_id, name, is_active)
               VALUES ($1,$2,$3,true)"#,
        )
        .bind(id).bind(s.company_id).bind(&s.name)
        .execute(&mut *tx)
        .await?;
        for c in &s.components {
            if c.amount < Decimal::ZERO {
                return Err(PayrollError::Invalid("component amount must be non-negative".into()));
            }
            sqlx::query(
                r#"INSERT INTO payroll.salary_components
                     (id, structure_id, name, component_type, amount, gl_account_id)
                   VALUES ($1,$2,$3,$4::component_type,$5,$6)"#,
            )
            .bind(Uuid::new_v4()).bind(id).bind(&c.name).bind(&c.component_type).bind(money(c.amount)).bind(c.gl_account_id)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(id)
    }

    /// Open a payroll run for a company/period (draft). Unique per (company, year, month).
    pub async fn create_payroll_entry(&self, e: NewPayrollEntry) -> Result<Uuid, PayrollError> {
        if !(1..=12).contains(&e.period_month) {
            return Err(PayrollError::Invalid("period_month must be 1..12".into()));
        }
        let id = Uuid::new_v4();
        // RLS scope (ADR-0008): company is on the DTO — scope the insert so it passes the WITH CHECK fence.
        let insert_q = sqlx::query(
            r#"INSERT INTO payroll.payroll_entries
                 (id, company_id, period_year, period_month, status, salary_expense_account_id,
                  salary_payable_account_id, total_gross, total_deductions, total_net)
               VALUES ($1,$2,$3,$4,'draft'::payroll_status,$5,$6,0,0,0)"#,
        )
        .bind(id).bind(e.company_id).bind(e.period_year).bind(e.period_month)
        .bind(e.salary_expense_account_id).bind(e.salary_payable_account_id);
        let r = company_scope::with_company_scope(
            Some(e.company_id),
            company_scope::execute_scoped(&self.pool, insert_q),
        )
        .await;
        match r {
            Ok(_) => Ok(id),
            Err(err) if err.as_database_error().map(|d| d.is_unique_violation()).unwrap_or(false) =>
                Err(PayrollError::Invalid("a payroll run already exists for this company/period".into())),
            Err(err) => Err(err.into()),
        }
    }

    /// Add an employee's slip to a DRAFT run. Earnings come from the structure, prorated by unpaid days
    /// (`gross = Σ earning · (working − unpaid)/working`); fixed + supplied statutory deductions subtract.
    /// `net = gross − deductions` and must be non-negative.
    pub async fn add_salary_slip(&self, run_id: Uuid, s: NewSalarySlip) -> Result<Uuid, PayrollError> {
        // RLS scope (ADR-0008), ID-only pattern: identified by the run id alone — no company argument to
        // scope from up front. The lookup rides the request-dedicated connection (which carries the
        // caller's `app.company_id`), so another company's run simply isn't found. Having read the run,
        // we bind its company onto our own transaction below.
        let run = company_scope::fetch_optional_row_scoped(
            &self.pool,
            sqlx::query(
                r#"SELECT company_id, status::text AS status FROM payroll.payroll_entries
                   WHERE id=$1 AND (metadata->>'deleted_at') IS NULL"#,
            )
            .bind(run_id),
        )
        .await?
        .ok_or(PayrollError::NotFound("payroll run"))?;
        if run.get::<String, _>("status") != "draft" {
            return Err(PayrollError::InvalidState("run is not draft"));
        }
        let company_id: Uuid = run.get("company_id");
        if s.working_days <= Decimal::ZERO {
            return Err(PayrollError::Invalid("working_days must be positive".into()));
        }
        // Clamp unpaid days to [0, working_days] so the proration factor stays in [0, 1]. Without the
        // LOWER clamp a negative unpaid_days (a bad upstream hr.period_summary value) drives factor > 1
        // and inflates gross ABOVE the structure — a balanced-but-over-booked salary journal (maturity
        // council 2026-07-08). The DB CHECKs in 20260708000100_payroll_balance_guards backstop any writer.
        // Clamp unpaid days to [0, working_days] so the proration factor stays in [0, 1]. Without the
        // LOWER clamp a negative unpaid_days (a bad upstream hr.period_summary value) drives factor > 1
        // and inflates gross ABOVE the structure — a balanced-but-over-booked salary journal (maturity
        // council 2026-07-08). The DB CHECKs in 20260708000100_payroll_balance_guards backstop any writer.
        let unpaid = s.unpaid_days.clamp(Decimal::ZERO, s.working_days);
        let factor = (s.working_days - unpaid) / s.working_days; // proration for unpaid days

        // Load the structure components.
        let comps = company_scope::with_company_scope(
            Some(company_id),
            company_scope::fetch_all_rows_scoped(
                &self.pool,
                sqlx::query(
                    "SELECT name, component_type::text AS ct, amount, gl_account_id FROM payroll.salary_components WHERE structure_id=$1")
                    .bind(s.structure_id),
            ),
        )
        .await?;
        if comps.is_empty() {
            return Err(PayrollError::Invalid("salary structure has no components".into()));
        }

        struct Line { name: String, ct: String, is_statutory: bool, amount: Decimal, account: Uuid }
        let mut lines: Vec<Line> = Vec::new();
        let (mut gross, mut deductions) = (Decimal::ZERO, Decimal::ZERO);
        for c in &comps {
            let ct: String = c.get("ct");
            let base: Decimal = c.get("amount");
            let account: Uuid = c.get("gl_account_id");
            if ct == "earning" {
                let amt = money(base * factor);
                gross += amt;
                lines.push(Line { name: c.get("name"), ct, is_statutory: false, amount: amt, account });
            } else {
                deductions += base;
                lines.push(Line { name: c.get("name"), ct, is_statutory: false, amount: base, account });
            }
        }
        for st in &s.statutory {
            if st.amount < Decimal::ZERO {
                return Err(PayrollError::Invalid("statutory amount must be non-negative".into()));
            }
            let amt = money(st.amount);
            deductions += amt;
            lines.push(Line { name: st.name.clone(), ct: "deduction".into(), is_statutory: true, amount: amt, account: st.gl_account_id });
        }
        let net = gross - deductions;
        if net < Decimal::ZERO {
            return Err(PayrollError::Invalid("deductions exceed gross — net pay would be negative".into()));
        }

        let slip_id = Uuid::new_v4();
        let mut tx = self.pool.begin().await?;
        // The run's own company, read above — bind it so the slip + line inserts pass the WITH CHECK fence.
        company_scope::bind_company_on(&mut tx, company_id).await?;
        let ins = sqlx::query(
            r#"INSERT INTO payroll.salary_slips
                 (id, payroll_entry_id, company_id, employee_id, structure_id, working_days, unpaid_days,
                  gross_pay, total_deductions, net_pay)
               VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)"#,
        )
        .bind(slip_id).bind(run_id).bind(company_id).bind(s.employee_id).bind(s.structure_id)
        .bind(s.working_days).bind(unpaid).bind(gross).bind(deductions).bind(net)
        .execute(&mut *tx)
        .await;
        if let Err(err) = ins {
            return Err(if err.as_database_error().map(|d| d.is_unique_violation()).unwrap_or(false) {
                PayrollError::Invalid("this employee already has a slip in this run".into())
            } else { err.into() });
        }
        for l in &lines {
            sqlx::query(
                r#"INSERT INTO payroll.salary_slip_lines
                     (id, salary_slip_id, name, component_type, is_statutory, amount, gl_account_id)
                   VALUES ($1,$2,$3,$4::component_type,$5,$6,$7)"#,
            )
            .bind(Uuid::new_v4()).bind(slip_id).bind(&l.name).bind(&l.ct).bind(l.is_statutory).bind(l.amount).bind(l.account)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(slip_id)
    }

    /// Roll the run's slips up into its totals and move `draft → processed` (ready to post).
    pub async fn process_payroll_entry(&self, run_id: Uuid) -> Result<(), PayrollError> {
        // RLS scope (ADR-0008), ID-only pattern: the run id alone identifies the work, so the reads and
        // the transition ride the request-dedicated connection's `app.company_id`. An event-driven caller
        // must wrap this in `with_company_scope(Some(event.company_id))` or the reads fail closed.
        let totals = company_scope::fetch_one_row_scoped(
            &self.pool,
            sqlx::query(
                r#"SELECT COALESCE(SUM(gross_pay),0) AS g, COALESCE(SUM(total_deductions),0) AS d,
                          COALESCE(SUM(net_pay),0) AS n, count(*) AS c
                   FROM payroll.salary_slips WHERE payroll_entry_id=$1 AND (metadata->>'deleted_at') IS NULL"#,
            )
            .bind(run_id),
        )
        .await?;
        if totals.get::<i64, _>("c") == 0 {
            return Err(PayrollError::Invalid("a run needs at least one salary slip".into()));
        }
        let (g, d, n): (Decimal, Decimal, Decimal) = (totals.get("g"), totals.get("d"), totals.get("n"));
        let moved = company_scope::execute_scoped(
            &self.pool,
            sqlx::query(
                r#"UPDATE payroll.payroll_entries
                   SET status='processed'::payroll_status, total_gross=$2, total_deductions=$3, total_net=$4
                   WHERE id=$1 AND status='draft'::payroll_status"#,
            )
            .bind(run_id).bind(g).bind(d).bind(n),
        )
        .await?;
        if moved.rows_affected() != 1 {
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
        // RLS scope (ADR-0008), ID-only pattern: identified by the run id alone. Under HTTP the
        // request-dedicated connection carries the scope. Driven by an EVENT, the caller must wrap this
        // in `with_company_scope(Some(event.company_id))` — otherwise these reads fail closed.
        let run = company_scope::fetch_optional_row_scoped(
            &self.pool,
            sqlx::query(
                r#"SELECT company_id, status::text AS status, salary_expense_account_id, salary_payable_account_id,
                          total_gross, total_deductions, total_net, journal_id, accounting_post_id
                   FROM payroll.payroll_entries WHERE id=$1 AND (metadata->>'deleted_at') IS NULL"#,
            )
            .bind(run_id),
        )
        .await?
        .ok_or(PayrollError::NotFound("payroll run"))?;
        let status: String = run.get("status");
        let total_net: Decimal = run.get("total_net");
        if status == "posted" {
            let j: Uuid = run.get::<Option<Uuid>, _>("journal_id").ok_or(PayrollError::InvalidState("posted without a journal"))?;
            let p: Uuid = run.get::<Option<Uuid>, _>("accounting_post_id").unwrap_or(j);
            return Ok(PostOutcome { payroll_entry_id: run_id, journal_id: j, post_id: p, total_net, already: true });
        }
        if status != "processed" {
            return Err(PayrollError::InvalidState("run is not processed"));
        }
        let company_id: Uuid = run.get("company_id");
        let total_gross: Decimal = run.get("total_gross");
        let total_deductions: Decimal = run.get("total_deductions");
        let salary_expense: Uuid = run.get::<Option<Uuid>, _>("salary_expense_account_id")
            .ok_or(PayrollError::Invalid("run has no salary expense account".into()))?;
        let salary_payable: Uuid = run.get::<Option<Uuid>, _>("salary_payable_account_id")
            .ok_or(PayrollError::Invalid("run has no salary payable account".into()))?;

        // Deductions grouped by their payable account across every slip, carrying whether the account is
        // a statutory payable (routes the settlement consumer's remittance to the right authority).
        let ded_rows = company_scope::with_company_scope(
            Some(company_id),
            company_scope::fetch_all_rows_scoped(
                &self.pool,
                sqlx::query(
                    r#"SELECT l.gl_account_id, SUM(l.amount) AS amt, bool_or(l.is_statutory) AS statutory
                       FROM payroll.salary_slip_lines l JOIN payroll.salary_slips s ON s.id = l.salary_slip_id
                       WHERE s.payroll_entry_id=$1 AND l.component_type='deduction'::component_type
                         AND (s.metadata->>'deleted_at') IS NULL
                       GROUP BY l.gl_account_id"#,
                )
                .bind(run_id),
            ),
        )
        .await?;

        // Build the balanced posting: Dr Expense (gross) · Cr Payable (net) · Cr each deduction account.
        // The same grouping becomes the payable breakdown on PayrollPosted (settlement's input).
        let mut lines = vec![
            GlPostLine::debit(salary_expense, total_gross).with_description("Salary expense"),
            GlPostLine::credit(salary_payable, total_net).with_description("Net pay payable"),
        ];
        let mut payables: Vec<PayrollPayable> = Vec::new();
        for r in &ded_rows {
            let acct: Uuid = r.get("gl_account_id");
            let amt: Decimal = r.get("amt");
            if amt > Decimal::ZERO {
                lines.push(GlPostLine::credit(acct, amt).with_description("Payroll deduction payable"));
                payables.push(PayrollPayable { gl_account_id: acct, amount: amt, statutory: r.get("statutory") });
            }
        }
        let env = AccountingPostEnvelope {
            idempotency_key: format!("payroll:{run_id}"),
            company_id, branch_id: None, source_type: "payroll".into(), source_id: run_id,
            source_reference: None, posting_date, currency: "IDR".into(), posting_type: "original".into(),
            description: Some("Payroll run".into()), lines,
        };
        if !env.is_balanced() {
            return Err(PayrollError::Unbalanced);
        }

        let ack = sink.post(&env).await.map_err(|r| PayrollError::GlRejected(r.code))?;

        let moved = company_scope::with_company_scope(
            Some(company_id),
            company_scope::execute_scoped(
                &self.pool,
                sqlx::query(
                    r#"UPDATE payroll.payroll_entries
                       SET status='posted'::payroll_status, posting_date=$2, journal_id=$3, accounting_post_id=$4
                       WHERE id=$1 AND status='processed'::payroll_status"#,
                )
                .bind(run_id).bind(chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(posting_date.and_hms_opt(0,0,0).unwrap(), chrono::Utc))
                .bind(ack.journal_id).bind(ack.post_id),
            ),
        )
        .await?;
        if moved.rows_affected() != 1 {
            // Raced — the winner posted; return its journal.
            let j: Uuid = company_scope::with_company_scope(
                Some(company_id),
                company_scope::fetch_one_scalar_scoped(
                    &self.pool,
                    sqlx::query_scalar("SELECT journal_id FROM payroll.payroll_entries WHERE id=$1")
                        .bind(run_id),
                ),
            )
            .await?;
            return Ok(PostOutcome { payroll_entry_id: run_id, journal_id: j, post_id: ack.post_id, total_net, already: true });
        }
        events.publish(&PayrollEvent::PayrollPosted(PayrollPosted {
            payroll_entry_id: run_id, company_id, journal_id: ack.journal_id, post_id: ack.post_id,
            total_gross, total_deductions, total_net,
            salary_payable_account_id: salary_payable, payables,
        }));
        Ok(PostOutcome { payroll_entry_id: run_id, journal_id: ack.journal_id, post_id: ack.post_id, total_net, already: false })
    }
}
