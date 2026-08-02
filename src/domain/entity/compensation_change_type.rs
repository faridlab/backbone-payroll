use serde::{Deserialize, Serialize};
use sqlx::Type;
use std::str::FromStr;
#[cfg(feature = "openapi")]
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
#[cfg_attr(feature = "openapi", derive(ToSchema))]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "compensation_change_type", rename_all = "snake_case")]
pub enum CompensationChangeType {
    Hire,
    Promotion,
    Transfer,
    Adjustment,
    Offboarding,
}

impl std::fmt::Display for CompensationChangeType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Hire => write!(f, "hire"),
            Self::Promotion => write!(f, "promotion"),
            Self::Transfer => write!(f, "transfer"),
            Self::Adjustment => write!(f, "adjustment"),
            Self::Offboarding => write!(f, "offboarding"),
        }
    }
}

impl FromStr for CompensationChangeType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "hire" => Ok(Self::Hire),
            "promotion" => Ok(Self::Promotion),
            "transfer" => Ok(Self::Transfer),
            "adjustment" => Ok(Self::Adjustment),
            "offboarding" => Ok(Self::Offboarding),
            _ => Err(format!("Unknown CompensationChangeType variant: {}", s)),
        }
    }
}

impl Default for CompensationChangeType {
    fn default() -> Self {
        Self::Hire
    }
}
