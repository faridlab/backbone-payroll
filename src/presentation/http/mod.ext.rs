// This directory's extension (hand-written; ADR-0031). The generated mod.rs (lib.rs for
// src/) pulls it in with include!, so its lines resolve against this directory.

// Guarded composition (user-owned file): validated run verbs + computed slips + safe reads, no
// generic run/slip/slip-line mutation.
pub mod guarded_routes;
pub use guarded_routes::create_guarded_payroll_routes;
