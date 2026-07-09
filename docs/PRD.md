# backbone-payroll — PRD

Tier 5b · People pillar · the **8th GL producer** · reads `backbone-hr`.

## Why
An Indonesia SMB that has an employee roster (backbone-hr) needs to **pay it**: assemble each person's
pay from their salary structure, reduce it for unpaid days, subtract the statutory deductions (BPJS, PPh
21), and land ONE balanced salary journal in the ledger. This is the lean payroll core — the salary run
engine — that turns the HR people master into a paid-and-posted period.

## Scope (KEEP — tier5-deferred.md §3)
- **SalaryStructure (+ SalaryComponent)** — a reusable set of earning/deduction components (Gaji Pokok,
  Tunjangan, …), each carrying the GL account it hits. The template a slip is built from.
- **PayrollEntry** — one salary run for a company/period (`draft → processed → posted`), unique per
  `(company, year, month)`. Rolls its slips up into run totals and owns the GL post.
- **SalarySlip (+ SalarySlipLine)** — one employee's pay for one run: earnings from the structure
  **prorated by HR unpaid days**, minus fixed + supplied statutory deductions. `net = gross − deductions`,
  never negative.
- **The salary run engine** — `gross → deductions → net`, run roll-up, and ONE balanced posting per run:
  `Dr Salary Expense (gross) · Cr Salary Payable (net) · Cr Σ deduction-account`. Posted **at most once**
  (idempotent per run), `source_type='payroll'`.
- **HR read** — unpaid days come from `hr.period_summary` (proven against REAL backbone-hr); the salary
  identity (PTKP, NPWP) is read from the Employee master.

## Non-goals (CUT / DEFER — tier5-deferred.md §3)
- **The statutory math itself** — BPJS Kesehatan/Ketenagakerjaan rates and PPh 21 (PTKP relief, TER/
  progressive brackets) are the **deferred Indonesia overlay** (`backbone-tax-id`); amounts are *supplied*
  to a slip like billing's tax lines, not computed here.
- Employer-side contributions as a separate expense/liability (v1 posts employee deductions only), loan/
  advance sub-ledgers, THR/bonus off-cycle runs, employer BPJS accrual.
- Net-pay **settlement** (paying employees / remitting the payables) — that is `backbone-payments`
  consuming `PayrollPosted`; payroll posts the obligation, it does not disburse.
- Payslip PDF/e-mail, per-employee tax-slip (1721-A1) generation, multi-currency payroll.

## Success criteria
- Net-pay math is exact under proration and grouping (golden cases), and the salary journal always
  balances (`gross = net + Σ deductions`).
- The run posts **at most once** to the REAL ledger (idempotent), accepted as `source_type='payroll'`.
- HR unpaid days actually reduce pay (proven against REAL backbone-hr).
- Zero normal Cargo edge; survives a full codegen regen (§5).
