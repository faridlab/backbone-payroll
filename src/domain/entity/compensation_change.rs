use chrono::{DateTime, Utc, NaiveDate};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use rust_decimal::Decimal;

use super::CompensationChangeType;
use super::AuditMetadata;

/// Strongly-typed ID for CompensationChange
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CompensationChangeId(pub Uuid);

impl CompensationChangeId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for CompensationChangeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for CompensationChangeId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for CompensationChangeId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<CompensationChangeId> for Uuid {
    fn from(id: CompensationChangeId) -> Self { id.0 }
}

impl AsRef<Uuid> for CompensationChangeId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for CompensationChangeId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct CompensationChange {
    pub id: Uuid,
    pub company_id: Uuid,
    pub employee_id: Uuid,
    pub change_type: CompensationChangeType,
    pub new_amount: Option<Decimal>,
    pub effective_date: Option<NaiveDate>,
    pub reference_id: Option<Uuid>,
    pub note: Option<String>,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl CompensationChange {
    /// Create a builder for CompensationChange
    pub fn builder() -> CompensationChangeBuilder {
        CompensationChangeBuilder::default()
    }

    /// Create a new CompensationChange with required fields
    pub fn new(company_id: Uuid, employee_id: Uuid, change_type: CompensationChangeType) -> Self {
        Self {
            id: Uuid::new_v4(),
            company_id,
            employee_id,
            change_type,
            new_amount: None,
            effective_date: None,
            reference_id: None,
            note: None,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> CompensationChangeId {
        CompensationChangeId(self.id)
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

    /// Set the new_amount field (chainable)
    pub fn with_new_amount(mut self, value: Decimal) -> Self {
        self.new_amount = Some(value);
        self
    }

    /// Set the effective_date field (chainable)
    pub fn with_effective_date(mut self, value: NaiveDate) -> Self {
        self.effective_date = Some(value);
        self
    }

    /// Set the reference_id field (chainable)
    pub fn with_reference_id(mut self, value: Uuid) -> Self {
        self.reference_id = Some(value);
        self
    }

    /// Set the note field (chainable)
    pub fn with_note(mut self, value: String) -> Self {
        self.note = Some(value);
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
                "employee_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.employee_id = v; }
                }
                "change_type" => {
                    if let Ok(v) = serde_json::from_value(value) { self.change_type = v; }
                }
                "new_amount" => {
                    if let Ok(v) = serde_json::from_value(value) { self.new_amount = v; }
                }
                "effective_date" => {
                    if let Ok(v) = serde_json::from_value(value) { self.effective_date = v; }
                }
                "reference_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.reference_id = v; }
                }
                "note" => {
                    if let Ok(v) = serde_json::from_value(value) { self.note = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for CompensationChange {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "CompensationChange"
    }
}

impl backbone_core::PersistentEntity for CompensationChange {
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

impl backbone_orm::EntityRepoMeta for CompensationChange {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("company_id".to_string(), "uuid".to_string());
        m.insert("employee_id".to_string(), "uuid".to_string());
        m.insert("reference_id".to_string(), "uuid".to_string());
        m.insert("change_type".to_string(), "compensation_change_type".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &[]
    }
    fn company_field() -> Option<&'static str> {
        Some("company_id")
    }
}

/// Builder for CompensationChange entity
///
/// Provides a fluent API for constructing CompensationChange instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct CompensationChangeBuilder {
    company_id: Option<Uuid>,
    employee_id: Option<Uuid>,
    change_type: Option<CompensationChangeType>,
    new_amount: Option<Decimal>,
    effective_date: Option<NaiveDate>,
    reference_id: Option<Uuid>,
    note: Option<String>,
}

impl CompensationChangeBuilder {
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

    /// Set the change_type field (required)
    pub fn change_type(mut self, value: CompensationChangeType) -> Self {
        self.change_type = Some(value);
        self
    }

    /// Set the new_amount field (optional)
    pub fn new_amount(mut self, value: Decimal) -> Self {
        self.new_amount = Some(value);
        self
    }

    /// Set the effective_date field (optional)
    pub fn effective_date(mut self, value: NaiveDate) -> Self {
        self.effective_date = Some(value);
        self
    }

    /// Set the reference_id field (optional)
    pub fn reference_id(mut self, value: Uuid) -> Self {
        self.reference_id = Some(value);
        self
    }

    /// Set the note field (optional)
    pub fn note(mut self, value: String) -> Self {
        self.note = Some(value);
        self
    }

    /// Build the CompensationChange entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<CompensationChange, String> {
        let company_id = self.company_id.ok_or_else(|| "company_id is required".to_string())?;
        let employee_id = self.employee_id.ok_or_else(|| "employee_id is required".to_string())?;
        let change_type = self.change_type.ok_or_else(|| "change_type is required".to_string())?;

        Ok(CompensationChange {
            id: Uuid::new_v4(),
            company_id,
            employee_id,
            change_type,
            new_amount: self.new_amount,
            effective_date: self.effective_date,
            reference_id: self.reference_id,
            note: self.note,
            metadata: AuditMetadata::default(),
        })
    }
}
