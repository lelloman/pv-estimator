use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, anyhow};
use ndarray::Array2;
use ort::session::Session;
use ort::value::TensorRef;
use pv_core::simulation::ProductionProfile;
use pv_core::source_model::SourceEnsembleEstimateDocument;

pub use pv_model_runtime::{
    EstimateArray, EstimateRequest, FinishedEstimate, PreparedEstimate, SourceInferenceJob,
    SourceInferenceOutput, SourceModelRuntime, days_in_month, format_table, short_month_name,
    validate_arrays, validate_request,
};

const EMBEDDED_NASA_POWER_ONNX: &[u8] =
    include_bytes!("../../../artifacts/source-models-768x8-int8/nasa_power.onnx");
const EMBEDDED_PVGIS_ERA5_ONNX: &[u8] =
    include_bytes!("../../../artifacts/source-models-768x8-int8/pvgis_era5.onnx");
const EMBEDDED_PVGIS_SARAH3_ONNX: &[u8] =
    include_bytes!("../../../artifacts/source-models-768x8-int8/pvgis_sarah3.onnx");

#[derive(Debug)]
pub struct SourceModelEstimator {
    runtime: SourceModelRuntime,
    sessions: HashMap<String, LoadedSession>,
}

#[derive(Debug)]
struct LoadedSession {
    session: Session,
}

impl SourceModelEstimator {
    pub fn load_embedded() -> Result<Self> {
        let runtime = SourceModelRuntime::load_embedded()?;
        let sessions = runtime
            .sources()
            .iter()
            .map(|source| {
                let bytes = embedded_onnx_bytes(&source.onnx_path)?;
                let session = Session::builder()
                    .context("creating ONNX Runtime session builder")?
                    .with_intra_threads(1)
                    .map_err(|error| anyhow!(error.to_string()))?
                    .with_inter_threads(1)
                    .map_err(|error| anyhow!(error.to_string()))?
                    .commit_from_memory(bytes)
                    .with_context(|| {
                        format!(
                            "loading embedded ONNX model {} for {}",
                            source.onnx_path.display(),
                            source.source_id
                        )
                    })?;
                Ok((source.source_id.clone(), LoadedSession { session }))
            })
            .collect::<Result<HashMap<_, _>>>()?;
        Ok(Self { runtime, sessions })
    }

    pub fn load(model_dir: impl AsRef<Path>, manifest_name: &str) -> Result<Self> {
        let runtime = SourceModelRuntime::load(model_dir, manifest_name)?;
        let sessions = runtime
            .sources()
            .iter()
            .map(|source| {
                let session = Session::builder()
                    .context("creating ONNX Runtime session builder")?
                    .with_intra_threads(1)
                    .map_err(|error| anyhow!(error.to_string()))?
                    .with_inter_threads(1)
                    .map_err(|error| anyhow!(error.to_string()))?
                    .commit_from_file(&source.onnx_path)
                    .with_context(|| {
                        format!("loading ONNX model {}", source.onnx_path.display())
                    })?;
                Ok((source.source_id.clone(), LoadedSession { session }))
            })
            .collect::<Result<HashMap<_, _>>>()?;
        Ok(Self { runtime, sessions })
    }

    pub fn estimate(
        &mut self,
        request: &EstimateRequest,
    ) -> Result<SourceEnsembleEstimateDocument> {
        self.estimate_arrays(request, &[request.single_array()])
    }

    pub fn production_profile(&mut self, request: &EstimateRequest) -> Result<ProductionProfile> {
        self.production_profile_arrays(request, &[request.single_array()])
    }

    pub fn production_profile_arrays(
        &mut self,
        request: &EstimateRequest,
        arrays: &[EstimateArray],
    ) -> Result<ProductionProfile> {
        let finished = self.estimate_arrays_with_profile(request, arrays)?;
        Ok(finished.production_profile)
    }

    pub fn estimate_arrays(
        &mut self,
        request: &EstimateRequest,
        arrays: &[EstimateArray],
    ) -> Result<SourceEnsembleEstimateDocument> {
        let finished = self.estimate_arrays_with_profile(request, arrays)?;
        Ok(finished.estimate)
    }

    pub fn estimate_arrays_with_profile(
        &mut self,
        request: &EstimateRequest,
        arrays: &[EstimateArray],
    ) -> Result<FinishedEstimate> {
        self.finish_prepared(self.runtime.prepare_estimate_arrays(request, arrays)?)
    }

    fn finish_prepared(&mut self, prepared: PreparedEstimate) -> Result<FinishedEstimate> {
        let source_outputs = self.run_prepared(&prepared)?;
        self.runtime.finish_estimate(&prepared, &source_outputs)
    }

    fn run_prepared(&mut self, prepared: &PreparedEstimate) -> Result<Vec<SourceInferenceOutput>> {
        prepared.jobs.iter().map(|job| self.run_job(job)).collect()
    }

    fn run_job(&mut self, job: &SourceInferenceJob) -> Result<SourceInferenceOutput> {
        let loaded = self
            .sessions
            .get_mut(&job.source_id)
            .ok_or_else(|| anyhow!("missing ONNX session for source {}", job.source_id))?;
        let features = Array2::from_shape_vec(
            (job.features_shape[0], job.features_shape[1]),
            job.features.clone(),
        )
        .with_context(|| format!("building feature matrix for source {}", job.source_id))?;
        let outputs = loaded
            .session
            .run(ort::inputs![job.input_name.as_str() => TensorRef::from_array_view(&features)?])
            .with_context(|| format!("running source model {} ({})", job.source_id, job.label))?;
        let output = outputs.get(&job.output_name).unwrap_or_else(|| &outputs[0]);
        let (shape, values) = output.try_extract_tensor::<f32>()?;
        Ok(SourceInferenceOutput {
            source_id: job.source_id.clone(),
            normalized_targets: values[..shape.num_elements()].to_vec(),
        })
    }
}

fn embedded_onnx_bytes(path: &Path) -> Result<&'static [u8]> {
    match path.to_str() {
        Some("nasa_power.onnx") => Ok(EMBEDDED_NASA_POWER_ONNX),
        Some("pvgis_era5.onnx") => Ok(EMBEDDED_PVGIS_ERA5_ONNX),
        Some("pvgis_sarah3.onnx") => Ok(EMBEDDED_PVGIS_SARAH3_ONNX),
        _ => anyhow::bail!(
            "embedded source-model artifact is missing {}",
            path.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combined_estimate_returns_the_matching_production_profile() {
        let request = EstimateRequest::default();
        let arrays = [request.single_array()];
        let mut estimator = SourceModelEstimator::load_embedded().expect("load embedded models");

        let finished = estimator
            .estimate_arrays_with_profile(&request, &arrays)
            .expect("estimate and profile");

        assert_eq!(finished.production_profile.hourly_mean_kwh.len(), 8760);
        assert_eq!(
            finished
                .estimate
                .ensemble_estimate
                .annual_energy
                .mean
                .as_kilowatt_hours(),
            finished.production_profile.annual_mean_kwh
        );
    }
}
