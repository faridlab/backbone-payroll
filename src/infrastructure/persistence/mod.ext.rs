// This directory's extension (hand-written; ADR-0031). The generated mod.rs (lib.rs for
// src/) pulls it in with include!, so its lines resolve against this directory.

mod statutory_params_repository;
// The hand-written payroll SQL's parameter/projection types (see the five `*_repository` modules, all
// declared `user_owned` in metaphor.codegen.yaml).
pub use payroll_entry_repository::{NewPayrollEntryRow, RunPeriodRow, RunPostingRow, RunStateRow};
pub use salary_component_repository::{ComponentRow, NewComponentRow};
pub use salary_slip_line_repository::{DeductionGroupRow, NewSlipLineRow};
pub use salary_slip_repository::{NewSalarySlipRow, SlipTotalsRow};
pub use salary_structure_repository::NewStructureRow;
pub use statutory_params_repository::StatutoryParamsRepository;
