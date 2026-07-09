use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;
use rust_decimal::Decimal;

use super::ComponentType;
use super::AuditMetadata;

/// Strongly-typed ID for SalarySlipLine
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SalarySlipLineId(pub Uuid);

impl SalarySlipLineId {
    pub fn new(id: Uuid) -> Self { Self(id) }
    pub fn generate() -> Self { Self(Uuid::new_v4()) }
    pub fn into_inner(self) -> Uuid { self.0 }
}

impl std::fmt::Display for SalarySlipLineId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for SalarySlipLineId {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<Uuid> for SalarySlipLineId {
    fn from(id: Uuid) -> Self { Self(id) }
}

impl From<SalarySlipLineId> for Uuid {
    fn from(id: SalarySlipLineId) -> Self { id.0 }
}

impl AsRef<Uuid> for SalarySlipLineId {
    fn as_ref(&self) -> &Uuid { &self.0 }
}

impl std::ops::Deref for SalarySlipLineId {
    type Target = Uuid;
    fn deref(&self) -> &Self::Target { &self.0 }
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct SalarySlipLine {
    pub id: Uuid,
    pub salary_slip_id: Uuid,
    pub name: String,
    pub component_type: ComponentType,
    pub is_statutory: bool,
    pub amount: Decimal,
    pub gl_account_id: Uuid,
    #[serde(default)]
    #[sqlx(json)]
    pub metadata: AuditMetadata,
}

impl SalarySlipLine {
    /// Create a builder for SalarySlipLine
    pub fn builder() -> SalarySlipLineBuilder {
        SalarySlipLineBuilder::default()
    }

    /// Create a new SalarySlipLine with required fields
    pub fn new(salary_slip_id: Uuid, name: String, component_type: ComponentType, is_statutory: bool, amount: Decimal, gl_account_id: Uuid) -> Self {
        Self {
            id: Uuid::new_v4(),
            salary_slip_id,
            name,
            component_type,
            is_statutory,
            amount,
            gl_account_id,
            metadata: AuditMetadata::default(),
        }
    }

    /// Get the entity's unique identifier
    pub fn id(&self) -> &Uuid {
        &self.id
    }

    /// Get a strongly-typed ID for this entity
    pub fn typed_id(&self) -> SalarySlipLineId {
        SalarySlipLineId(self.id)
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
    // Partial Update
    // ==========================================================

    /// Apply partial updates from a map of field name to JSON value
    pub fn apply_patch(&mut self, fields: std::collections::HashMap<String, serde_json::Value>) {
        for (key, value) in fields {
            match key.as_str() {
                "salary_slip_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.salary_slip_id = v; }
                }
                "name" => {
                    if let Ok(v) = serde_json::from_value(value) { self.name = v; }
                }
                "component_type" => {
                    if let Ok(v) = serde_json::from_value(value) { self.component_type = v; }
                }
                "is_statutory" => {
                    if let Ok(v) = serde_json::from_value(value) { self.is_statutory = v; }
                }
                "amount" => {
                    if let Ok(v) = serde_json::from_value(value) { self.amount = v; }
                }
                "gl_account_id" => {
                    if let Ok(v) = serde_json::from_value(value) { self.gl_account_id = v; }
                }
                _ => {} // ignore unknown fields
            }
        }
    }

    // <<< CUSTOM METHODS START >>>
    // <<< CUSTOM METHODS END >>>
}

impl super::Entity for SalarySlipLine {
    type Id = Uuid;

    fn entity_id(&self) -> &Self::Id {
        &self.id
    }

    fn entity_type() -> &'static str {
        "SalarySlipLine"
    }
}

impl backbone_core::PersistentEntity for SalarySlipLine {
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

impl backbone_orm::EntityRepoMeta for SalarySlipLine {
    fn column_types() -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        m.insert("id".to_string(), "uuid".to_string());
        m.insert("salary_slip_id".to_string(), "uuid".to_string());
        m.insert("gl_account_id".to_string(), "uuid".to_string());
        m.insert("component_type".to_string(), "component_type".to_string());
        m
    }
    fn search_fields() -> &'static [&'static str] {
        &["name"]
    }
}

/// Builder for SalarySlipLine entity
///
/// Provides a fluent API for constructing SalarySlipLine instances.
/// System fields (id, metadata, timestamps) are auto-initialized.
#[derive(Debug, Clone, Default)]
pub struct SalarySlipLineBuilder {
    salary_slip_id: Option<Uuid>,
    name: Option<String>,
    component_type: Option<ComponentType>,
    is_statutory: Option<bool>,
    amount: Option<Decimal>,
    gl_account_id: Option<Uuid>,
}

impl SalarySlipLineBuilder {
    /// Set the salary_slip_id field (required)
    pub fn salary_slip_id(mut self, value: Uuid) -> Self {
        self.salary_slip_id = Some(value);
        self
    }

    /// Set the name field (required)
    pub fn name(mut self, value: String) -> Self {
        self.name = Some(value);
        self
    }

    /// Set the component_type field (default: `ComponentType::default()`)
    pub fn component_type(mut self, value: ComponentType) -> Self {
        self.component_type = Some(value);
        self
    }

    /// Set the is_statutory field (default: `false`)
    pub fn is_statutory(mut self, value: bool) -> Self {
        self.is_statutory = Some(value);
        self
    }

    /// Set the amount field (default: `Decimal::from(0)`)
    pub fn amount(mut self, value: Decimal) -> Self {
        self.amount = Some(value);
        self
    }

    /// Set the gl_account_id field (required)
    pub fn gl_account_id(mut self, value: Uuid) -> Self {
        self.gl_account_id = Some(value);
        self
    }

    /// Build the SalarySlipLine entity
    ///
    /// Returns Err if any required field without a default is missing.
    pub fn build(self) -> Result<SalarySlipLine, String> {
        let salary_slip_id = self.salary_slip_id.ok_or_else(|| "salary_slip_id is required".to_string())?;
        let name = self.name.ok_or_else(|| "name is required".to_string())?;
        let gl_account_id = self.gl_account_id.ok_or_else(|| "gl_account_id is required".to_string())?;

        Ok(SalarySlipLine {
            id: Uuid::new_v4(),
            salary_slip_id,
            name,
            component_type: self.component_type.unwrap_or(ComponentType::default()),
            is_statutory: self.is_statutory.unwrap_or(false),
            amount: self.amount.unwrap_or(Decimal::from(0)),
            gl_account_id,
            metadata: AuditMetadata::default(),
        })
    }
}
