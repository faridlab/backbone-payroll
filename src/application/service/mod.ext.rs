// This directory's extension (hand-written; ADR-0031). The generated mod.rs (lib.rs for
// src/) pulls it in with include!, so its lines resolve against this directory.

pub mod payroll_gl;
pub mod payroll_events;
pub mod payroll_remittance;
pub mod payroll_write_service;
pub mod payslip_pdf;
pub mod statutory_calcs;
// Where a slip's overtime hours come from (attendance owns the time_debt semantics; the pool
// default mirrors its exported read so payroll works standalone).
pub mod overtime_port;
pub mod timesheet_port;
// Where a slip's employee-side tax facts come from (the employee module owns them; the pool
// default mirrors its exported read so payroll works standalone).
pub mod employee_inputs_port;
// ADR-005 consumers: the two payroll-side receivers for the lifecycle compound events. Each appends
// a compensation_changes row idempotently (inbox dedup on the envelope id). Registered on the
// integration bus in backbone-hr-app's main.rs.
pub mod promotion_salary_handler;
pub mod offboarding_settlement_handler;
// ADR-005 consumer: the payroll-side receiver for `onboarding.completed`. Seeds the initial
// compensation_changes row from the joiner's starting salary, idempotently (inbox dedup on the
// envelope id). Registered on the integration bus in backbone-hr-app's main.rs.
pub mod onboarding_enrolled_handler;
// Consumer: recruitment.hired → the new employee's first compensation change (the offered salary,
// effective on the first day), idempotently.
pub mod hire_compensation_handler;
// A run beside the run before it: the read behind checking a run before it is approved.
pub mod payroll_comparison;
// Changing a structure from a date: plan, withdraw, and the daily pass that applies due changes.
pub mod pay_schedule;
pub use payroll_gl::{
    AccountingPostEnvelope, GlPostAck, GlPostLine, GlPostRejected, GlPostSink, UnwiredGlSink,
};
pub use payroll_events::{
    LoggingSink, PayrollEvent, PayrollEventError, PayrollEventSink, PayrollPayable, PayrollPosted,
};
pub use payroll_remittance::{
    RemitAck, RemittanceInstruction, RemittanceSeamError, RemittanceSink, UnwiredRemittance,
};
pub use payroll_write_service::{
    ComputedSlipRequest, NewComponent, NewPayrollEntry, NewSalarySlip, NewStructure, PayrollError, SlipPreview,
    PayrollWriteService, PostOutcome, RemitOutcome, StatutoryAccounts, StatutoryLine,
};
pub use overtime_port::{OvertimeInputs, PoolOvertimeInputs};
pub use employee_inputs_port::{
    EmployeeStatutory, EmployeeStatutoryInputs, PoolEmployeeStatutoryInputs,
};
pub use statutory_calcs::{
    bpjs_kesehatan, bpjs_ketenagakerjaan, compute_statutory, overtime_pay, pph21, pph21_ter, thr,
    BpjsConfig, BpjsKesehatanConfig, BpjsTkBreakdown, BpjsTkConfig, OvertimeBand, OvertimeConfig,
    Pph21Bracket, Pph21Config, Pph21Method, PtkpTier, StatutoryComponent, StatutoryConfig,
    StatutoryError, TerCategory, TerRateBand,
};
pub use pay_schedule::{structure_versions, PlanOutcome, PlannedChange, PlannedItem};
pub use payroll_comparison::{compare_runs, ComponentChange, PersonChange, RunComparison, RunTotals};
pub use promotion_salary_handler::PromotionSalaryHandler;
pub use offboarding_settlement_handler::OffboardingSettlementHandler;
pub use onboarding_enrolled_handler::{OnboardingEnrolledHandler, OnboardingEnrollInputs, PoolOnboardingEnrollInputs};
pub use hire_compensation_handler::{HireCompensationHandler, HiredEmployeeResolver};
