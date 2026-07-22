use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use rust_decimal::Decimal;
use super::AuditMetadata;

/// Strongly-typed ID for SalarySlip
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SalarySlipId(pub Uuid);

impl SalarySlipId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for SalarySlipId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for SalarySlipId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for SalarySlipId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<SalarySlipId> for Uuid {
    fn from(id: SalarySlipId) -> Self { id.0 }
}

impl AsRef<Uuid> for SalarySlipId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for SalarySlipId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct SalarySlip {
    pub id: Uuid,
    pub payroll_entry_id: Uuid,
    pub company_id: Uuid,
    pub employee_id: Uuid,
    pub structure_id: Option<Uuid>,
    pub working_days: Decimal,
    pub unpaid_days: Decimal,
    pub gross_pay: Decimal,
    pub total_deductions: Decimal,
    pub net_pay: Decimal,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl SalarySlip {
    /// Create a builder for SalarySlip
    pub fn builder() -> SalarySlipBuilder {
        SalarySlipBuilder::default()
    }

    /// Create a new SalarySlip with required fields
    pub fn new(payroll_entry_id: Uuid, company_id: Uuid, employee_id: Uuid, working_days: Decimal, unpaid_days: Decimal, gross_pay: Decimal, total_deductions: Decimal, net_pay: Decimal) -> Self {
        Self {
            id: Uuid::new_v4(),
            payroll_entry_id,
            company_id,
            employee_id,
            structure_id: None,
            working_days,
            unpaid_days,
            gross_pay,
            total_deductions,
            net_pay,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> SalarySlipId {
        SalarySlipId(self.id)
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


    // ==========================================================
    // Fluent Setters (with_* for optional fields)
    // ==========================================================

    /// Set the structure_id field (chainable)
    pub fn with_structure_id(mut self, value: Uuid) -> Self {
        self.structure_id = Some(value);
        self
    }

    // ==========================================================
    // Partial Update
    // ==========================================================

    /// Apply partial updates from a map of field name to JSON value
    pub fn apply_patch(&mut self, fields: std::collections::HashMap<String, serde_json::Value>) {
        for (key, value) in fields {
            match key.as_str() {
                "payroll_entry_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.payroll_entry_id = v; }
                }
                "company_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.company_id = v; }
                }
                "employee_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.employee_id = v; }
                }
                "structure_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.structure_id = v; }
                }
                "working_days" => {
                    if let Ok(v) = serde_json::from_value(value) { self.working_days = v; }
                }
                "unpaid_days" => {
                    if let Ok(v) = serde_json::from_value(value) { self.unpaid_days = v; }
                }
                "gross_pay" => {
                    if let Ok(v) = serde_json::from_value(value) { self.gross_pay = v; }
                }
                "total_deductions" => {
                    if let Ok(v) = serde_json::from_value(value) { self.total_deductions = v; }
                }
                "net_pay" => {
                    if let Ok(v) = serde_json::from_value(value) { self.net_pay = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for SalarySlip {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "SalarySlip"
    }
}

impl backbone_core::PersistentEntity for SalarySlip {
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

impl backbone_orm::EntityRepoMeta for SalarySlip {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("payroll_entry_id".to_string(), "uuid".to_string());
        m.insert("company_id".to_string(), "uuid".to_string());
        m.insert("employee_id".to_string(), "uuid".to_string());
        m.insert("structure_id".to_string(), "uuid".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &[]
    }
    fn company_field() -> Option<&'static str> {
        Some("company_id")
    }
}

/// Builder for SalarySlip entity
///
/// Provides a fluent API for constructing SalarySlip instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct SalarySlipBuilder {
    payroll_entry_id: Option<Uuid>,
    company_id: Option<Uuid>,
    employee_id: Option<Uuid>,
    structure_id: Option<Uuid>,
    working_days: Option<Decimal>,
    unpaid_days: Option<Decimal>,
    gross_pay: Option<Decimal>,
    total_deductions: Option<Decimal>,
    net_pay: Option<Decimal>,
}

impl SalarySlipBuilder {
    /// Set the payroll_entry_id field (required)
    pub fn payroll_entry_id(mut self, value: Uuid) -> Self {
        self.payroll_entry_id = Some(value);
        self
    }

    /// Set the company_id field (required)
    pub fn company_id(mut self, value: Uuid) -> Self {
        self.company_id = Some(value);
        self
    }

    /// Set the employee_id field (required)
    pub fn employee_id(mut self, value: Uuid) -> Self {
        self.employee_id = Some(value);
        self
    }

    /// Set the structure_id field (optional)
    pub fn structure_id(mut self, value: Uuid) -> Self {
        self.structure_id = Some(value);
        self
    }

    /// Set the working_days field (default: `Decimal::from(0)`)
    pub fn working_days(mut self, value: Decimal) -> Self {
        self.working_days = Some(value);
        self
    }

    /// Set the unpaid_days field (default: `Decimal::from(0)`)
    pub fn unpaid_days(mut self, value: Decimal) -> Self {
        self.unpaid_days = Some(value);
        self
    }

    /// Set the gross_pay field (default: `Decimal::from(0)`)
    pub fn gross_pay(mut self, value: Decimal) -> Self {
        self.gross_pay = Some(value);
        self
    }

    /// Set the total_deductions field (default: `Decimal::from(0)`)
    pub fn total_deductions(mut self, value: Decimal) -> Self {
        self.total_deductions = Some(value);
        self
    }

    /// Set the net_pay field (default: `Decimal::from(0)`)
    pub fn net_pay(mut self, value: Decimal) -> Self {
        self.net_pay = Some(value);
        self
    }

    /// Build the SalarySlip entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<SalarySlip, String> {
        let payroll_entry_id = self.payroll_entry_id.ok_or_else(|| "payroll_entry_id is required".to_string())?;
        let company_id = self.company_id.ok_or_else(|| "company_id is required".to_string())?;
        let employee_id = self.employee_id.ok_or_else(|| "employee_id is required".to_string())?;

        Ok(SalarySlip {
            id: Uuid::new_v4(),
            payroll_entry_id,
            company_id,
            employee_id,
            structure_id: self.structure_id,
            working_days: self.working_days.unwrap_or(Decimal::from(0)),
            unpaid_days: self.unpaid_days.unwrap_or(Decimal::from(0)),
            gross_pay: self.gross_pay.unwrap_or(Decimal::from(0)),
            total_deductions: self.total_deductions.unwrap_or(Decimal::from(0)),
            net_pay: self.net_pay.unwrap_or(Decimal::from(0)),
            metadata: AuditMetadata::default(),
        })
    }
}
