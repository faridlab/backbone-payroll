//! As-of resolver for the statutory parameter tables.
//!
//! The authoritative source of statutory rates is the five effective-dated tables seeded by
//! migration (`pph21_brackets`, `pph21_ptkp`, `pph21_ter_rates`, `bpjs_params`, `overtime_params`).
//! This repository resolves each table AS OF a payroll period into one [`StatutoryConfig`] — the
//! same struct the pure calcs consume — so the calc layer never knows where its numbers came from.
//!
//! Resolution semantics: for each table, the `effective_from` that applies is the greatest one
//! `<= as_of`; every row at that `effective_from` is loaded together (a correction is a NEW
//! effective-dated row set, never an edit of live rows, so a set is always internally consistent).
//!
//! Fail-closed: any table with NO rows effective at `as_of` (and any incomplete BPJS/overtime
//! component set) is [`StatutoryError::NoParamsForPeriod`] — payroll refuses to compute rather
//! than silently zeroing a tax or an insurance contribution. There is deliberately NO runtime
//! fallback to YAML/`Default` config: those are seed-source material only; serving them at runtime
//! would let a deployment drift from the auditable parameter history.

use chrono::NaiveDate;
use rust_decimal::Decimal;
use sqlx::PgPool;
use std::collections::HashMap;

use crate::application::service::statutory_calcs::{
    BpjsConfig, BpjsKesehatanConfig, BpjsTkConfig, OvertimeBand, OvertimeConfig, Pph21Bracket,
    Pph21Config, StatutoryConfig, StatutoryError, TerRateBand,
};

/// Reads the statutory parameter tables. Thin struct (pool holder) for symmetry with the module's
/// other repositories; the reads are global-master lookups, so there is no company scoping here —
/// the tables are unfenced national-law data (no `company_id`, no RLS) by design.
pub struct StatutoryParamsRepository {
    pool: PgPool,
}

impl StatutoryParamsRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Resolve every statutory parameter table as-of `as_of` into one config. See the module docs
    /// for the resolution and fail-closed semantics.
    pub async fn resolve_as_of(
        &self,
        country_code: &str,
        as_of: NaiveDate,
    ) -> Result<StatutoryConfig, StatutoryError> {
        let brackets = self.brackets_as_of(country_code, as_of).await?;
        let ptkp_map = self.ptkp_as_of(country_code, as_of).await?;
        let ter = self.ter_as_of(country_code, as_of).await?;
        let bpjs = self.bpjs_as_of(country_code, as_of).await?;
        let overtime = self.overtime_as_of(country_code, as_of).await?;

        Ok(StatutoryConfig {
            pph21: Pph21Config {
                brackets,
                ptkp_map,
                // The no-NPWP surtax is a fixed statutory multiplier, not effective-dated table
                // data; the Default (1.2) is the law's constant.
                npwp_surtax_multiplier: Decimal::new(12, 1),
                ter,
            },
            bpjs,
            overtime,
        })
    }

    /// The `effective_from` that applies at `as_of` for `table`, or `None` when the table has no
    /// effective rows (each caller turns that into the fail-closed error).
    async fn effective_at(
        &self,
        table: &str,
        country_code: &str,
        as_of: NaiveDate,
    ) -> Result<Option<NaiveDate>, sqlx::Error> {
        let effective: Option<(Option<NaiveDate>,)> = sqlx::query_as(&format!(
            "SELECT MAX(effective_from) FROM {table} WHERE country_code = $1 AND effective_from <= $2"
        ))
        .bind(country_code)
        .bind(as_of)
        .fetch_optional(&self.pool)
        .await?;
        Ok(effective.and_then(|(d,)| d))
    }

    async fn brackets_as_of(
        &self,
        country_code: &str,
        as_of: NaiveDate,
    ) -> Result<Vec<Pph21Bracket>, StatutoryError> {
        let effective = self
            .effective_at("payroll.pph21_brackets", country_code, as_of)
            .await?
            .ok_or_else(|| no_params(country_code, as_of))?;
        let rows: Vec<(i32, Decimal, Option<Decimal>, Decimal)> = sqlx::query_as(
            r#"SELECT seq, lower_bound, upper_bound, rate
                 FROM payroll.pph21_brackets
                WHERE country_code = $1 AND effective_from = $2
                ORDER BY seq"#,
        )
        .bind(country_code)
        .bind(effective)
        .fetch_all(&self.pool)
        .await?;
        let brackets: Vec<Pph21Bracket> = rows
            .into_iter()
            .map(|(_, lower, upper, rate)| Pph21Bracket { lower_bound: lower, upper_bound: upper, rate })
            .collect();
        // A complete bracket set opens at income zero and closes open-ended: the progressive-tax
        // walk silently zero-taxes everything below a set that starts above zero, so a lone
        // correction row (an incomplete set) must refuse here rather than compute.
        if brackets.is_empty()
            || brackets.first().map(|b| b.lower_bound) != Some(Decimal::ZERO)
            || brackets.last().and_then(|b| b.upper_bound).is_some()
        {
            return Err(no_params(country_code, as_of));
        }
        Ok(brackets)
    }

    async fn ptkp_as_of(
        &self,
        country_code: &str,
        as_of: NaiveDate,
    ) -> Result<HashMap<String, Decimal>, StatutoryError> {
        let effective = self
            .effective_at("payroll.pph21_ptkp", country_code, as_of)
            .await?
            .ok_or_else(|| no_params(country_code, as_of))?;
        let rows: Vec<(String, Decimal)> = sqlx::query_as(
            r#"SELECT tier, annual_amount
                 FROM payroll.pph21_ptkp
                WHERE country_code = $1 AND effective_from = $2"#,
        )
        .bind(country_code)
        .bind(effective)
        .fetch_all(&self.pool)
        .await?;
        let ptkp_map: HashMap<String, Decimal> = rows.into_iter().collect();
        // The tier axis is closed (eight tiers); a set missing any of them is an incomplete
        // correction — the employee lookup would 500 on the first affected worker otherwise.
        for tier in ["tk0", "tk1", "tk2", "tk3", "k0", "k1", "k2", "k3"] {
            if !ptkp_map.contains_key(tier) {
                return Err(no_params(country_code, as_of));
            }
        }
        Ok(ptkp_map)
    }

    async fn ter_as_of(
        &self,
        country_code: &str,
        as_of: NaiveDate,
    ) -> Result<HashMap<String, Vec<TerRateBand>>, StatutoryError> {
        let effective = self
            .effective_at("payroll.pph21_ter_rates", country_code, as_of)
            .await?
            .ok_or_else(|| no_params(country_code, as_of))?;
        let rows: Vec<(String, i32, Decimal, Decimal)> = sqlx::query_as(
            r#"SELECT category, seq, lower_bound, rate
                 FROM payroll.pph21_ter_rates
                WHERE country_code = $1 AND effective_from = $2
                ORDER BY category, seq"#,
        )
        .bind(country_code)
        .bind(effective)
        .fetch_all(&self.pool)
        .await?;
        let mut map: HashMap<String, Vec<TerRateBand>> = HashMap::new();
        for (category, _, lower, rate) in rows {
            map.entry(category).or_default().push(TerRateBand { lower_bound: lower, rate });
        }
        // A complete TER set carries every category, each opening at base zero — a lone category
        // or a band list that starts above zero is an incomplete correction and must refuse.
        for category in ["ter_a", "ter_b", "ter_c"] {
            match map.get(category) {
                Some(bands)
                    if !bands.is_empty() && bands.first().map(|b| b.lower_bound) == Some(Decimal::ZERO) =>
                {}
                _ => return Err(no_params(country_code, as_of)),
            }
        }
        Ok(map)
    }

    async fn bpjs_as_of(&self, country_code: &str, as_of: NaiveDate) -> Result<BpjsConfig, StatutoryError> {
        let effective = self
            .effective_at("payroll.bpjs_params", country_code, as_of)
            .await?
            .ok_or_else(|| no_params(country_code, as_of))?;
        let rows: Vec<(String, String, Decimal, Option<Decimal>)> = sqlx::query_as(
            r#"SELECT component, side, rate, wage_cap
                 FROM payroll.bpjs_params
                WHERE country_code = $1 AND effective_from = $2"#,
        )
        .bind(country_code)
        .bind(effective)
        .fetch_all(&self.pool)
        .await?;

        let mut kes_employee = None;
        let mut kes_employer = None;
        let mut kes_cap: Option<Option<Decimal>> = None;
        let mut jht_employee = None;
        let mut jht_employer = None;
        let mut jp_employee = None;
        let mut jp_employer = None;
        let mut jp_cap: Option<Option<Decimal>> = None;
        let mut jkk: HashMap<String, Decimal> = HashMap::new();
        let mut jkm = None;

        for (component, side, rate, cap) in rows {
            match (component.as_str(), side.as_str()) {
                ("kes", "employee") => kes_employee = Some(rate),
                ("kes", "employer") => kes_employer = Some(rate),
                ("jht", "employee") => jht_employee = Some(rate),
                ("jht", "employer") => jht_employer = Some(rate),
                ("jp", "employee") => jp_employee = Some(rate),
                ("jp", "employer") => jp_employer = Some(rate),
                (jk, "employer") if jk.starts_with("jkk_") => {
                    jkk.insert(jk.trim_start_matches("jkk_").to_string(), rate);
                }
                ("jkm", "employer") => jkm = Some(rate),
                // An unknown component/side pair means the table carries data this build cannot
                // interpret — refuse instead of half-applying it.
                _ => return Err(no_params(country_code, as_of)),
            }
            // Kesehatan/Jaminan Pensiun are capped components by law; a NULL cap row means the
            // seeded set is incomplete — the `flatten().ok_or` below refuses rather than inventing
            // an unbounded cap.
            if component == "kes" {
                kes_cap = Some(cap);
            }
            if component == "jp" {
                jp_cap = Some(cap);
            }
        }

        Ok(BpjsConfig {
            kesehatan: BpjsKesehatanConfig {
                employee_rate: kes_employee.ok_or_else(|| no_params(country_code, as_of))?,
                employer_rate: kes_employer.ok_or_else(|| no_params(country_code, as_of))?,
                salary_cap: kes_cap.flatten().ok_or_else(|| no_params(country_code, as_of))?,
            },
            ketenagakerjaan: BpjsTkConfig {
                jht_employee_rate: jht_employee.ok_or_else(|| no_params(country_code, as_of))?,
                jht_employer_rate: jht_employer.ok_or_else(|| no_params(country_code, as_of))?,
                jp_employee_rate: jp_employee.ok_or_else(|| no_params(country_code, as_of))?,
                jp_employer_rate: jp_employer.ok_or_else(|| no_params(country_code, as_of))?,
                jp_salary_cap: jp_cap.flatten().ok_or_else(|| no_params(country_code, as_of))?,
                jkk_rates_by_risk_class: jkk,
                jkm_rate: jkm.ok_or_else(|| no_params(country_code, as_of))?,
            },
        })
    }

    async fn overtime_as_of(
        &self,
        country_code: &str,
        as_of: NaiveDate,
    ) -> Result<OvertimeConfig, StatutoryError> {
        let effective = self
            .effective_at("payroll.overtime_params", country_code, as_of)
            .await?
            .ok_or_else(|| no_params(country_code, as_of))?;
        let rows: Vec<(String, i32, Option<i32>, Decimal)> = sqlx::query_as(
            r#"SELECT day_kind, hour_from, hour_to, multiplier
                 FROM payroll.overtime_params
                WHERE country_code = $1 AND effective_from = $2
                ORDER BY day_kind, hour_from"#,
        )
        .bind(country_code)
        .bind(effective)
        .fetch_all(&self.pool)
        .await?;

        let mut workday = Vec::new();
        let mut rest_day = Vec::new();
        for (day_kind, hour_from, hour_to, multiplier) in rows {
            let band = OvertimeBand { hour_from, hour_to, multiplier };
            match day_kind.as_str() {
                "workday" => workday.push(band),
                "rest_day" => rest_day.push(band),
                _ => return Err(no_params(country_code, as_of)),
            }
        }
        // The pay calc dispatches the workday schedule; an effective set with no workday bands
        // cannot price ANY overtime — refuse.
        if workday.is_empty() {
            return Err(no_params(country_code, as_of));
        }

        Ok(OvertimeConfig {
            // The monthly-hours divisor is a fixed statutory constant, not effective-dated table
            // data; 173 is the law's value.
            hours_per_month: Decimal::new(173, 0),
            workday,
            rest_day,
        })
    }
}

fn no_params(country_code: &str, as_of: NaiveDate) -> StatutoryError {
    StatutoryError::NoParamsForPeriod(country_code.to_string(), as_of.to_string())
}
