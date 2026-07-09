# ADR-001 — The payroll run engine, the salary journal, and the HR/tax seams

Status: accepted · 2026-07-08 · Tier 5b (People pillar; the 8th GL producer)

## Context
Payroll is built **after** backbone-hr because a salary run has nothing to run against without an employee
roster, salary identity, and leave/attendance inputs. ERPNext bakes payroll into its HR app with no
Indonesia pack; the value here is a clean run engine (gross→deductions→net) that lands ONE balanced
salary journal in the ledger, while keeping the Indonesia statutory *math* in a deferred overlay.

## Decision
1. **The run is the unit of posting; the slip is the unit of computation.** A `PayrollEntry` moves
   `draft → processed → posted`; `SalarySlip`s are built on the draft, rolled up on process, and the run —
   not each slip — emits ONE aggregate posting. This mirrors how a payroll journal actually books.
2. **Payroll posts through the GL contract as `source_type='payroll'` (the 8th producer).** It emits ONE
   balanced `AccountingPostEnvelope` — `Dr Salary Expense (gross) · Cr Salary Payable (net) · Cr Σ
   deduction-account` — through a `GlPostSink` (zero normal Cargo edge). Because `gross = net + Σ
   deductions`, it balances by construction. Idempotent per run (`source_id = payroll_entry_id`);
   re-posting reaches the ledger zero further times (transition-gated `processed → posted`).
3. **The statutory amounts are supplied, not computed.** BPJS and PPh 21 amounts arrive as
   `StatutoryLine`s on `add_salary_slip` — like billing's tax lines — computed by the deferred
   `backbone-tax-id` overlay from the Employee's `tax_status` (PTKP). Payroll carries the mechanism (a
   balanced run + grouped payables), not the DJP rules.
4. **HR unpaid days actually reduce pay.** Earnings are prorated by `(working − unpaid)/working`, with
   `unpaid_days` sourced from `hr.period_summary` — so the HR→payroll link is load-bearing, not cosmetic
   (proven against REAL backbone-hr, PHRSEAM-1).
5. **The deduction payables are aggregate control accounts, not AR/AP subledgers.** Net pay and statutory
   payables credit `current_liability` control accounts (no per-employee party) — the salary journal is an
   aggregate; per-employee settlement is `backbone-payments`' job.

## Consequences
- Turn payroll off and no salary ever books; it is the only place employee money moves into the ledger.
- Proven end-to-end against the REAL accounting ledger (`tests/payroll_gl_seam.rs`) and the REAL HR read
  (`tests/payroll_hr_seam.rs`), and survives regen (§5).
- Settlement is decoupled: `PayrollPosted` hands the net + payables to backbone-payments.

## Parking lot (each with a gate)
- **PayrollPosted shipped only lump totals — settlement was blocked** — FIXED (completeness council
  2026-07-08): the event carried `total_deductions` as one number with no account ids, so
  `backbone-payments` could not split/remit each statutory payable (BPJS, PPh 21) to its authority without
  re-querying payroll's private `salary_slip_lines` (a boundary violation). The breakdown was computed at
  post time and discarded. Fixed by carrying `salary_payable_account_id` (net-pay leg) + `payables: Vec<
  PayrollPayable {gl_account_id, amount, statutory}>` on `PayrollPosted`, populated from the grouping the
  post path already builds (PGC-5, proven-by-revert).
- **Negative `unpaid_days` inflated gross above the structure** — FIXED (maturity council 2026-07-08): the
  proration clamped only the UPPER bound of `unpaid_days`, and `@non_negative` emitted no DB CHECK, so a
  bad upstream `hr.period_summary` value (`unpaid_days < 0`) drove the factor above 1 and booked a
  balanced-but-over-booked salary journal silently. Fixed with a two-sided clamp (`unpaid.clamp(0,
  working_days)`) + DB CHECKs (`20260708000100_payroll_balance_guards`: non-negativity + `unpaid <=
  working` + the `net = gross − deductions` identity) as the backstop against any writer (PIP-7,
  proven-by-revert).
- **Employer-side contributions not posted** — v1 books only employee deductions; employer BPJS
  (JHT/JKK/JKM/Kesehatan) is a company expense + a matching payable that the salary journal omits. Gate:
  an employer-contribution component type that adds `Dr Expense · Cr Payable` pairs to the run post.
- **Net pay posted to a control account without employee party** — no per-employee AP subledger, so
  "who is owed how much" is not ageable from the ledger. Gate: split net pay into per-employee party
  lines (subtype `accounts_payable` + `party_type='employee'`) or resolve it in backbone-payments.
- **Generic CRUD/PATCH exposes run status + slip amounts** — a client can PATCH a run to `posted` or
  rewrite `net_pay` outside the engine, bypassing the balanced-post build and the post-once gate. Gate: an
  authorization review of the generic mutation surface (as HR).
- **No reversal / off-cycle correction** — a posted run has no `cancelled`-with-reversing-entry path;
  `posting_type` is always `original`. Gate: a reversal verb emitting the contra posting.
- **Proration model is fixed-fraction** — `(working − unpaid)/working` per earning; it does not handle
  mid-period joiners/leavers or daily-rate vs monthly-rate components. Gate: a proration policy per
  component.
- **Statutory amounts unvalidated** — payroll trusts the supplied BPJS/PPh 21 figures; a wrong overlay
  value books wrong. Gate: overlay-side golden cases + a sanity band once backbone-tax-id lands.
