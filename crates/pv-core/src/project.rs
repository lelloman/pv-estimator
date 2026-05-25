use crate::ids::{CatalogItemId, ComponentId, EndpointId, LocationId, ProjectId, WeatherSourceId};
use crate::issues::{Issue, IssueCode, IssueSeverity};
use crate::units::{Angle, Length, TimeSpan};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::error::Error;
use std::fmt;

const CURRENT_SCHEMA_VERSION: u32 = 1;
const REQUIRED_FIELD_PATHS: &[&str] = &[
    "schema_version",
    "metadata",
    "metadata.project_id",
    "metadata.name",
    "location",
    "weather_source",
    "simulation_settings",
    "simulation_settings.timestep",
    "simulation_settings.operating_mode",
    "simulation_settings.baseline_loss_fraction",
    "custom_equipment",
    "components",
    "mounting_groups",
    "strings",
    "topology",
    "load_profile",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SchemaVersion(u32);

impl SchemaVersion {
    pub const fn current() -> Self {
        Self(CURRENT_SCHEMA_VERSION)
    }

    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PvSystemProject {
    pub schema_version: SchemaVersion,
    pub metadata: ProjectMetadata,
    pub location: SystemLocation,
    pub weather_source: WeatherSourceId,
    pub simulation_settings: SimulationSettings,
    pub custom_equipment: Vec<CustomEquipmentDefinition>,
    pub components: Vec<ComponentInstance>,
    pub mounting_groups: Vec<MountingGroup>,
    pub strings: Vec<StringDefinition>,
    pub topology: Vec<TopologyConnection>,
    pub load_profile: Option<LoadProfileReference>,
}

impl PvSystemProject {
    pub fn new(
        project_id: ProjectId,
        name: impl Into<String>,
        location: SystemLocation,
        weather_source: WeatherSourceId,
    ) -> Self {
        Self {
            schema_version: SchemaVersion::current(),
            metadata: ProjectMetadata {
                project_id,
                name: name.into(),
                description: None,
            },
            location,
            weather_source,
            simulation_settings: SimulationSettings::default(),
            custom_equipment: Vec::new(),
            components: Vec::new(),
            mounting_groups: Vec::new(),
            strings: Vec::new(),
            topology: Vec::new(),
            load_profile: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectMetadata {
    pub project_id: ProjectId,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SystemLocation {
    Embedded {
        location_id: LocationId,
    },
    Custom {
        name: String,
        latitude: Angle,
        longitude: Angle,
        elevation: Option<Length>,
        timezone: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimulationSettings {
    pub timestep: TimeSpan,
    pub operating_mode: OperatingMode,
    pub baseline_loss_fraction: f64,
}

impl Default for SimulationSettings {
    fn default() -> Self {
        Self {
            timestep: TimeSpan::from_hours(1.0),
            operating_mode: OperatingMode::GridTiedHybrid,
            baseline_loss_fraction: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperatingMode {
    GridTiedHybrid,
    GridTiedNoStorage,
    OffGrid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomEquipmentDefinition {
    pub custom_equipment_id: CatalogItemId,
    pub category: EquipmentCategory,
    pub manufacturer: Option<String>,
    pub model: String,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EquipmentCategory {
    Panel,
    Inverter,
    Battery,
    Bms,
    BlockingDiode,
    Cable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum EquipmentReference {
    Catalog { catalog_item_id: CatalogItemId },
    Custom { custom_equipment_id: CatalogItemId },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentInstance {
    pub component_id: ComponentId,
    pub name: String,
    pub kind: ComponentKind,
    pub equipment: EquipmentReference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ComponentKind {
    Panel,
    Inverter,
    Battery,
    Bms,
    BlockingDiode,
    Cable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MountingGroup {
    pub mounting_group_id: ComponentId,
    pub name: String,
    pub tilt: Angle,
    pub azimuth: Angle,
    pub albedo: Option<f64>,
    pub panel_ids: Vec<ComponentId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StringDefinition {
    pub string_id: ComponentId,
    pub name: String,
    pub panel_ids: Vec<ComponentId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TopologyConnection {
    pub from: EndpointId,
    pub to: EndpointId,
    pub role: ConnectionRole,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionRole {
    DcStringToMppt,
    DcBus,
    BatteryBus,
    AcOutput,
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LoadProfileReference {
    BuiltInTemplate {
        template_id: String,
    },
    CsvFile {
        path: String,
        energy_unit: EnergyUnit,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EnergyUnit {
    Wh,
    KWh,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectLoadError {
    InvalidJson(String),
    UnsupportedSchemaVersion { found: u32, supported: u32 },
    Validation(Vec<Issue>),
}

impl fmt::Display for ProjectLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson(message) => write!(f, "invalid project JSON: {message}"),
            Self::UnsupportedSchemaVersion { found, supported } => write!(
                f,
                "unsupported project schema version {found}; supported version is {supported}"
            ),
            Self::Validation(issues) => {
                write!(
                    f,
                    "project failed schema validation with {} issue(s)",
                    issues.len()
                )
            }
        }
    }
}

impl Error for ProjectLoadError {}

pub fn load_project_json(bytes: &[u8]) -> Result<PvSystemProject, ProjectLoadError> {
    let value: Value = serde_json::from_slice(bytes)
        .map_err(|error| ProjectLoadError::InvalidJson(error.to_string()))?;

    let missing_required_fields = missing_required_fields(&value);
    if !missing_required_fields.is_empty() {
        return Err(ProjectLoadError::Validation(missing_required_fields));
    }

    let schema_version = value
        .get("schema_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            ProjectLoadError::Validation(vec![missing_required_field_issue("schema_version")])
        })?;

    if schema_version > u64::from(CURRENT_SCHEMA_VERSION) {
        return Err(ProjectLoadError::UnsupportedSchemaVersion {
            found: schema_version as u32,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }

    let migrated = migrate_project_value(value, schema_version as u32)?;

    serde_json::from_value(migrated)
        .map_err(|error| ProjectLoadError::InvalidJson(error.to_string()))
}

pub fn save_project_json(project: &PvSystemProject) -> Result<Vec<u8>, ProjectLoadError> {
    serde_json::to_vec_pretty(project)
        .map_err(|error| ProjectLoadError::InvalidJson(error.to_string()))
}

fn migrate_project_value(value: Value, schema_version: u32) -> Result<Value, ProjectLoadError> {
    match schema_version {
        CURRENT_SCHEMA_VERSION => Ok(value),
        older => Err(ProjectLoadError::UnsupportedSchemaVersion {
            found: older,
            supported: CURRENT_SCHEMA_VERSION,
        }),
    }
}

fn missing_required_fields(value: &Value) -> Vec<Issue> {
    REQUIRED_FIELD_PATHS
        .iter()
        .filter(|field_path| !field_path_exists(value, field_path))
        .map(|field_path| missing_required_field_issue(field_path))
        .collect()
}

fn field_path_exists(value: &Value, field_path: &str) -> bool {
    let mut current = value;

    for segment in field_path.split('.') {
        let Some(next) = current.get(segment) else {
            return false;
        };

        current = next;
    }

    true
}

fn missing_required_field_issue(field: &str) -> Issue {
    Issue::new(
        IssueCode::new("project_schema.missing_required_field"),
        IssueSeverity::Error,
    )
    .with_parameter("field", field)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_project_round_trips_through_json() {
        let project = minimal_project();
        let json = save_project_json(&project).expect("serialize project");
        let decoded = load_project_json(&json).expect("deserialize project");

        assert_eq!(decoded, project);
    }

    #[test]
    fn representative_hybrid_project_round_trips_through_json() {
        let mut project = minimal_project();
        project.custom_equipment.push(CustomEquipmentDefinition {
            custom_equipment_id: CatalogItemId::new("custom.panel.1").expect("valid id"),
            category: EquipmentCategory::Panel,
            manufacturer: Some("Example Solar".to_string()),
            model: "Example 450 W".to_string(),
            notes: Some("phase 5 inline equipment fixture".to_string()),
        });
        project.components = vec![
            ComponentInstance {
                component_id: ComponentId::new("panel-1").expect("valid id"),
                name: "Panel 1".to_string(),
                kind: ComponentKind::Panel,
                equipment: EquipmentReference::Custom {
                    custom_equipment_id: CatalogItemId::new("custom.panel.1").expect("valid id"),
                },
            },
            ComponentInstance {
                component_id: ComponentId::new("inverter-1").expect("valid id"),
                name: "Hybrid inverter".to_string(),
                kind: ComponentKind::Inverter,
                equipment: EquipmentReference::Catalog {
                    catalog_item_id: CatalogItemId::new("catalog.inverter.1").expect("valid id"),
                },
            },
            ComponentInstance {
                component_id: ComponentId::new("battery-1").expect("valid id"),
                name: "Battery".to_string(),
                kind: ComponentKind::Battery,
                equipment: EquipmentReference::Catalog {
                    catalog_item_id: CatalogItemId::new("catalog.battery.1").expect("valid id"),
                },
            },
        ];
        project.mounting_groups.push(MountingGroup {
            mounting_group_id: ComponentId::new("roof-south").expect("valid id"),
            name: "South roof".to_string(),
            tilt: Angle::from_degrees(30.0),
            azimuth: Angle::from_degrees(180.0),
            albedo: Some(0.2),
            panel_ids: vec![ComponentId::new("panel-1").expect("valid id")],
        });
        project.strings.push(StringDefinition {
            string_id: ComponentId::new("string-1").expect("valid id"),
            name: "String 1".to_string(),
            panel_ids: vec![ComponentId::new("panel-1").expect("valid id")],
        });
        project.topology.push(TopologyConnection {
            from: EndpointId::new("string-1.positive").expect("valid id"),
            to: EndpointId::new("inverter-1.mppt-1").expect("valid id"),
            role: ConnectionRole::DcStringToMppt,
        });
        project.load_profile = Some(LoadProfileReference::BuiltInTemplate {
            template_id: "residential.default".to_string(),
        });

        let json = save_project_json(&project).expect("serialize project");
        let decoded = load_project_json(&json).expect("deserialize project");

        assert_eq!(decoded, project);
    }

    #[test]
    fn unsupported_future_schema_version_returns_clear_error() {
        let mut value = serde_json::to_value(minimal_project()).expect("project value");
        value["schema_version"] = Value::from(CURRENT_SCHEMA_VERSION + 1);
        let json = serde_json::to_vec(&value).expect("project json");

        let error = load_project_json(&json).expect_err("future schema must fail");

        assert_eq!(
            error,
            ProjectLoadError::UnsupportedSchemaVersion {
                found: CURRENT_SCHEMA_VERSION + 1,
                supported: CURRENT_SCHEMA_VERSION,
            }
        );
    }

    #[test]
    fn missing_required_fields_return_structured_validation_issues() {
        let json = br#"{"schema_version":1,"metadata":{}}"#;

        let error = load_project_json(json).expect_err("missing fields must fail");
        let ProjectLoadError::Validation(issues) = error else {
            panic!("expected validation error");
        };

        assert!(issues.iter().any(|issue| {
            issue.code().as_str() == "project_schema.missing_required_field"
                && issue.parameters().get("field") == Some(&"metadata.project_id".to_string())
        }));
    }

    fn minimal_project() -> PvSystemProject {
        PvSystemProject::new(
            ProjectId::new("project-1").expect("valid id"),
            "Minimal project",
            SystemLocation::Embedded {
                location_id: LocationId::new("it-rome").expect("valid id"),
            },
            WeatherSourceId::new("pvgis-tmy").expect("valid id"),
        )
    }
}
