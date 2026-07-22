use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use rust_decimal::Decimal;

use super::PayrollStatus;
use super::AuditMetadata;

/// Strongly-typed ID for PayrollEntry
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PayrollEntryId(pub Uuid);

impl PayrollEntryId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for PayrollEntryId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for PayrollEntryId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for PayrollEntryId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<PayrollEntryId> for Uuid {
    fn from(id: PayrollEntryId) -> Self { id.0 }
}

impl AsRef<Uuid> for PayrollEntryId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for PayrollEntryId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct PayrollEntry {
    pub id: Uuid,
    pub company_id: Uuid,
    pub period_year: i32,
    pub period_month: i32,
    pub posting_date: Option<DateTime<Utc>>,
    pub status: PayrollStatus,
    pub salary_expense_account_id: Option<Uuid>,
    pub salary_payable_account_id: Option<Uuid>,
    pub total_gross: Decimal,
    pub total_deductions: Decimal,
    pub total_net: Decimal,
    pub journal_id: Option<Uuid>,
    pub accounting_post_id: Option<Uuid>,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl PayrollEntry {
    /// Create a builder for PayrollEntry
    pub fn builder() -> PayrollEntryBuilder {
        PayrollEntryBuilder::default()
    }

    /// Create a new PayrollEntry with required fields
    pub fn new(company_id: Uuid, period_year: i32, period_month: i32, status: PayrollStatus, total_gross: Decimal, total_deductions: Decimal, total_net: Decimal) -> Self {
        Self {
            id: Uuid::new_v4(),
            company_id,
            period_year,
            period_month,
            posting_date: None,
            status,
            salary_expense_account_id: None,
            salary_payable_account_id: None,
            total_gross,
            total_deductions,
            total_net,
            journal_id: None,
            accounting_post_id: None,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> PayrollEntryId {
        PayrollEntryId(self.id)
    }

    /// Get when this entity was created
    pub fn created_at(&self) -> Option<&DateTime<Utc>> {
        self.metadata.created_at.as_ref()
    }

    /// Get when this entity was last updated
    pub fn updated_at(&self) -> Option<&DateTime<Utc>> {
        self.metadata.updated_at.as_ref()
    }

    /// Check if this entity is soft deleted
    pub fn is_deleted(&self) -> bool {
        self.metadata.deleted_at.is_some()
    }

    /// Check if this entity is active (not deleted)
    pub fn is_active(&self) -> bool {
        self.metadata.deleted_at.is_none()
    }

    /// Get when this entity was deleted
    pub fn deleted_at(&self) -> Option<&DateTime<Utc>> {
        self.metadata.deleted_at.as_ref()
    }

    /// Get who created this entity
    pub fn created_by(&self) -> Option<&Uuid> {
        self.metadata.created_by.as_ref()
    }

    /// Get who last updated this entity
    pub fn updated_by(&self) -> Option<&Uuid> {
        self.metadata.updated_by.as_ref()
    }

    /// Get who deleted this entity
    pub fn deleted_by(&self) -> Option<&Uuid> {
        self.metadata.deleted_by.as_ref()
    }

    /// Get the current status
    pub fn status(&self) -> &PayrollStatus {
        &self.status
    }


    // ==========================================================
    // Fluent Setters (with_* for optional fields)
    // ==========================================================

    /// Set the posting_date field (chainable)
    pub fn with_posting_date(mut self, value: DateTime<Utc>) -> Self {
        self.posting_date = Some(value);
        self
    }

    /// Set the salary_expense_account_id field (chainable)
    pub fn with_salary_expense_account_id(mut self, value: Uuid) -> Self {
        self.salary_expense_account_id = Some(value);
        self
    }

    /// Set the salary_payable_account_id field (chainable)
    pub fn with_salary_payable_account_id(mut self, value: Uuid) -> Self {
        self.salary_payable_account_id = Some(value);
        self
    }

    /// Set the journal_id field (chainable)
    pub fn with_journal_id(mut self, value: Uuid) -> Self {
        self.journal_id = Some(value);
        self
    }

    /// Set the accounting_post_id field (chainable)
    pub fn with_accounting_post_id(mut self, value: Uuid) -> Self {
        self.accounting_post_id = Some(value);
        self
    }

    // ==========================================================
    // Partial Update
    // ==========================================================

    /// Apply partial updates from a map of field name to JSON value
    pub fn apply_patch(&mut self, fields: std::collections::HashMap<String, serde_json::Value>) {
        for (key, value) in fields {
            match key.as_str() {
                "company_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.company_id = v; }
                }
                "period_year" => {
                    if let Ok(v) = serde_json::from_value(value) { self.period_year = v; }
                }
                "period_month" => {
                    if let Ok(v) = serde_json::from_value(value) { self.period_month = v; }
                }
                "posting_date" => {
                    if let Ok(v) = serde_json::from_value(value) { self.posting_date = v; }
                }
                "status" => {
                    if let Ok(v) = serde_json::from_value(value) { self.status = v; }
                }
                "salary_expense_account_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.salary_expense_account_id = v; }
                }
                "salary_payable_account_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.salary_payable_account_id = v; }
                }
                "total_gross" => {
                    if let Ok(v) = serde_json::from_value(value) { self.total_gross = v; }
                }
                "total_deductions" => {
                    if let Ok(v) = serde_json::from_value(value) { self.total_deductions = v; }
                }
                "total_net" => {
                    if let Ok(v) = serde_json::from_value(value) { self.total_net = v; }
                }
                "journal_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.journal_id = v; }
                }
                "accounting_post_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.accounting_post_id = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for PayrollEntry {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "PayrollEntry"
    }
}

impl backbone_core::PersistentEntity for PayrollEntry {
    fn entity_id(&self) -> String {
        self.id.to_string()
    }
    fn set_entity_id(&mut self, id: String) {
        if let Ok(uuid) = uuid::Uuid::parse_str(&id) {
            self.id = uuid;
        }
    }
    fn created_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.metadata.created_at
    }
    fn set_created_at(&mut self, ts: chrono::DateTime<chrono::Utc>) {
        self.metadata.created_at = Some(ts);
    }
    fn updated_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.metadata.updated_at
    }
    fn set_updated_at(&mut self, ts: chrono::DateTime<chrono::Utc>) {
        self.metadata.updated_at = Some(ts);
    }
    fn deleted_at(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.metadata.deleted_at
    }
    fn set_deleted_at(&mut self, ts: Option<chrono::DateTime<chrono::Utc>>) {
        self.metadata.deleted_at = ts;
    }
}

impl backbone_orm::EntityRepoMeta for PayrollEntry {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("company_id".to_string(), "uuid".to_string());
        m.insert("salary_expense_account_id".to_string(), "uuid".to_string());
        m.insert("salary_payable_account_id".to_string(), "uuid".to_string());
        m.insert("journal_id".to_string(), "uuid".to_string());
        m.insert("accounting_post_id".to_string(), "uuid".to_string());
        m.insert("status".to_string(), "payroll_status".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &[]
    }
    fn company_field() -> Option<&'static str> {
        Some("company_id")
    }
}

/// Builder for PayrollEntry entity
///
/// Provides a fluent API for constructing PayrollEntry instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct PayrollEntryBuilder {
    company_id: Option<Uuid>,
    period_year: Option<i32>,
    period_month: Option<i32>,
    posting_date: Option<DateTime<Utc>>,
    status: Option<PayrollStatus>,
    salary_expense_account_id: Option<Uuid>,
    salary_payable_account_id: Option<Uuid>,
    total_gross: Option<Decimal>,
    total_deductions: Option<Decimal>,
    total_net: Option<Decimal>,
    journal_id: Option<Uuid>,
    accounting_post_id: Option<Uuid>,
}

impl PayrollEntryBuilder {
    /// Set the company_id field (required)
    pub fn company_id(mut self, value: Uuid) -> Self {
        self.company_id = Some(value);
        self
    }

    /// Set the period_year field (required)
    pub fn period_year(mut self, value: i32) -> Self {
        self.period_year = Some(value);
        self
    }

    /// Set the period_month field (required)
    pub fn period_month(mut self, value: i32) -> Self {
        self.period_month = Some(value);
        self
    }

    /// Set the posting_date field (optional)
    pub fn posting_date(mut self, value: DateTime<Utc>) -> Self {
        self.posting_date = Some(value);
        self
    }

    /// Set the status field (default: `PayrollStatus::default()`)
    pub fn status(mut self, value: PayrollStatus) -> Self {
        self.status = Some(value);
        self
    }

    /// Set the salary_expense_account_id field (optional)
    pub fn salary_expense_account_id(mut self, value: Uuid) -> Self {
        self.salary_expense_account_id = Some(value);
        self
    }

    /// Set the salary_payable_account_id field (optional)
    pub fn salary_payable_account_id(mut self, value: Uuid) -> Self {
        self.salary_payable_account_id = Some(value);
        self
    }

    /// Set the total_gross field (default: `Decimal::from(0)`)
    pub fn total_gross(mut self, value: Decimal) -> Self {
        self.total_gross = Some(value);
        self
    }

    /// Set the total_deductions field (default: `Decimal::from(0)`)
    pub fn total_deductions(mut self, value: Decimal) -> Self {
        self.total_deductions = Some(value);
        self
    }

    /// Set the total_net field (default: `Decimal::from(0)`)
    pub fn total_net(mut self, value: Decimal) -> Self {
        self.total_net = Some(value);
        self
    }

    /// Set the journal_id field (optional)
    pub fn journal_id(mut self, value: Uuid) -> Self {
        self.journal_id = Some(value);
        self
    }

    /// Set the accounting_post_id field (optional)
    pub fn accounting_post_id(mut self, value: Uuid) -> Self {
        self.accounting_post_id = Some(value);
        self
    }

    /// Build the PayrollEntry entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<PayrollEntry, String> {
        let company_id = self.company_id.ok_or_else(|| "company_id is required".to_string())?;
        let period_year = self.period_year.ok_or_else(|| "period_year is required".to_string())?;
        let period_month = self.period_month.ok_or_else(|| "period_month is required".to_string())?;

        Ok(PayrollEntry {
            id: Uuid::new_v4(),
            company_id,
            period_year,
            period_month,
            posting_date: self.posting_date,
            status: self.status.unwrap_or(PayrollStatus::default()),
            salary_expense_account_id: self.salary_expense_account_id,
            salary_payable_account_id: self.salary_payable_account_id,
            total_gross: self.total_gross.unwrap_or(Decimal::from(0)),
            total_deductions: self.total_deductions.unwrap_or(Decimal::from(0)),
            total_net: self.total_net.unwrap_or(Decimal::from(0)),
            journal_id: self.journal_id,
            accounting_post_id: self.accounting_post_id,
            metadata: AuditMetadata::default(),
        })
    }
}
