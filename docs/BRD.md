# backbone-payroll — BRD

## Documents
SalaryStructure (+ SalaryComponent) · PayrollEntry (the run) · SalarySlip (+ SalarySlipLine). Own Postgres
schema `payroll`. **Posts GL** (the 8th producer) via the AccountingPost contract, `source_type='payroll'`.

## Business rules

**BR-1 (structure).** `create_structure` defines a named set of earning/deduction components, each with a
GL account and a non-negative amount. A structure needs a name and ≥1 component.

**BR-2 (open a run).** `create_payroll_entry` opens a `draft` run for a company/period, **unique per
`(company, period_year, period_month)`** — a duplicate is refused. It carries the salary-expense and
salary-payable control accounts the post will use.

**BR-3 (build a slip — the net-pay invariant).** `add_salary_slip` builds one employee's slip on a
**draft** run: earnings from the structure are **prorated by unpaid days** (`amount × (working −
unpaid)/working`, unpaid clamped to working), fixed deductions and the **supplied** statutory deductions
subtract, and `net = gross − total_deductions`. `net` must be **non-negative** — deductions exceeding gross
are refused. An employee appears **at most once** per run.

**BR-4 (process).** `process_payroll_entry` rolls the run's slips up into `total_gross/total_deductions/
total_net` and claims `draft → processed`. A run with no slips cannot process. One-way (a processed run
cannot be re-processed; a slip cannot be added after processing).

**BR-5 (post — the GL invariant).** `post_payroll_entry` builds ONE **balanced** posting — `Dr Salary
Expense (gross) · Cr Salary Payable (net) · Cr Σ deduction-account (grouped)` — drives the `GlPostSink`,
then transition-gates `processed → posted` with the returned journal. Because `gross = net + Σ
deductions`, it balances. Posts **at most once**: re-posting a posted run reaches the ledger zero further
times and returns the same journal. Emits `PayrollPosted`.

**BR-6 (deduction grouping).** Deduction lines across all slips are **grouped by GL account** into one
credit line per payable account (Salary Payable, BPJS Payable, PPh 21 Payable) — the salary journal is an
aggregate, not per-employee.

## Events
`PayrollPosted` — the signal `backbone-payments` consumes to settle. Carries run id, journal id, the
gross/deductions/net control totals, **and the payable breakdown**: `salary_payable_account_id` (net pay
clears here, amount = total_net) + `payables[]` = each deduction payable by account with a `statutory`
flag (BPJS/PPh 21 → remit to authority). The consumer settles net + remits each payable from the event
alone — no re-query of payroll's private slip tables (completeness council 2026-07-08).

## Deferred (with reason)
The statutory **math** (BPJS/PPh 21 → backbone-tax-id overlay; amounts supplied here), employer-side
contributions, loan/advance sub-ledgers, THR/off-cycle runs, net-pay settlement (→ backbone-payments),
payslip/tax-slip generation.
