//! Guarded route composition — the RECOMMENDED way to mount the payroll module.
//!
//! Hand-authored (user-owned; see `metaphor.codegen.yaml`). Closes the CRUD-bypass: the generated
//! 12-endpoint CRUD surface would let a well-formed request set a run's status or totals directly,
//! fabricate slip lines, or soft-delete a structure out from under live slips. Here:
//!
//! - **Entity reads**: GETs only (slips + slip lines are proof surfaces — what the verbs produced).
//! - **Structure/component CRUD**: master data — full generated CRUD (admin surface).
//! - **Writes**: every payroll mutation goes through [`PayrollWriteService`], which owns the run
//!   lifecycle (`draft → processed → posted`, at-most-once GL post), the slip roll-up, the balanced
//!   salary journal, and the fail-closed seams (GL / remittance default-unwired).
//!
//! The tenant comes from the [`CompanyContext`] the `company_auth` middleware inserts — never from
//! the body. `statutoryAccounts` and `riskClass` on the computed-slip body are stopgaps: the
//! accounting composition will resolve the payable accounts itself, and the HR master will carry
//! the BPJS JKK risk class, at which point both leave the request.
//!
//! **Fence posture** (ADR-0008): the generated read routes and structure CRUD carry no company
//! predicate in SQL — row visibility is the DB fence (strict RLS, `app.company_id` request
//! binding). Composers MUST mount this behind `company_auth` with the request-scoped DB binding
//! (the serpa posture), where a cross-tenant id simply matches zero rows. Every write verb's SQL
//! additionally rides the same request scope, so a cross-tenant run id 404s — pinned by
//! tests/integrity_probes.rs.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Json, Router,
};
use backbone_auth::company::CompanyContext;
use chrono::NaiveDate;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::application::service::{
    ComputedSlipRequest, NewComponent, NewPayrollEntry, NewStructure, PayrollError,
    PayrollWriteService, StatutoryAccounts,
};
use crate::presentation::http::{
    create_compensation_change_read_routes, create_payroll_entry_read_routes,
    create_salary_component_routes, create_salary_slip_line_read_routes,
    create_salary_slip_read_routes, create_salary_structure_routes,
};
use crate::PayrollModule;

#[derive(Debug, Serialize)]
struct ErrorBody {
    error: &'static str,
    message: String,
}

fn err_response(e: PayrollError) -> axum::response::Response {
    let status = StatusCode::from_u16(e.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (
        status,
        Json(ErrorBody { error: e.code(), message: e.to_string() }),
    )
        .into_response()
}

// ── request/response bodies ─────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateStructureBody {
    name: String,
    components: Vec<ComponentBody>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComponentBody {
    name: String,
    component_type: String, // "earning" | "deduction"
    amount: Decimal,
    gl_account_id: Uuid,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateRunBody {
    period_year: i32,
    period_month: i32,
    salary_expense_account_id: Uuid,
    salary_payable_account_id: Uuid,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComputedSlipBody {
    employee_id: Uuid,
    structure_id: Uuid,
    working_days: Decimal,
    #[serde(default)]
    unpaid_days: Decimal,
    /// BPJS JKK risk class 1..=5 — stopgap until the HR master carries the field.
    #[serde(default = "default_risk_class")]
    risk_class: i32,
    /// Payable accounts for statutory deductions — stopgap until the accounting composition
    /// resolves them itself.
    statutory_accounts: StatutoryAccountsBody,
}

fn default_risk_class() -> i32 {
    3
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StatutoryAccountsBody {
    pph21_payable_account_id: Uuid,
    bpjs_kesehatan_payable_account_id: Uuid,
    bpjs_ketenagakerjaan_payable_account_id: Uuid,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PostBody {
    #[serde(default = "default_posting_date")]
    posting_date: NaiveDate,
}

/// Default the posting date to today when the caller omits it.
fn default_posting_date() -> NaiveDate {
    chrono::Utc::now().date_naive()
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct IdBody {
    id: Uuid,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PostOutcomeBody {
    payroll_entry_id: Uuid,
    journal_id: Uuid,
    post_id: Uuid,
    total_net: Decimal,
    already: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemitAckBody {
    gl_account_id: Uuid,
    amount: Decimal,
    statutory: bool,
    idempotency_key: String,
    duplicate: bool,
    payment_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RemitOutcomeBody {
    payroll_entry_id: Uuid,
    remitted: Vec<RemitAckBody>,
}

// ── handlers ───────────────────────────────────────────────────────────────────

async fn create_structure(
    State(svc): State<Arc<PayrollWriteService>>,
    tenant: CompanyContext,
    Json(b): Json<CreateStructureBody>,
) -> axum::response::Response {
    match svc
        .create_structure(NewStructure {
            company_id: tenant.company_id,
            name: b.name,
            components: b
                .components
                .into_iter()
                .map(|c| NewComponent {
                    name: c.name,
                    component_type: c.component_type,
                    amount: c.amount,
                    gl_account_id: c.gl_account_id,
                })
                .collect(),
        })
        .await
    {
        Ok(id) => (StatusCode::CREATED, Json(IdBody { id })).into_response(),
        Err(e) => err_response(e),
    }
}

async fn create_run(
    State(svc): State<Arc<PayrollWriteService>>,
    tenant: CompanyContext,
    Json(b): Json<CreateRunBody>,
) -> axum::response::Response {
    match svc
        .create_payroll_entry(NewPayrollEntry {
            company_id: tenant.company_id,
            period_year: b.period_year,
            period_month: b.period_month,
            salary_expense_account_id: b.salary_expense_account_id,
            salary_payable_account_id: b.salary_payable_account_id,
        })
        .await
    {
        Ok(id) => (StatusCode::CREATED, Json(IdBody { id })).into_response(),
        Err(e) => err_response(e),
    }
}

async fn add_computed_slip(
    State(svc): State<Arc<PayrollWriteService>>,
    _tenant: CompanyContext,
    Path(run_id): Path<Uuid>,
    Json(b): Json<ComputedSlipBody>,
) -> axum::response::Response {
    match svc
        .add_computed_salary_slip(ComputedSlipRequest {
            run_id,
            employee_id: b.employee_id,
            structure_id: b.structure_id,
            working_days: b.working_days,
            unpaid_days: b.unpaid_days,
            risk_class: b.risk_class,
            accounts: StatutoryAccounts {
                pph21_payable: b.statutory_accounts.pph21_payable_account_id,
                bpjs_kesehatan_payable: b.statutory_accounts.bpjs_kesehatan_payable_account_id,
                bpjs_ketenagakerjaan_payable: b.statutory_accounts.bpjs_ketenagakerjaan_payable_account_id,
            },
        })
        .await
    {
        Ok(id) => (StatusCode::CREATED, Json(IdBody { id })).into_response(),
        Err(e) => err_response(e),
    }
}

async fn process_run(
    State(svc): State<Arc<PayrollWriteService>>,
    _tenant: CompanyContext,
    Path(run_id): Path<Uuid>,
) -> axum::response::Response {
    match svc.process_payroll_entry(run_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err_response(e),
    }
}

async fn post_run(
    State(svc): State<Arc<PayrollWriteService>>,
    _tenant: CompanyContext,
    Path(run_id): Path<Uuid>,
    body: Option<Json<PostBody>>,
) -> axum::response::Response {
    let posting_date = body.map(|Json(b)| b.posting_date).unwrap_or_else(default_posting_date);
    match svc.post_run(run_id, posting_date).await {
        Ok(o) => (
            StatusCode::OK,
            Json(PostOutcomeBody {
                payroll_entry_id: o.payroll_entry_id,
                journal_id: o.journal_id,
                post_id: o.post_id,
                total_net: o.total_net,
                already: o.already,
            }),
        )
            .into_response(),
        Err(e) => err_response(e),
    }
}

async fn remit_run(
    State(svc): State<Arc<PayrollWriteService>>,
    _tenant: CompanyContext,
    Path(run_id): Path<Uuid>,
) -> axum::response::Response {
    match svc.remit_run(run_id).await {
        Ok(o) => (
            StatusCode::OK,
            Json(RemitOutcomeBody {
                payroll_entry_id: o.payroll_entry_id,
                remitted: o
                    .remitted
                    .into_iter()
                    .map(|(i, a)| RemitAckBody {
                        gl_account_id: i.gl_account_id,
                        amount: i.amount,
                        statutory: i.statutory,
                        idempotency_key: i.idempotency_key,
                        duplicate: a.duplicate,
                        payment_id: a.payment_id,
                    })
                    .collect(),
            }),
        )
            .into_response(),
        Err(e) => err_response(e),
    }
}

// ── composition ────────────────────────────────────────────────────────────────

/// Build the guarded payroll router: entity reads + structure/component CRUD + the run verbs,
/// NO generic run/slip/slip-line mutation. Mount under the host's authenticated (`company_auth`)
/// tree with the request-scoped DB binding.
pub fn create_guarded_payroll_routes(m: &PayrollModule) -> Router {
    let writes = Router::new()
        .route("/salary-structures", post(create_structure))
        .route("/payroll-entries", post(create_run))
        .route("/payroll-entries/:id/slips", post(add_computed_slip))
        .route("/payroll-entries/:id/process", post(process_run))
        .route("/payroll-entries/:id/post", post(post_run))
        .route("/payroll-entries/:id/remit", post(remit_run))
        .with_state(m.payroll_write_service());

    // Reads: run/slip/slip-line/compensation GETs (proof surfaces). Structures + components mount
    // full CRUD (master data, the admin surface). Generic run/slip/slip-line WRITES are
    // deliberately absent — every mutation flows through the verbs above.
    Router::new()
        .merge(create_payroll_entry_read_routes(m.payroll_entry_service.clone()))
        .merge(create_salary_slip_read_routes(m.salary_slip_service.clone()))
        .merge(create_salary_slip_line_read_routes(m.salary_slip_line_service.clone()))
        .merge(create_compensation_change_read_routes(m.compensation_change_service.clone()))
        .merge(create_salary_structure_routes(m.salary_structure_service.clone()))
        .merge(create_salary_component_routes(m.salary_component_service.clone()))
        .merge(writes)
}
