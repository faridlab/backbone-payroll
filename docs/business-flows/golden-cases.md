# backbone-payroll — business flows & golden cases

## Flow: structure → run → slip → process → post (the salary run engine)
```
create_structure (earning/deduction components, each with a GL account)
   │
   ▼  create_payroll_entry → draft run (unique per company/period)
   │
   ▼  add_salary_slip (DRAFT) → earnings prorated by HR unpaid days − deductions = net (≥ 0), one/employee
   │
   ▼  process_payroll_entry → roll slips up into run totals, [draft→processed] (one-way)
   │
   ▼  post_payroll_entry → build ONE balanced post + drive GlPostSink + [processed→posted] → PayrollPosted
   │      Dr Salary Expense (gross) · Cr Salary Payable (net) · Cr Σ deduction-account (grouped)
   │      └─ re-post → reaches the ledger 0 more times, returns the same journal (posts at most once)
   │
   └▶ backbone-payments consumes PayrollPosted to settle net + remit payables
```
Statutory deduction amounts (BPJS, PPh 21) are **supplied** to `add_salary_slip` (the deferred tax
overlay computes them). Unpaid days come from `hr.period_summary`.

## Golden cases (`tests/payroll_golden_cases.rs`)
- **PGC-1 — full-month net pay.** Gross 12,000,000 − (BPJS 240,000 + PPh21 500,000) = **11,260,000**; the
  posting balances, Dr Salary Expense = 12,000,000, Cr Salary Payable = 11,260,000, `source_type='payroll'`.
- **PGC-2 — unpaid-day proration (the HR link).** 2 unpaid of 22 working days scales earnings by 20/22 →
  gross **10,909,090.91** (Gaji Pokok 9,090,909.09 + Tunjangan 1,818,181.82); net = gross − 740,000 =
  **10,169,090.91**.
- **PGC-3 — run roll-up + deduction grouping.** Two 12,000,000 slips → run totals gross 24,000,000 /
  deductions 1,480,000 / net 22,520,000; BPJS across both slips grouped into **one** credit line = 480,000.
- **PGC-4 — post-once idempotency.** Re-posting a posted run reaches the sink **once**; both calls return
  the same journal, the second with `already=true`.
- **PGC-5 — settlement-facing payable breakdown.** `PayrollPosted` carries `salary_payable_account_id`
  (net pay, total_net) + `payables[]` = BPJS (240,000, statutory) + PPh 21 (500,000, statutory),
  reconciling to `total_deductions` — `backbone-payments` settles from the event alone.

## Integrity probes (`tests/integrity_probes.rs`)
- **PIP-1 — net never negative.** Deductions (6,000,000) exceeding gross (5,000,000) are refused.
- **PIP-2 — no duplicate slip.** An employee appears at most once per run.
- **PIP-3 — no post before process.** A draft run cannot post.
- **PIP-4 — no empty process.** A run with no slips cannot process.
- **PIP-5 — transition gates one-way.** A processed run cannot be re-processed; no slip after processing.
- **PIP-6 — one run per company/period.** A duplicate `(company, year, month)` run is refused.
- **PIP-7 — proration never inflates gross.** A negative `unpaid_days` is clamped to [0, working]; gross
  never exceeds the structure base (maturity council). DB CHECKs backstop any writer.

## Seams
- **`tests/payroll_gl_seam.rs` (REAL accounting).** PGSEAM-1: the salary journal lands balanced in the
  real ledger (Dr Expense 10,000,000 · Cr Payable 9,260,000 · Cr BPJS 240,000 · Cr PPh21 500,000; Σ = 0),
  accepted as `source_type='payroll'`. PGSEAM-2: re-posting reuses the one journal (expense not doubled).
- **`tests/payroll_hr_seam.rs` (REAL hr).** PHRSEAM-1: HR approves 2 unpaid leave days → `period_summary`
  reports 2 → payroll prorates a 12,000,000 structure to 10,909,090.91.

## §5 round-trip (`scripts/payroll_gl_seam_roundtrip.sh`)
Regen (`--force`) leaves the seam files (`payroll_gl.rs`, `payroll_events.rs`, `payroll_write_service.rs`)
byte-identical; the oracle + both seams re-run green.
