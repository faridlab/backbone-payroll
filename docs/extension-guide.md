# backbone-payroll — Extension Guide

## Public surface (stable)
- **GL port** (`application::service::payroll_gl`): `GlPostSink` + the contract envelope
  (`AccountingPostEnvelope`, `GlPostLine`, `GlPostAck`, `GlPostRejected`) — the salary-journal posting
  seam a composing service implements over accounting's `PostingService`. Zero normal Cargo edge; the
  envelope is the wire contract (duplicated per producer by design).
- **Events** (`application::service::payroll_events`): `PayrollPosted` (with `PayrollPayable`), the
  `PayrollEvent` union, and `PayrollEventSink` (a consuming service supplies its own — bus, outbox, …).
  `backbone-payments` subscribes and settles from the event alone: net pay clears
  `salary_payable_account_id` (amount = `total_net`), and each `payables[]` entry
  (`{gl_account_id, amount, statutory}`) is remitted to its account — statutory ones to their authority.
- **Write path** (`application::service::payroll_write_service::PayrollWriteService`): the guarded verbs
  (`create_structure`, `create_payroll_entry`, `add_salary_slip`, `process_payroll_entry`,
  `post_payroll_entry`) + DTOs (`NewStructure`, `NewComponent`, `NewPayrollEntry`, `NewSalarySlip`,
  `StatutoryLine`, `PostOutcome`, `PayrollError`).

## How a consuming service uses payroll
Supply the statutory deduction amounts (BPJS, PPh 21) on each `add_salary_slip` call as `StatutoryLine`s
— computed by the deferred `backbone-tax-id` overlay from the Employee's `tax_status` (PTKP) — exactly as
billing supplies tax lines. Read unpaid days from `hr.period_summary` and pass them as `unpaid_days`.
Implement `GlPostSink` over accounting to post the run; subscribe to `PayrollPosted` to settle — the event
carries the full payable breakdown (net-pay account + each remittance by account), so settlement never
re-queries payroll's private slip tables. The consumer's sink + adapter survive a regen of both modules (§5).

## Not a contract
- The 12 generated CRUD endpoints per entity are convenience scaffolding. Do **not** move a run's status
  or write a slip's amounts through the generic PATCH surface — it bypasses the net-pay math, the
  balanced-posting build, and the post-once gate. Use `PayrollWriteService`.
- `// <<< CUSTOM` blocks preserve local edits only; not a cross-module extension point.

## Invariants a consumer must not break
- `net_pay = gross_pay − total_deductions` and is never negative; the salary journal always balances
  (`gross = net + Σ deductions`).
- A run posts to the ledger **at most once** (idempotent on `source_id = payroll_entry_id`).
- The statutory **amounts** are supplied to payroll; the PTKP/BPJS/PPh-21 **math** lives in the tax
  overlay — don't recompute it here.
