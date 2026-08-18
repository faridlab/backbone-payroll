# Golden cases — hand-computed pins for the statutory math

Every statutory figure this module computes is pinned by a test whose expected value was computed
**by hand from the regulation's arithmetic**, not by running the code and copying the output. This
document records each derivation so a reviewer can re-derive the pin from the rule and catch a
wrong formula wearing a right-looking test.

Where the cases live:

| Suite | File | Kind |
|---|---|---|
| Statutory unit goldens | `src/application/service/statutory_calcs.rs` (test module) | pure functions, no DB |
| Run-engine goldens | `tests/payroll_golden_cases.rs` (`pgc*`) | real DB, run lifecycle |
| HR-input seam | `tests/payroll_hr_seam.rs` (`phrseam*`) | real DB + real backbone-employee/attendance inputs |
| Integrity probes | `tests/integrity_probes.rs` (`pip*`) | fail-closed / boundary behaviour |

> **Seed values are defaults, not authority.** The parameter tables seeded by migration (brackets,
> PTKP tiers, TER rates, BPJS rates, overtime bands) are best-effort transcriptions of the cited
> regulations, included so a fresh composition computes real-shaped numbers out of the box. They
> have not been through a statutory review. A confirmed correction lands as a **new
> `effective_from` row** — never an edit to history and never a code change (see
[ADR-002](adr/ADR-002-statutory-parameter-tables.md)).

## PPh 21 — progressive brackets (npwp path)

`pph21_tk0_npwp_12m_is_625000`
: TK/0, has NPWP, gross 12,000,000/mo → annual 144M − PTKP 54M = 90M taxable →
  5% × 60M + 15% × 30M = 7,500,000 annual → ÷12 = **625,000**/mo.

`pph21_no_npwp_surtax_is_120x`
: Same input without NPWP → 625,000 × 1.2 = **750,000** (the Art. 21 surtax pinned as a pure
  multiplier).

`pph21_k3_high_income_hits_four_brackets`
: K/3 (PTKP 72M), gross 50M/mo → annual 600M − 72M = 528M taxable → 5%×60M + 15%×190M + 25%×250M
  + 30%×28M = 102,400,000 annual → ÷12 = **8,533,333.33** (2 dp, half-up) — the piecewise walk
  across all four brackets.

`pph21_salary_below_ptkp_is_zero`
: Taxable below PTKP → **0**, and the pin asserts the zero flows through `compute_statutory`.

## PPh 21 — TER (ter_category path)

TER replaces the bracket walk with one rate applied to a reduced base (PMK 168/2023):

- **Base** = gross monthly − **JHT employee − JP employee** (the two employment-program employee
  shares only — Kesehatan is *not* deducted). Pinned by `pph21_ter_base_uses_unrounded_insurance_products`:
  gross 9,999,999.99 → share = 3% × gross = 299,999.9997 → base = 9,699,999.9903, i.e. the base
  subtracts the **unrounded** products, not the sen-rounded per-component figures.
- **Rate** = the seeded TER band for the employee's category whose `lower_bound` is the greatest
  `lower_bound <= base` (lower-bound inclusive — pinned by `pph21_ter_band_edge_is_lower_bound_inclusive`).
- **Non-NPWP** ×1.2, same as the bracket path.

`pph21_ter_a_10m_base_9_7m_is_242500`
: Gross 10,000,000: JHT 2% = 200,000, JP 1% = 100,000 → base 9,700,000. TER A band
  [9,650,000, 10,350,000) → 2.5% → 9,700,000 × 0.025 = **242,500**.

`pph21_ter_a_no_npwp_is_291000`
: 242,500 × 1.2 = **291,000**.

`pph21_ter_empty_table_fails_closed`
: No bands for the category → `Err` (never a silent 0% — an empty rate table is broken data, not
  a tax holiday).

`pph21_method_labels_are_the_audit_stamp`
: The slip's `tax_method` column records which path priced it (`npwp_brackets` / `ter_a|b|c`), so a
  later re-check can reproduce the figure from the same table.

`phrseam4_computed_slip_dispatches_on_ter_category`
: End-to-end against a real employee row: a `ter_category` set on the employee's taxes prices via
  the TER table; null falls back to the npwp-brackets path. `phrseam2_statutory_drives_indonesian_net_pay`
  pins the brackets-path net (10,000,000 − kes 100,000 − TK 300,000 − PPh 242,500) through the same
  computed-slip verb.

## BPJS

`bpjs_kesehatan_at_cap_is_120k_480k` / `..._above_cap_clamps` / `..._below_cap_pro_rata`
: Employee 1% / employer 4% of salary **capped at the seeded wage cap** (12,000,000): at-cap →
  120,000 / 480,000; above-cap clamps to the same; below-cap is pro-rata.

`bpjs_tk_risk_class_3_at_10m`, `bpjs_tk_jp_cap_kicks_in_above_cap`, `bpjs_tk_unknown_risk_class_errors`
: JKK rate follows the company's risk class (5 classes seeded); JP employee 2% / employer 3% with
  the seeded JP wage cap; an unseeded risk class **errors** (fail closed).

## THR proration

`thr_*` (4 cases)
: THR = one month's salary × `min(tenure_months, 12)/12`; the pins cover half (6 mo), full (12 mo),
  capped (>12 mo), and zero tenure.

## Overtime

Hourly base = `monthly_base / 173` (the statutory divisor). Workday bands: hour 1 → 1.5×, hour 2+
→ 2.0×, **resetting with each calendar day's stretch** — only a per-day walk prices every day's
first hour at 1.5×.

`overtime_10h_at_8_7m_base_is_980635_84`
: One day's 10h: 1.5 + 9 × 2.0 = 19.5 multiplier-hours × (8,700,000/173) = 980,635.838… →
  **980,635.84**. A period's pay is the sum of this over its days.

`overtime_first_hour_only_is_1_5x`, `overtime_fractional_hour_prorates_the_band`, `overtime_zero_hours_or_base_is_zero`
: Band edges of the same schedule.

`overtime_hour_beyond_the_last_band_fails_closed`, `overtime_missing_bands_fail_closed`
: Hours past the table's coverage, or a gap between bands, → `MissingOvertimeBands`, never a
  neighbouring band's rate.

`phrseam3_overtime_comes_from_attendance_time_debt`
: End-to-end against real attendance rows: five days × 2h (120 min in `time_debt.overtime_minutes`)
  = 10h total but **5 × 3.5 = 17.5 multiplier-hours** (each day restarts at 1.5×) ×
  (8,700,000/173) = **880,057.80**, riding gross as an earning (gross = 9,580,057.80). This is the
  pin that a window-sum implementation would get wrong (10h → 19.5 multiplier-hours = 980,635.84).

## Parameter resolution (as-of)

`pgc6_params_resolve_as_of_effective_from`
: A run's period resolves the parameter rows `effective_from <= first day of period`, newest wins —
  a correction row added for next month does not reprice this month.

`pgc7_pre_effective_period_run_refuses_to_compute`
: A period before any row's `effective_from` → error (422 `no_statutory_params_for_period`), not
  zero-rated defaults.

`pgc8_lone_correction_rows_refuse_the_period`
: A table holding only rows `effective_from` in the future (e.g. a lone correction landed before
  its base set) reads as an **incomplete set** and refuses the period. The alternative — treating
  "no row as-of today" as zero tax — silently zeroes a run (the failure this test pins out).

## Adding a golden

1. Compute the expected value **from the regulation's arithmetic on paper**; cite the rule in a
   comment above the assert.
2. If the pin encodes a seeded table value, say which migration row it traces to.
3. Never update a pin to match new output — either the formula or the seed changed, and the commit
   must say which and why.
