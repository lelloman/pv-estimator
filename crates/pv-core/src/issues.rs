use crate::ids::ComponentId;
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IssueCode(String);

impl IssueCode {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IssueSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    code: IssueCode,
    severity: IssueSeverity,
    affected_components: Vec<ComponentId>,
    parameters: BTreeMap<String, String>,
}

impl Issue {
    pub fn new(code: IssueCode, severity: IssueSeverity) -> Self {
        Self {
            code,
            severity,
            affected_components: Vec::new(),
            parameters: BTreeMap::new(),
        }
    }

    pub fn code(&self) -> &IssueCode {
        &self.code
    }

    pub fn severity(&self) -> IssueSeverity {
        self.severity
    }

    pub fn affected_components(&self) -> &[ComponentId] {
        &self.affected_components
    }

    pub fn parameters(&self) -> &BTreeMap<String, String> {
        &self.parameters
    }

    pub fn with_component(mut self, component_id: ComponentId) -> Self {
        self.affected_components.push(component_id);
        self
    }

    pub fn with_parameter(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.parameters.insert(key.into(), value.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_carries_machine_readable_data() {
        let issue = Issue::new(
            IssueCode::new("string_voltage_exceeds_limit"),
            IssueSeverity::Error,
        )
        .with_component(ComponentId::new("string-1").expect("valid id"))
        .with_parameter("limit_v", "1000");

        assert_eq!(issue.code().as_str(), "string_voltage_exceeds_limit");
        assert_eq!(issue.severity(), IssueSeverity::Error);
        assert_eq!(issue.affected_components()[0].as_str(), "string-1");
        assert_eq!(issue.parameters().get("limit_v"), Some(&"1000".to_string()));
    }
}
