// This directory's extension (hand-written; ADR-0031). The generated mod.rs (lib.rs for
// src/) pulls it in with include!, so its lines resolve against this directory.

// The hand-owned request-pool shim (the composing service's tenant pool
// resolution): a generated-tree declaration the regenerator drops, so it
// lives in the preserved block (#447 cause-2 class).
pub mod request_pool;
// The validated run verbs (lifecycle + computed slips + the write-service request/outcome types),
// the fail-closed seams (GL post / remittance / domain events), the overtime + employee-statutory
// input ports, the statutory calculators, and the guarded HTTP composition. Re-exported so a
// host composes payroll without reaching into internal module paths.
pub use application::service::{
    bpjs_kesehatan, bpjs_ketenagakerjaan, compute_statutory, overtime_pay, pph21, pph21_ter, thr,
    AccountingPostEnvelope, ComputedSlipRequest, EmployeeStatutory, EmployeeStatutoryInputs,
    GlPostAck, GlPostLine, GlPostRejected, GlPostSink, LoggingSink, NewComponent, NewPayrollEntry,
    NewSalarySlip, NewStructure, OvertimeInputs, PayrollError, PayrollEvent, PayrollEventError,
    PayrollEventSink, PayrollPayable, PayrollPosted, PayrollWriteService, PoolEmployeeStatutoryInputs,
    PoolOvertimeInputs, PostOutcome, Pph21Method, RemitAck, RemitOutcome, RemittanceInstruction,
    RemittanceSeamError, RemittanceSink, StatutoryAccounts, StatutoryConfig, StatutoryError,
    UnwiredGlSink, UnwiredRemittance,
};
pub use presentation::http::create_guarded_payroll_routes;
