//! Indonesian statutory payroll calculations — pure, config-driven functions.
//!
//! These are the deductions/contributions that turn gross pay into Indonesian net pay:
//!   - **PPh 21** — progressive personal income tax (UU HPP brackets, PTKP relief, NPWP surtax).
//!   - **BPJS Kesehatan** — national health insurance (employee 1% / employer 4%, salary-capped).
//!   - **BPJS Ketenagakerjaan** — employment insurance (JHT + JP + JKK + JKM).
//!   - **THR** — holiday allowance, pro-rated by tenure.
//!
//! Design rules (read me):
//!   - **Pure math only.** No DB, no ports, no async. Every rate/bracket/PTKP value/cap lives in
//!     [`StatutoryConfig`] (loaded from `config/application.yml` or built via [`StatutoryConfig::default`],
//!     which bakes the current-law values). The calc bodies hardcode *the formula structure*, never the
//!     numeric rates — BPJS caps move yearly and PTKP/brackets move by law, so config is the single
//!     source of truth.
//!   - **No Cargo edge to `backbone-employee`.** The shipped library has zero normal dependency on the
//!     employee module (it is a dev-only path dep used by integration tests). [`PtkpTier`] is therefore
//!     mirrored locally here — same 8 variants, same `snake_case` serde, same lowercase `Display` /
//!     `FromStr` as `backbone_employee::domain::entity::PtkpTier`. The slip-assembly layer (which does
//!     hold the employee edge) maps one-to-one via the string round-trip
//!     `employee_tier.to_string().parse::<PtkpTier>()` — both speak `"tk0".."k3"`.
//!   - **Rounding.** Every monetary output is rounded to 2 dp (rupiah + sen) with
//!     `RoundingStrategy::HalfUp`, applied once at the end of each function so intermediate precision
//!     is preserved. IDR has no sen in cash practice, but payroll ledgers keep 2 dp for the deduction
//!     totals to remain reconcilable; the composing slip may `.round_dp(0)` if whole-rupiah posting is
//!     desired.
//!   - **Fallibility.** Lookups keyed by config (PTKP tier, JKK risk class) can miss if a config is
//!     malformed; those functions return `Result<_, StatutoryError>`. The Default config is always
//!     complete, so well-formed deployments never see the error variants.

use rust_decimal::{Decimal, RoundingStrategy};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Two-decimal rounding used for every statutory money output.
const MONEY_DP: u32 = 2;
/// Round a Decimal to ledger precision (2 dp, half-up).
fn money(d: Decimal) -> Decimal {
    d.round_dp_with_strategy(MONEY_DP, RoundingStrategy::MidpointAwayFromZero)
}

// ============================================================================
// PtkpTier — local mirror of backbone_employee::domain::entity::PtkpTier
// ============================================================================

/// Pengurang Tanggungan Pajak (PTKP) tier — personal income-tax relief category.
///
/// Mirrors `backbone_employee::domain::entity::PtkpTier` 1:1 (same variants, same wire encoding) so
/// the slip-assembly layer can convert with `employee_tier.to_string().parse::<PtkpTier>()`. Duplicated
/// deliberately: payroll's shipped library has no Cargo edge to the employee module, and a local enum
/// is both type-safe and self-documenting where a `&str` key would not be.
///
/// Variants: `Tk0..Tk3` (unmarried, 0–3 dependants) · `K0..K3` (married, 0–3 dependants).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PtkpTier {
    Tk0,
    Tk1,
    Tk2,
    Tk3,
    K0,
    K1,
    K2,
    K3,
}

impl PtkpTier {
    /// Stable lowercase key used to index [`Pph21Config::ptkp_map`] — matches the YAML map keys
    /// and `backbone_employee::PtkpTier`'s `Display` output.
    pub fn key(self) -> &'static str {
        match self {
            PtkpTier::Tk0 => "tk0",
            PtkpTier::Tk1 => "tk1",
            PtkpTier::Tk2 => "tk2",
            PtkpTier::Tk3 => "tk3",
            PtkpTier::K0 => "k0",
            PtkpTier::K1 => "k1",
            PtkpTier::K2 => "k2",
            PtkpTier::K3 => "k3",
        }
    }
}

impl std::fmt::Display for PtkpTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.key())
    }
}

impl std::str::FromStr for PtkpTier {
    type Err = StatutoryError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "tk0" => Ok(Self::Tk0),
            "tk1" => Ok(Self::Tk1),
            "tk2" => Ok(Self::Tk2),
            "tk3" => Ok(Self::Tk3),
            "k0" => Ok(Self::K0),
            "k1" => Ok(Self::K1),
            "k2" => Ok(Self::K2),
            "k3" => Ok(Self::K3),
            other => Err(StatutoryError::UnknownPtkpTier(other.to_string())),
        }
    }
}

impl Default for PtkpTier {
    fn default() -> Self {
        Self::Tk0
    }
}

// ============================================================================
// Errors
// ============================================================================

/// Errors raised by the statutory calculators. Only the config-keyed lookups (PTKP tier, JKK risk
/// class) and the YAML loader are fallible; the math itself is total.
#[derive(Debug, thiserror::Error)]
pub enum StatutoryError {
    /// The PTKP tier is not present in `pph21.ptkp_map` — the config is incomplete.
    #[error("unknown PTKP tier '{0}' — add it to pph21.ptkp_map in config/application.yml")]
    UnknownPtkpTier(String),
    /// The BPJS JKK risk class is not present in `bpjs.ketenagakerjaan.jkk_rates_by_risk_class`.
    #[error("unknown BPJS JKK risk class {0} — add it to bpjs.ketenagakerjaan.jkk_rates_by_risk_class")]
    UnknownRiskClass(u8),
    /// A statutory config YAML could not be parsed.
    #[error("invalid statutory config YAML: {0}")]
    Yaml(#[from] serde_yaml::Error),
    /// A config file could not be read.
    #[error("statutory config I/O error: {0}")]
    Io(#[from] std::io::Error),
}

// ============================================================================
// Config
// ============================================================================

/// Top-level statutory configuration — the `statutory:` block of `config/application.yml`.
///
/// Construct with [`StatutoryConfig::default`] (current-law values baked in) or load from YAML via
/// [`StatutoryConfig::from_yaml_str`] / [`StatutoryConfig::load_from_config_dir`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatutoryConfig {
    #[serde(default)]
    pub pph21: Pph21Config,
    #[serde(default)]
    pub bpjs: BpjsConfig,
}

/// PPh 21 configuration: progressive tax brackets + PTKP relief map + NPWP surtax multiplier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pph21Config {
    /// Progressive brackets, sorted by `lower_bound` ascending. Each bracket taxes the slice of
    /// taxable income **above** its `lower_bound` (exclusive) up to its `upper_bound` (inclusive) at
    /// `rate`. The final bracket has `upper_bound: null` (unbounded).
    pub brackets: Vec<Pph21Bracket>,
    /// Annual PTKP relief (IDR) keyed by lowercase tier label (`"tk0".."k3"`).
    #[serde(default)]
    pub ptkp_map: HashMap<String, Decimal>,
    /// Multiplier applied to computed tax when the taxpayer has no NPWP (default `1.2` = 20% surtax).
    #[serde(default = "default_npwp_surtax")]
    pub npwp_surtax_multiplier: Decimal,
}

/// One progressive-tax bracket. `upper_bound = None` means unbounded (the top bracket).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pph21Bracket {
    /// Inclusive lower bound of the bracket (IDR/yr). Income at exactly this bound contributes 0 to
    /// this bracket — it was already exhausted by the bracket below.
    pub lower_bound: Decimal,
    /// Inclusive upper bound (IDR/yr), or `None` for the unbounded top bracket.
    pub upper_bound: Option<Decimal>,
    /// Marginal rate for this bracket (e.g. `Decimal::new(5, 2)` == 5%).
    pub rate: Decimal,
}

/// BPJS configuration — Kesehatan (health) + Ketenagakerjaan (employment) insurance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BpjsConfig {
    #[serde(default)]
    pub kesehatan: BpjsKesehatanConfig,
    #[serde(default)]
    pub ketenagakerjaan: BpjsTkConfig,
}

/// BPJS Kesehatan — health insurance. Employee and employer rates are both applied to the salary
/// capped at `salary_cap`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BpjsKesehatanConfig {
    #[serde(default = "default_kesehatan_employee")]
    pub employee_rate: Decimal,
    #[serde(default = "default_kesehatan_employer")]
    pub employer_rate: Decimal,
    #[serde(default = "default_kesehatan_cap")]
    pub salary_cap: Decimal,
}

/// BPJS Ketenagakerjaan — employment insurance (JHT, JP, JKK, JKM).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BpjsTkConfig {
    /// JHT — employee share (uncapped).
    #[serde(default = "default_jht_employee")]
    pub jht_employee_rate: Decimal,
    /// JHT — employer share (uncapped).
    #[serde(default = "default_jht_employer")]
    pub jht_employer_rate: Decimal,
    /// JP — employee share, applied to salary capped at `jp_salary_cap`.
    #[serde(default = "default_jp_employee")]
    pub jp_employee_rate: Decimal,
    /// JP — employer share, applied to salary capped at `jp_salary_cap`.
    #[serde(default = "default_jp_employer")]
    pub jp_employer_rate: Decimal,
    /// JP salary cap (IDR/month).
    #[serde(default = "default_jp_cap")]
    pub jp_salary_cap: Decimal,
    /// JKK — employer-only rate keyed by risk class (1–5) as a **string** key
    /// (`"1".."5"`). serde_yaml round-trips numeric YAML keys unreliably across versions, so string
    /// keys are used; the calc looks up `risk_class.to_string()`.
    #[serde(default)]
    pub jkk_rates_by_risk_class: HashMap<String, Decimal>,
    /// JKM — employer-only flat rate.
    #[serde(default = "default_jkm")]
    pub jkm_rate: Decimal,
}

fn default_npwp_surtax() -> Decimal {
    Decimal::new(12, 1) // 1.2
}
fn default_kesehatan_employee() -> Decimal {
    Decimal::new(1, 2) // 0.01
}
fn default_kesehatan_employer() -> Decimal {
    Decimal::new(4, 2) // 0.04
}
fn default_kesehatan_cap() -> Decimal {
    Decimal::new(12_000_000, 0)
}
fn default_jht_employee() -> Decimal {
    Decimal::new(2, 2) // 0.02
}
fn default_jht_employer() -> Decimal {
    Decimal::new(37, 3) // 0.037
}
fn default_jp_employee() -> Decimal {
    Decimal::new(1, 2) // 0.01
}
fn default_jp_employer() -> Decimal {
    Decimal::new(2, 2) // 0.02
}
fn default_jp_cap() -> Decimal {
    Decimal::new(10_547_400, 0)
}
fn default_jkm() -> Decimal {
    Decimal::new(3, 3) // 0.003
}

impl Default for StatutoryConfig {
    /// Current-law (UU HPP / BPJS 2024) statutory values. Kept in sync with the `statutory:` block
    /// of `config/application.yml`; when the law changes, update both (or just the YAML).
    fn default() -> Self {
        let ptkp_map = [
            ("tk0", Decimal::new(54_000_000, 0)),
            ("tk1", Decimal::new(58_500_000, 0)),
            ("tk2", Decimal::new(63_000_000, 0)),
            ("tk3", Decimal::new(67_500_000, 0)),
            ("k0", Decimal::new(58_500_000, 0)),
            ("k1", Decimal::new(63_000_000, 0)),
            ("k2", Decimal::new(67_500_000, 0)),
            ("k3", Decimal::new(72_000_000, 0)),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect();

        let brackets = vec![
            Pph21Bracket {
                lower_bound: Decimal::ZERO,
                upper_bound: Some(Decimal::new(60_000_000, 0)),
                rate: Decimal::new(5, 2), // 5%
            },
            Pph21Bracket {
                lower_bound: Decimal::new(60_000_000, 0),
                upper_bound: Some(Decimal::new(250_000_000, 0)),
                rate: Decimal::new(15, 2), // 15%
            },
            Pph21Bracket {
                lower_bound: Decimal::new(250_000_000, 0),
                upper_bound: Some(Decimal::new(500_000_000, 0)),
                rate: Decimal::new(25, 2), // 25%
            },
            Pph21Bracket {
                lower_bound: Decimal::new(500_000_000, 0),
                upper_bound: Some(Decimal::new(5_000_000_000, 0)),
                rate: Decimal::new(30, 2), // 30%
            },
            Pph21Bracket {
                lower_bound: Decimal::new(5_000_000_000, 0),
                upper_bound: None,
                rate: Decimal::new(35, 2), // 35%
            },
        ];

        let jkk_rates_by_risk_class = [
            (1u8, Decimal::new(24, 4)),  // 0.24%
            (2u8, Decimal::new(54, 4)),  // 0.54%
            (3u8, Decimal::new(89, 4)),  // 0.89%
            (4u8, Decimal::new(127, 4)), // 1.27%
            (5u8, Decimal::new(174, 4)), // 1.74%
        ]
        .into_iter()
        .map(|(c, r)| (c.to_string(), r))
        .collect();

        StatutoryConfig {
            pph21: Pph21Config {
                brackets,
                ptkp_map,
                npwp_surtax_multiplier: default_npwp_surtax(),
            },
            bpjs: BpjsConfig {
                kesehatan: BpjsKesehatanConfig {
                    employee_rate: default_kesehatan_employee(),
                    employer_rate: default_kesehatan_employer(),
                    salary_cap: default_kesehatan_cap(),
                },
                ketenagakerjaan: BpjsTkConfig {
                    jht_employee_rate: default_jht_employee(),
                    jht_employer_rate: default_jht_employer(),
                    jp_employee_rate: default_jp_employee(),
                    jp_employer_rate: default_jp_employer(),
                    jp_salary_cap: default_jp_cap(),
                    jkk_rates_by_risk_class,
                    jkm_rate: default_jkm(),
                },
            },
        }
    }
}

impl Default for Pph21Config {
    fn default() -> Self {
        StatutoryConfig::default().pph21
    }
}
impl Default for BpjsConfig {
    fn default() -> Self {
        StatutoryConfig::default().bpjs
    }
}
impl Default for BpjsKesehatanConfig {
    fn default() -> Self {
        StatutoryConfig::default().bpjs.kesehatan
    }
}
impl Default for BpjsTkConfig {
    fn default() -> Self {
        StatutoryConfig::default().bpjs.ketenagakerjaan
    }
}

impl Pph21Config {
    /// Annual PTKP relief for `tier`, or an error if the tier is absent from `ptkp_map`.
    pub fn ptkp_relief(&self, tier: PtkpTier) -> Result<Decimal, StatutoryError> {
        self.ptkp_map
            .get(tier.key())
            .copied()
            .ok_or_else(|| StatutoryError::UnknownPtkpTier(tier.key().to_string()))
    }
}

impl StatutoryConfig {
    /// Parse the `statutory:` block out of a full `application.yml` document string. If the block is
    /// absent, the current-law [`StatutoryConfig::default`] is returned so a payroll node that has not
    /// yet added the block still boots with correct statutory values.
    pub fn from_yaml_str(application_yml: &str) -> Result<Self, StatutoryError> {
        let root: serde_yaml::Value = serde_yaml::from_str(application_yml)?;
        match root.get("statutory") {
            Some(block) => Ok(serde_yaml::from_value(block.clone())?),
            None => Ok(Self::default()),
        }
    }

    /// Load from a config directory containing `application.yml` (+ optional
    /// `application-{env}.yml` override). The env file's **whole** `statutory:` block replaces the
    /// base block when present (shallow override — you tune rates by overriding the entire section).
    pub fn load_from_config_dir(dir: &Path, environment: &str) -> Result<Self, StatutoryError> {
        let base = std::fs::read_to_string(dir.join("application.yml"))?;
        let mut root: serde_yaml::Value = serde_yaml::from_str(&base)?;

        let env_path = dir.join(format!("application-{}.yml", environment));
        if env_path.exists() {
            let env_str = std::fs::read_to_string(&env_path)?;
            if let Ok(env_root) = serde_yaml::from_str::<serde_yaml::Value>(&env_str) {
                if let Some(env_stat) = env_root.get("statutory") {
                    // Replace the whole statutory subtree (shallow override by design).
                    match root.get_mut("statutory") {
                        Some(slot) => *slot = env_stat.clone(),
                        None => {
                            if let serde_yaml::Value::Mapping(ref mut m) = root {
                                m.insert(
                                    serde_yaml::Value::String("statutory".into()),
                                    env_stat.clone(),
                                );
                            }
                        }
                    }
                }
            }
        }

        match root.get("statutory") {
            Some(block) => Ok(serde_yaml::from_value(block.clone())?),
            None => Ok(Self::default()),
        }
    }
}

// ============================================================================
// Calculations
// ============================================================================

/// Apply a sorted-ascending progressive bracket schedule to `taxable`. Each bracket taxes the slice
/// strictly above its `lower_bound` up to (and including) its `upper_bound`. Internal helper.
fn progressive_tax(taxable: Decimal, brackets: &[Pph21Bracket]) -> Decimal {
    let mut tax = Decimal::ZERO;
    for b in brackets {
        // No income reaches this or any higher bracket once taxable <= this bracket's lower bound.
        if taxable <= b.lower_bound {
            break;
        }
        let slice = match &b.upper_bound {
            Some(upper) => {
                let top = if taxable < *upper {
                    taxable
                } else {
                    *upper
                };
                top - b.lower_bound
            }
            None => taxable - b.lower_bound,
        };
        tax += slice * b.rate;
    }
    tax
}

/// **PPh 21** — monthly personal income tax (progressive, PTKP-relieved, NPWP-surtaxed).
///
/// Formula: gross_annual = `gross_monthly × 12`; `annual_taxable = max(0, gross_annual − ptkp_relief)`;
/// apply progressive brackets; multiply by `npwp_surtax_multiplier` (1.2×) when `has_npwp == false`;
/// `monthly_tax = annual_tax / 12`, rounded to 2 dp.
///
/// Returns the **monthly** PPh 21 withholding (IDR, 2 dp).
pub fn pph21(
    ptkp: PtkpTier,
    has_npwp: bool,
    gross_monthly: Decimal,
    cfg: &Pph21Config,
) -> Result<Decimal, StatutoryError> {
    let twelve = Decimal::new(12, 0);
    let gross_annual = gross_monthly * twelve;
    let relief = cfg.ptkp_relief(ptkp)?;
    let annual_taxable = if gross_annual > relief {
        gross_annual - relief
    } else {
        Decimal::ZERO
    };
    let mut annual_tax = progressive_tax(annual_taxable, &cfg.brackets);
    if !has_npwp {
        annual_tax *= cfg.npwp_surtax_multiplier;
    }
    let monthly_tax = annual_tax / twelve;
    Ok(money(monthly_tax))
}

/// **BPJS Kesehatan** — health insurance (employee 1%, employer 4%, on salary capped at the cap).
///
/// Returns `(employee, employer)` monthly contributions (IDR, 2 dp).
pub fn bpjs_kesehatan(gross_monthly: Decimal, cfg: &BpjsConfig) -> (Decimal, Decimal) {
    let k = &cfg.kesehatan;
    let capped = if gross_monthly > k.salary_cap {
        k.salary_cap
    } else {
        gross_monthly
    };
    let employee = money(capped * k.employee_rate);
    let employer = money(capped * k.employer_rate);
    (employee, employer)
}

/// Full per-component BPJS Ketenagakerjaan breakdown. All amounts monthly (IDR, 2 dp).
#[derive(Debug, Clone, PartialEq)]
pub struct BpjsTkBreakdown {
    /// JHT — employee share (2%, uncapped).
    pub jht_employee: Decimal,
    /// JHT — employer share (3.7%, uncapped).
    pub jht_employer: Decimal,
    /// JP — employee share (1%, JP-capped).
    pub jp_employee: Decimal,
    /// JP — employer share (2%, JP-capped).
    pub jp_employer: Decimal,
    /// JKK — employer-only (rate by `risk_class`).
    pub jkk_employer: Decimal,
    /// JKM — employer-only (0.3%).
    pub jkm_employer: Decimal,
    /// Sum of all employee-paid components (JHT + JP).
    pub employee_total: Decimal,
    /// Sum of all employer-paid components (JHT + JP + JKK + JKM).
    pub employer_total: Decimal,
}

/// **BPJS Ketenagakerjaan** — employment insurance (JHT + JP + JKK + JKM).
///
/// - JHT: employee `jht_employee_rate`, employer `jht_employer_rate`, both on **uncapped** salary.
/// - JP:  employee `jp_employee_rate`, employer `jp_employer_rate`, both on `min(salary, jp_salary_cap)`.
/// - JKK: employer-only, rate selected by `risk_class` (1–5).
/// - JKM: employer-only, flat `jkm_rate` on uncapped salary.
///
/// Returns the full [`BpjsTkBreakdown`] with per-component and total employee/employer amounts.
pub fn bpjs_ketenagakerjaan(
    gross_monthly: Decimal,
    risk_class: u8,
    cfg: &BpjsConfig,
) -> Result<BpjsTkBreakdown, StatutoryError> {
    let tk = &cfg.ketenagakerjaan;

    let jht_employee = money(gross_monthly * tk.jht_employee_rate);
    let jht_employer = money(gross_monthly * tk.jht_employer_rate);

    let jp_capped = if gross_monthly > tk.jp_salary_cap {
        tk.jp_salary_cap
    } else {
        gross_monthly
    };
    let jp_employee = money(jp_capped * tk.jp_employee_rate);
    let jp_employer = money(jp_capped * tk.jp_employer_rate);

    let jkk_rate = tk
        .jkk_rates_by_risk_class
        .get(&risk_class.to_string())
        .copied()
        .ok_or(StatutoryError::UnknownRiskClass(risk_class))?;
    let jkk_employer = money(gross_monthly * jkk_rate);

    let jkm_employer = money(gross_monthly * tk.jkm_rate);

    let employee_total = money(jht_employee + jp_employee);
    let employer_total = money(jht_employer + jp_employer + jkk_employer + jkm_employer);

    Ok(BpjsTkBreakdown {
        jht_employee,
        jht_employer,
        jp_employee,
        jp_employer,
        jkk_employer,
        jkm_employer,
        employee_total,
        employer_total,
    })
}

/// **THR** — holiday allowance: 1× monthly salary pro-rated by tenure.
///
/// `amount = monthly_salary × min(tenure_months / 12, 1)` — employees with ≥ 12 months tenure get the
/// full month; shorter tenures are pro-rated. `tenure_months` is a `Decimal` so fractional months
/// (e.g. computed from day-level `join_date` math) are honoured. Clamped to a non-negative fraction.
pub fn thr(monthly_salary: Decimal, tenure_months: Decimal) -> Decimal {
    let twelve = Decimal::new(12, 0);
    let fraction = if tenure_months >= twelve {
        Decimal::new(1, 0)
    } else if tenure_months > Decimal::ZERO {
        tenure_months / twelve
    } else {
        Decimal::ZERO
    };
    money(monthly_salary * fraction)
}

// ============================================================================
// compute_statutory — the slip-assembly entry point
// ============================================================================

/// One computed statutory component ready to attach to a salary slip. The neutral output of
/// [`compute_statutory`]: payroll's slip-assembly attaches a GL account (the payable for a deduction,
/// the expense for the THR earning) to turn this into a [`StatutoryLine`](super::payroll_write_service::StatutoryLine).
///
/// `component_type` mirrors the slip-line vocabulary: `"earning"` (THR) or `"deduction"` (PPh 21,
/// BPJS Kesehatan, BPJS Ketenagakerjaan employee share).
#[derive(Debug, Clone)]
pub struct StatutoryComponent {
    pub name: String,
    pub component_type: String, // "earning" | "deduction"
    pub amount: Decimal,
}

impl StatutoryComponent {
    fn earning(name: impl Into<String>, amount: Decimal) -> Self {
        Self { name: name.into(), component_type: "earning".into(), amount }
    }
    fn deduction(name: impl Into<String>, amount: Decimal) -> Self {
        Self { name: name.into(), component_type: "deduction".into(), amount }
    }
}

/// Compose every Indonesia statutory component for one employee's monthly pay into a slip-ready list.
///
/// This is the seam between the (pure, employee-edge-free) calcs above and the slip-assembly layer:
/// it takes the employee's statutory inputs as primitives (PTKP tier, NPWP presence) plus the gross
/// monthly salary, the BPJS JKK `risk_class`, the THR tenure, and the [`StatutoryConfig`], and calls
/// [`pph21`] / [`bpjs_kesehatan`] / [`bpjs_ketenagakerjaan`] / [`thr`] to produce:
///
/// - **THR** earning (tenure-pro-rated; omitted when tenure is zero → no THR), using the monthly gross
///   as the THR base (1× monthly salary).
/// - **PPh 21** deduction (monthly withholding).
/// - **BPJS Kesehatan** employee deduction (1% of capped salary).
/// - **BPJS Ketenagakerjaan** employee deduction (JHT 2% + JP 1% of capped/uncapped salary).
///
/// Only **employee-paid** deductions are emitted — employer shares (BPJS Kesehatan 4%, JKK, JKM, JP
/// employer 2%, JHT employer 3.7%) are real costs but they are NOT withheld from the slip's net pay;
/// they hit a separate employer-cost accrual that a different process books. Zero-amount components
/// are dropped so the slip is not cluttered with no-op lines.
///
/// Returns `Err` only if `risk_class` is unknown or the PTKP tier is absent from the config — both
/// fail closed (a malformed config should never silently produce a wrong net pay).
pub fn compute_statutory(
    ptkp: PtkpTier,
    has_npwp: bool,
    gross_monthly: Decimal,
    risk_class: u8,
    thr_tenure_months: Decimal,
    cfg: &StatutoryConfig,
) -> Result<Vec<StatutoryComponent>, StatutoryError> {
    let mut out = Vec::new();

    // THR earning first (it raises gross; the deductions below are not THR-taxable here — Indonesia
    // taxes THR separately at year-end / on payment under a different scheme, so the monthly PPh 21
    // base stays the ordinary gross).
    let thr_amt = thr(gross_monthly, thr_tenure_months);
    if thr_amt > Decimal::ZERO {
        out.push(StatutoryComponent::earning("THR", thr_amt));
    }

    // PPh 21 monthly withholding on the ordinary gross.
    let pph = pph21(ptkp, has_npwp, gross_monthly, &cfg.pph21)?;
    if pph > Decimal::ZERO {
        out.push(StatutoryComponent::deduction("PPh 21", pph));
    }

    // BPJS Kesehatan — employee share only (1% of capped salary).
    let (kes_employee, _kes_employer) = bpjs_kesehatan(gross_monthly, &cfg.bpjs);
    if kes_employee > Decimal::ZERO {
        out.push(StatutoryComponent::deduction("BPJS Kesehatan", kes_employee));
    }

    // BPJS Ketenagakerjaan — employee share only (JHT + JP). Employer components (JHT-er, JP-er, JKK,
    // JKM) are an employer cost, not a slip deduction.
    let tk = bpjs_ketenagakerjaan(gross_monthly, risk_class, &cfg.bpjs)?;
    if tk.employee_total > Decimal::ZERO {
        out.push(StatutoryComponent::deduction("BPJS Ketenagakerjaan", tk.employee_total));
    }

    Ok(out)
}

// ============================================================================
// Tests — the gate. Hand-computed expected values.
// ============================================================================
#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    /// Convenience: the default (current-law) config.
    fn cfg() -> StatutoryConfig {
        StatutoryConfig::default()
    }

    // ---- PPh 21 -------------------------------------------------------------

    #[test]
    fn pph21_tk0_npwp_12m_is_625000() {
        // TK0, has_npwp, gross 12,000,000/mo → annual 144M − PTKP 54M = 90M taxable
        // → 5%×60M + 15%×30M = 3M + 4.5M = 7.5M annual → /12 = 625,000/mo.
        let monthly = pph21(
            PtkpTier::Tk0,
            true,
            Decimal::new(12_000_000, 0),
            &cfg().pph21,
        )
        .expect("tk0 is in the default ptkp_map");
        assert_eq!(monthly, Decimal::new(625_000, 0));
    }

    #[test]
    fn pph21_no_npwp_surtax_is_120x() {
        // Same case, no NPWP → 625,000 × 1.2 = 750,000.
        let monthly = pph21(
            PtkpTier::Tk0,
            false,
            Decimal::new(12_000_000, 0),
            &cfg().pph21,
        )
        .expect("tk0 is in the default ptkp_map");
        assert_eq!(monthly, Decimal::new(750_000, 0));
    }

    #[test]
    fn pph21_k3_high_income_hits_four_brackets() {
        // K3 (PTKP 72M), has_npwp, gross 50M/mo → annual 600M − 72M = 528M taxable.
        //  5%×60M        =  3,000,000
        // 15%×(250M-60M) = 28,500,000   (190M slice)
        // 25%×(500M-250M)= 62,500,000   (250M slice)
        // 30%×(528M-500M)=  8,400,000   (28M slice)
        // annual = 102,400,000 → /12 = 8,533,333.33 (2 dp, half-up).
        let monthly = pph21(
            PtkpTier::K3,
            true,
            Decimal::new(50_000_000, 0),
            &cfg().pph21,
        )
        .expect("k3 is in the default ptkp_map");
        assert_eq!(monthly, Decimal::from_str("8533333.33").unwrap());
    }

    #[test]
    fn pph21_salary_below_ptkp_is_zero() {
        // Gross below the PTKP relief → no tax. TK3 relief 67.5M; gross 5M/mo = 60M annual < 67.5M.
        let monthly = pph21(
            PtkpTier::Tk3,
            true,
            Decimal::new(5_000_000, 0),
            &cfg().pph21,
        )
        .unwrap();
        assert_eq!(monthly, Decimal::ZERO);
    }

    // ---- BPJS Kesehatan -----------------------------------------------------

    #[test]
    fn bpjs_kesehatan_at_cap_is_120k_480k() {
        // salary 12M, cap 12M → employee 1%×12M = 120,000; employer 4%×12M = 480,000.
        let (emp, er) = bpjs_kesehatan(Decimal::new(12_000_000, 0), &cfg().bpjs);
        assert_eq!(emp, Decimal::new(120_000, 0));
        assert_eq!(er, Decimal::new(480_000, 0));
    }

    #[test]
    fn bpjs_kesehatan_above_cap_clamps() {
        // salary 20M > cap 12M → contributions computed on the 12M cap, not 20M.
        let (emp, er) = bpjs_kesehatan(Decimal::new(20_000_000, 0), &cfg().bpjs);
        assert_eq!(emp, Decimal::new(120_000, 0));
        assert_eq!(er, Decimal::new(480_000, 0));
    }

    #[test]
    fn bpjs_kesehatan_below_cap_pro_rata() {
        // salary 7.5M < cap → 1%×7.5M = 75,000; 4%×7.5M = 300,000.
        let (emp, er) = bpjs_kesehatan(Decimal::new(7_500_000, 0), &cfg().bpjs);
        assert_eq!(emp, Decimal::new(75_000, 0));
        assert_eq!(er, Decimal::new(300_000, 0));
    }

    // ---- BPJS Ketenagakerjaan ----------------------------------------------

    #[test]
    fn bpjs_tk_risk_class_3_at_10m() {
        // gross 10M, risk class 3 (JKK 0.89%).
        //  JHT emp 2%×10M    = 200,000   JHT er 3.7%×10M = 370,000
        //  JP  emp 1%×10M    = 100,000  (10M < JP cap 10,547,400)
        //  JP  er  2%×10M    = 200,000
        //  JKK er  0.89%×10M =  89,000
        //  JKM er  0.3%×10M  =  30,000
        //  emp total = 300,000 ; er total = 370k+200k+89k+30k = 689,000.
        let b = bpjs_ketenagakerjaan(Decimal::new(10_000_000, 0), 3, &cfg().bpjs).unwrap();
        assert_eq!(b.jht_employee, Decimal::new(200_000, 0));
        assert_eq!(b.jht_employer, Decimal::new(370_000, 0));
        assert_eq!(b.jp_employee, Decimal::new(100_000, 0));
        assert_eq!(b.jp_employer, Decimal::new(200_000, 0));
        assert_eq!(b.jkk_employer, Decimal::new(89_000, 0));
        assert_eq!(b.jkm_employer, Decimal::new(30_000, 0));
        assert_eq!(b.employee_total, Decimal::new(300_000, 0));
        assert_eq!(b.employer_total, Decimal::new(689_000, 0));
    }

    #[test]
    fn bpjs_tk_jp_cap_kicks_in_above_cap() {
        // gross 12M > JP cap 10,547,400 → JP computed on the cap.
        //  JP emp 1%×10,547,400 = 105,474 ; JP er 2%×10,547,400 = 210,948.
        //  JHT/JKK/JKM are uncapped → JHT emp 2%×12M = 240,000.
        let b = bpjs_ketenagakerjaan(Decimal::new(12_000_000, 0), 1, &cfg().bpjs).unwrap();
        assert_eq!(b.jp_employee, Decimal::new(105_474, 0));
        assert_eq!(b.jp_employer, Decimal::new(210_948, 0));
        assert_eq!(b.jht_employee, Decimal::new(240_000, 0));
        // JKK class 1 = 0.24% × 12M = 28,800.
        assert_eq!(b.jkk_employer, Decimal::new(28_800, 0));
        // JKM 0.3% × 12M = 36,000.
        assert_eq!(b.jkm_employer, Decimal::new(36_000, 0));
    }

    #[test]
    fn bpjs_tk_unknown_risk_class_errors() {
        // Class 9 is not configured → fail closed with UnknownRiskClass.
        let err = bpjs_ketenagakerjaan(Decimal::new(10_000_000, 0), 9, &cfg().bpjs)
            .err()
            .expect("class 9 should be unknown");
        assert!(matches!(err, StatutoryError::UnknownRiskClass(9)));
    }

    // ---- THR ----------------------------------------------------------------

    #[test]
    fn thr_prorated_6_months_is_half() {
        // 12M salary, 6 months tenure → 12M × (6/12) = 6,000,000.
        let amount = thr(Decimal::new(12_000_000, 0), Decimal::new(6, 0));
        assert_eq!(amount, Decimal::new(6_000_000, 0));
    }

    #[test]
    fn thr_full_at_12_months() {
        // ≥ 12 months → full 1× month, capped.
        let amount = thr(Decimal::new(15_000_000, 0), Decimal::new(12, 0));
        assert_eq!(amount, Decimal::new(15_000_000, 0));
    }

    #[test]
    fn thr_capped_above_12_months() {
        // 24 months tenure → still exactly 1× month (no double-THR).
        let amount = thr(Decimal::new(15_000_000, 0), Decimal::new(24, 0));
        assert_eq!(amount, Decimal::new(15_000_000, 0));
    }

    #[test]
    fn thr_zero_tenure_is_zero() {
        let amount = thr(Decimal::new(12_000_000, 0), Decimal::ZERO);
        assert_eq!(amount, Decimal::ZERO);
    }

    // ---- Config loading & PtkpTier interop ----------------------------------

    #[test]
    fn ptkp_tier_roundtrips_as_snake_case() {
        // Mirrors backbone_employee::PtkpTier's lowercase Display/FromStr exactly.
        for tier in [
            PtkpTier::Tk0,
            PtkpTier::Tk1,
            PtkpTier::Tk2,
            PtkpTier::Tk3,
            PtkpTier::K0,
            PtkpTier::K1,
            PtkpTier::K2,
            PtkpTier::K3,
        ] {
            let s = tier.to_string();
            assert_eq!(PtkpTier::from_str(&s).unwrap(), tier);
        }
        // An unknown label is rejected (interop safety).
        assert!(PtkpTier::from_str("tk9").is_err());
    }

    #[test]
    fn config_from_yaml_drives_same_calc_as_default() {
        // The shipped config/application.yml `statutory:` block must produce identical results to the
        // baked-in Default — proving the calcs are config-driven, not hardcoded.
        let yaml = include_str!("../../../config/application.yml");
        let loaded = StatutoryConfig::from_yaml_str(yaml).expect("application.yml parses");

        let via_default =
            pph21(PtkpTier::Tk0, true, Decimal::new(12_000_000, 0), &cfg().pph21).unwrap();
        let via_loaded =
            pph21(PtkpTier::Tk0, true, Decimal::new(12_000_000, 0), &loaded.pph21).unwrap();
        assert_eq!(via_default, via_loaded);
        assert_eq!(via_loaded, Decimal::new(625_000, 0));

        let (emp_default, _) = bpjs_kesehatan(Decimal::new(12_000_000, 0), &cfg().bpjs);
        let (emp_loaded, _) = bpjs_kesehatan(Decimal::new(12_000_000, 0), &loaded.bpjs);
        assert_eq!(emp_default, emp_loaded);
    }

    #[test]
    fn config_missing_statutory_block_falls_back_to_default() {
        // A YAML with no `statutory:` key still boots with the current-law defaults.
        let yaml = "server:\n  port: 8080\n";
        let loaded = StatutoryConfig::from_yaml_str(yaml).unwrap();
        let monthly = pph21(PtkpTier::Tk0, true, Decimal::new(12_000_000, 0), &loaded.pph21).unwrap();
        assert_eq!(monthly, Decimal::new(625_000, 0));
    }

    // ---- compute_statutory (the slip-assembly entry point) ------------------

    #[test]
    fn compute_statutory_tk0_npwp_12m_full_tenure_emits_four_components() {
        // TK0, has_npwp, gross 12M, risk class 3, full (12-month) tenure, default config. Hand-computed:
        //   THR earning           = thr(12M, 12)           = 12,000,000  (1× month, full tenure)
        //   PPh 21 deduction      = 625,000                 (verified above)
        //   BPJS Kesehatan emp    = 1% × min(12M, 12M cap) = 120,000
        //   BPJS TK emp (JHT+JP)  = 2%×12M + 1%×10,547,400 = 240,000 + 105,474 = 345,474
        // total deductions = 625,000 + 120,000 + 345,474 = 1,090,474.
        let comps = compute_statutory(
            PtkpTier::Tk0,
            true,
            Decimal::new(12_000_000, 0),
            3,
            Decimal::new(12, 0),
            &cfg(),
        )
        .expect("tk0 + risk class 3 are in the default config");

        let by_name: std::collections::HashMap<String, (String, Decimal)> = comps
            .iter()
            .map(|c| (c.name.clone(), (c.component_type.clone(), c.amount)))
            .collect();
        assert_eq!(by_name["THR"], ("earning".into(), Decimal::new(12_000_000, 0)));
        assert_eq!(by_name["PPh 21"], ("deduction".into(), Decimal::new(625_000, 0)));
        assert_eq!(by_name["BPJS Kesehatan"], ("deduction".into(), Decimal::new(120_000, 0)));
        assert_eq!(by_name["BPJS Ketenagakerjaan"], ("deduction".into(), Decimal::new(345_474, 0)));
        assert_eq!(comps.len(), 4, "exactly four components");

        let total_deductions: Decimal = comps
            .iter()
            .filter(|c| c.component_type == "deduction")
            .map(|c| c.amount)
            .sum();
        assert_eq!(total_deductions, Decimal::new(1_090_474, 0));
    }

    #[test]
    fn compute_statutory_zero_tenure_drops_thr() {
        // Tenure 0 → THR is 0 → omitted. The three deductions remain (they don't depend on tenure).
        let comps = compute_statutory(
            PtkpTier::Tk0,
            true,
            Decimal::new(12_000_000, 0),
            1,
            Decimal::ZERO,
            &cfg(),
        )
        .unwrap();
        assert!(!comps.iter().any(|c| c.name == "THR"), "zero-tenure THR must be dropped");
        assert_eq!(comps.len(), 3, "PPh21 + BPJS Kesehatan + BPJS TK only");
    }
}
