use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdError {
    Empty,
    ContainsWhitespace,
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("id must not be empty"),
            Self::ContainsWhitespace => f.write_str("id must not contain whitespace"),
        }
    }
}

impl std::error::Error for IdError {}

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(try_from = "String", into = "String")]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, IdError> {
                let value = value.into();
                validate_id(&value)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<String> for $name {
            type Error = IdError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for String {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

id_type!(ProjectId);
id_type!(ComponentId);
id_type!(CatalogItemId);
id_type!(LocationId);
id_type!(WeatherSourceId);
id_type!(EndpointId);

fn validate_id(value: &str) -> Result<(), IdError> {
    if value.is_empty() {
        return Err(IdError::Empty);
    }

    if value.chars().any(char::is_whitespace) {
        return Err(IdError::ContainsWhitespace);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_strongly_typed() {
        let project_id = ProjectId::new("project-1").expect("valid id");
        let component_id = ComponentId::new("component-1").expect("valid id");

        assert_eq!(project_id.as_str(), "project-1");
        assert_eq!(component_id.as_str(), "component-1");
    }

    #[test]
    fn ids_reject_empty_values() {
        assert_eq!(ProjectId::new("").unwrap_err(), IdError::Empty);
    }

    #[test]
    fn ids_reject_whitespace() {
        assert_eq!(
            ComponentId::new("bad id").unwrap_err(),
            IdError::ContainsWhitespace
        );
    }

    #[test]
    fn ids_round_trip_through_json_strings() {
        let id = LocationId::new("rome").expect("valid id");
        let encoded = serde_json::to_string(&id).expect("serialize id");
        let decoded: LocationId = serde_json::from_str(&encoded).expect("deserialize id");

        assert_eq!(encoded, "\"rome\"");
        assert_eq!(decoded, id);
    }
}
