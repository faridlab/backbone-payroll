# backbone-payroll — FSD

## Entities
SalaryStructure (`company_id`, `name`, `is_active`) · SalaryComponent (`structure_id`, `name`,
`component_type`, `amount`, `gl_account_id` logical FK accounting) · PayrollEntry (`company_id`,
`period_year`/`period_month` unique per company, `status`, `salary_expense_account_id`/
`salary_payable_account_id` logical FKs, `total_gross`/`total_deductions`/`total_net`, `journal_id`/
`accounting_post_id` logical FKs, `posting_date`) · SalarySlip (unique `(payroll_entry_id, employee_id)`;
`employee_id`/`structure_id` logical FKs, `working_days`/`unpaid_days`, `gross_pay`/`total_deductions`/
`net_pay`) · SalarySlipLine (`salary_slip_id`, `name`, `component_type`, `is_statutory`, `amount`,
`gl_account_id`). Enums: ComponentType {earning, deduction}, PayrollStatus {draft, processed, posted,
cancelled}. Money is IDR, 2dp, half-away-from-zero.

## Write path (`PayrollWriteService`, hand-authored, user-owned)
- `create_structure(NewStructure)` → structure with components
- `create_payroll_entry(NewPayrollEntry)` → draft run (unique per company/period)
- `add_salary_slip(run, NewSalarySlip)` → prorated earnings − deductions = net (non-negative); one per
  employee; DRAFT only
- `process_payroll_entry(run)` → roll slips up into run totals, `draft → processed` (one-way)
- `post_payroll_entry(run, posting_date, &dyn GlPostSink, &dyn PayrollEventSink)` → build the balanced
  posting, drive the sink, gate `processed → posted`, emit `PayrollPosted`; **posts at most once**

Errors: `PayrollError {Db, NotFound, InvalidState, Invalid, Unbalanced, GlRejected}`.

## Seam (ports — zero normal Cargo edge)
- **Post → accounting (proven, PGSEAM-1/2):** payroll emits ONE balanced `AccountingPostEnvelope` through
  `GlPostSink`; the ACL adapter maps it into accounting's `PostingRequest`. `source_type='payroll'` (the
  8th producer; registered via `ALTER TYPE posting_source_type ADD VALUE 'payroll'`). Idempotent per run.
- **Read → hr (proven, PHRSEAM-1):** unpaid days come from `hr.period_summary(employee, from, to)`; the
  salary identity is read from the Employee master. Payroll never mutates HR.
- **Outbound:** `PayrollPosted` for `backbone-payments` to settle — carries `salary_payable_account_id` +
  total_net (net-pay leg) and `payables: Vec<PayrollPayable {gl_account_id, amount, statutory}>` (each
  remittance by account), so the consumer settles from the event alone (PGC-5).

## Test oracle
`payroll_golden_cases` (5: PGC-1 full-month net pay + balanced post, PGC-2 unpaid-day proration, PGC-3
run roll-up + deduction grouping, PGC-4 post-once idempotency, PGC-5 PayrollPosted payable breakdown),
`integrity_probes` (7: PIP-1 net-never-negative, PIP-2 no-duplicate-slip, PIP-3 no-post-before-process,
PIP-4 no-empty-process, PIP-5 transition-gates-one-way, PIP-6 one-run-per-company-period, PIP-7
negative-unpaid-days-cannot-inflate-gross),
`payroll_gl_seam` (2: PGSEAM-1 balanced journal in REAL accounting, PGSEAM-2 re-post reuses one journal),
`payroll_hr_seam` (1: PHRSEAM-1 REAL-HR unpaid leave prorates gross) + §5 round-trip. **15 tests.**

> The generated `integration_tests.rs` hits an external HTTP server (`API_BASE_URL`, default
> `127.0.0.1:3000`) and is environmental scaffolding, not part of this module's correctness gate.
