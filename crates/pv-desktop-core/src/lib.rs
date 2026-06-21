use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::Path;

use anyhow::{Context, Result, bail};
use pv_core::simulation::{
    BuiltInLoadShapeId, LoadProfile, LoadShape, ProductionProfile, SimulationOptions,
    SimulationResult,
};
use pv_core::source_model::SourceEnsembleEstimateDocument;
use pv_model_runtime::{EstimateArray, EstimateRequest};
use serde::{Deserialize, Serialize};

pub const PROJECT_SCHEMA_VERSION: u32 = 1;
pub const PROJECT_APPLICATION: &str = "pv-estimator";
pub const PROJECT_KIND: &str = "pv_project";
pub const PROJECT_EXTENSION: &str = "pvproj";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PvProjectDocument {
    pub schema_version: u32,
    pub application: String,
    pub kind: String,
    pub metadata: ProjectMetadata,
    pub inputs: ProjectInputs,
    pub results: ProjectResults,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectMetadata {
    pub title: String,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectInputs {
    pub estimate_request: EstimateRequest,
    pub arrays: Vec<EstimateArray>,
    pub load_profile: LoadProfile,
    #[serde(default)]
    pub energy_price_eur_per_kwh: Option<f64>,
    pub simulation_options: SimulationOptions,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectResults {
    pub estimate: Option<SourceEnsembleEstimateDocument>,
    pub production_profile: Option<ProductionProfile>,
    pub simulation: Option<SimulationResult>,
}

impl Default for PvProjectDocument {
    fn default() -> Self {
        let request = EstimateRequest {
            latitude: 45.4642,
            longitude: 9.19,
            location_id: "custom".to_string(),
            name: "Milan".to_string(),
            region: "IT".to_string(),
            peak_power_kwp: 4.0,
            loss_pct: 14.0,
            tilt_deg: 30.0,
            azimuth_deg: 0.0,
            storage_usable_kwh: Some(5.0),
        };
        Self {
            schema_version: PROJECT_SCHEMA_VERSION,
            application: PROJECT_APPLICATION.to_string(),
            kind: PROJECT_KIND.to_string(),
            metadata: ProjectMetadata {
                title: "Untitled PV Project".to_string(),
                created_at: None,
                updated_at: None,
            },
            inputs: ProjectInputs {
                arrays: vec![EstimateArray {
                    name: Some("Main array".to_string()),
                    peak_power_kwp: request.peak_power_kwp,
                    tilt_deg: request.tilt_deg,
                    azimuth_deg: request.azimuth_deg,
                }],
                estimate_request: request,
                load_profile: LoadProfile::AnnualKwh {
                    annual_kwh: 4200.0,
                    shape: LoadShape::BuiltIn {
                        shape_id: BuiltInLoadShapeId::ResidentialDefault,
                    },
                },
                energy_price_eur_per_kwh: None,
                simulation_options: SimulationOptions {
                    runs: 10_000,
                    seed: None,
                },
            },
            results: ProjectResults::default(),
        }
    }
}

impl PvProjectDocument {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != PROJECT_SCHEMA_VERSION {
            bail!(
                "unsupported pv project schema_version={}",
                self.schema_version
            );
        }
        if self.application != PROJECT_APPLICATION {
            bail!("unsupported pv project application={}", self.application);
        }
        if self.kind != PROJECT_KIND {
            bail!("unsupported pv project kind={}", self.kind);
        }
        if self.inputs.arrays.is_empty() {
            bail!("pv project requires at least one production array");
        }
        Ok(())
    }

    pub fn title_for_window(&self) -> String {
        if self.metadata.title.trim().is_empty() {
            "Untitled PV Project".to_string()
        } else {
            self.metadata.title.clone()
        }
    }
}

pub fn load_project(path: impl AsRef<Path>) -> Result<PvProjectDocument> {
    let path = path.as_ref();
    let reader = BufReader::new(
        File::open(path).with_context(|| format!("opening PV project {}", path.display()))?,
    );
    let project: PvProjectDocument = serde_json::from_reader(reader)
        .with_context(|| format!("parsing PV project {}", path.display()))?;
    project.validate()?;
    Ok(project)
}

pub fn save_project(path: impl AsRef<Path>, project: &PvProjectDocument) -> Result<()> {
    let path = path.as_ref();
    project.validate()?;
    let writer = BufWriter::new(
        File::create(path).with_context(|| format!("creating PV project {}", path.display()))?,
    );
    serde_json::to_writer_pretty(writer, project)
        .with_context(|| format!("writing PV project {}", path.display()))
}

pub fn project_file_display_name(path: Option<&Path>, project: &PvProjectDocument) -> String {
    path.and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| project.title_for_window())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_project_is_valid_schema_v1() {
        let project = PvProjectDocument::default();
        project.validate().expect("default project is valid");
        assert_eq!(project.schema_version, 1);
        assert_eq!(project.application, "pv-estimator");
        assert_eq!(project.kind, "pv_project");
        assert_eq!(project.inputs.arrays.len(), 1);
    }

    #[test]
    fn project_json_round_trips() {
        let project = PvProjectDocument::default();
        let json = serde_json::to_string_pretty(&project).expect("serialize project");
        let restored: PvProjectDocument = serde_json::from_str(&json).expect("deserialize project");
        assert_eq!(restored, project);
        restored.validate().expect("restored project is valid");
    }

    #[test]
    fn project_inputs_default_missing_energy_price() {
        let project = PvProjectDocument::default();
        let mut json = serde_json::to_value(&project).expect("serialize project");
        json["inputs"]
            .as_object_mut()
            .expect("inputs is an object")
            .remove("energy_price_eur_per_kwh");
        let restored: PvProjectDocument =
            serde_json::from_value(json).expect("deserialize legacy project");
        assert_eq!(restored.inputs.energy_price_eur_per_kwh, None);
        restored.validate().expect("restored project is valid");
    }

    #[test]
    fn project_save_load_preserves_simulation_runs() {
        let mut project = PvProjectDocument::default();
        project.inputs.simulation_options.runs = 1_000_000;
        let path =
            std::env::temp_dir().join(format!("pv-estimator-runs-{}.pvproj", std::process::id()));

        save_project(&path, &project).expect("save project");
        let restored = load_project(&path).expect("load project");
        let _ = std::fs::remove_file(&path);

        assert_eq!(restored.inputs.simulation_options.runs, 1_000_000);
    }

    #[test]
    fn rejects_wrong_kind() {
        let project = PvProjectDocument {
            kind: "wrong".to_string(),
            ..PvProjectDocument::default()
        };
        assert!(project.validate().is_err());
    }
}
