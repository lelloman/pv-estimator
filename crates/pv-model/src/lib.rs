use std::f64::consts::PI;
use std::fmt::Write as _;
use std::fs::File;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use ndarray::Array2;
use ort::session::Session;
use ort::value::TensorRef;
use pv_core::ids::WeatherSourceId;
use pv_core::source_model::{
    AnnualPvEnsembleEstimate, EstimateCoverage, EstimateLocation, EstimateSystem, Irradiation,
    MonthOfYear, SourceAnnualPvEstimate, SourceEnsembleEstimateDocument, SourceMonthlyPvEstimate,
};
use pv_core::units::Energy;
use serde::Deserialize;
use serde_json::json;

const BASE_INPUT_FEATURES: usize = 66;
const TARGETS: usize = 10;
const TEMPORAL_BINS: usize = 288;
const MONTH_DAYS: [f64; 12] = [
    31.0, 28.0, 31.0, 30.0, 31.0, 30.0, 31.0, 31.0, 30.0, 31.0, 30.0, 31.0,
];
const MID_MONTH_DOY: [f64; 12] = [
    15.0, 46.0, 74.0, 105.0, 135.0, 166.0, 196.0, 227.0, 258.0, 288.0, 319.0, 349.0,
];
const DEFAULT_UNCERTAINTY_MULTIPLIER: f64 = 2.0;
const SUPPORTED_MANIFEST_SCHEMA_V1: u32 = 1;
const SUPPORTED_MANIFEST_SCHEMA_V2: u32 = 2;
const EMBEDDED_SOURCE_MODEL_MANIFEST_JSON: &[u8] =
    include_bytes!("../../../artifacts/source-models-768x8-int8/source-model-artifacts.json");
const EMBEDDED_NASA_POWER_ONNX: &[u8] =
    include_bytes!("../../../artifacts/source-models-768x8-int8/nasa_power.onnx");
const EMBEDDED_PVGIS_ERA5_ONNX: &[u8] =
    include_bytes!("../../../artifacts/source-models-768x8-int8/pvgis_era5.onnx");
const EMBEDDED_PVGIS_SARAH3_ONNX: &[u8] =
    include_bytes!("../../../artifacts/source-models-768x8-int8/pvgis_sarah3.onnx");
const EMBEDDED_PVGIS_SARAH3_MASK_JSON: &[u8] = include_bytes!(
    "../../../artifacts/source-models-768x8-int8/coverage/pvgis_sarah3_empirical_grid_mask.json"
);

#[derive(Debug, Clone)]
pub struct EstimateRequest {
    pub latitude: f64,
    pub longitude: f64,
    pub location_id: String,
    pub name: String,
    pub region: String,
    pub peak_power_kwp: f64,
    pub loss_pct: f64,
    pub tilt_deg: f64,
    pub azimuth_deg: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EstimateArray {
    pub peak_power_kwp: f64,
    pub tilt_deg: f64,
    pub azimuth_deg: f64,
}

impl Default for EstimateRequest {
    fn default() -> Self {
        Self {
            latitude: 40.65,
            longitude: 15.643,
            location_id: "custom".to_string(),
            name: "Custom location".to_string(),
            region: String::new(),
            peak_power_kwp: 1.0,
            loss_pct: 14.0,
            tilt_deg: 30.0,
            azimuth_deg: 0.0,
        }
    }
}

impl EstimateRequest {
    pub fn single_array(&self) -> EstimateArray {
        EstimateArray {
            peak_power_kwp: self.peak_power_kwp,
            tilt_deg: self.tilt_deg,
            azimuth_deg: self.azimuth_deg,
        }
    }
}

#[derive(Debug)]
pub struct SourceModelEstimator {
    model_family: String,
    input_features: usize,
    temporal_bins: usize,
    target_names: Vec<String>,
    uncertainty_multiplier: f64,
    geography_features: Option<GeographyFeatureGrid>,
    sources: Vec<LoadedSource>,
}

#[derive(Debug, Deserialize)]
struct ArtifactManifest {
    schema_version: u32,
    model_family: String,
    input_features: usize,
    temporal_bins: usize,
    target_names: Vec<String>,
    #[serde(default = "default_uncertainty_multiplier")]
    uncertainty_multiplier: f64,
    sources: Vec<ArtifactSource>,
    geography_features: Option<ArtifactGeographyFeatures>,
}

#[derive(Debug, Deserialize)]
struct ArtifactGeographyFeatures {
    contract_path: PathBuf,
    grid_path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct GeographyFeatureContract {
    columns: Vec<String>,
    mean: Vec<f32>,
    std: Vec<f32>,
    #[serde(default)]
    clip_abs: Option<f32>,
}

#[derive(Debug, Clone)]
struct GeographyFeatureGrid {
    columns: Vec<String>,
    mean: Vec<f32>,
    std: Vec<f32>,
    clip_abs: Option<f32>,
    rows: Vec<GeographyFeatureRow>,
}

#[derive(Debug, Clone)]
struct GeographyFeatureRow {
    latitude: f64,
    longitude: f64,
    values: Vec<f32>,
}

#[derive(Debug, Deserialize)]
struct ArtifactSource {
    source_id: String,
    label: String,
    active: bool,
    onnx_path: PathBuf,
    #[serde(default = "default_input_name")]
    input_name: String,
    #[serde(default = "default_output_name")]
    output_name: String,
    coverage_rule: ArtifactCoverageRule,
    target_mean: Vec<f32>,
    target_std: Vec<f32>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ArtifactCoverageRule {
    Global,
    GlobalLandPvgisGateway,
    EmpiricalGridMask { mask_path: PathBuf },
}

#[derive(Debug, Deserialize)]
struct CoverageMask {
    rows: Vec<CoverageMaskRow>,
}

#[derive(Debug, Deserialize)]
struct CoverageMaskRow {
    lat_min: f64,
    lat_max: f64,
    lon_intervals: Vec<[f64; 2]>,
}

#[derive(Debug)]
struct LoadedSource {
    source_id: String,
    label: String,
    session: Session,
    input_name: String,
    output_name: String,
    coverage_rule: LoadedCoverageRule,
    target_mean: [f32; TARGETS],
    target_std: [f32; TARGETS],
}

#[derive(Debug)]
enum LoadedCoverageRule {
    Global,
    GlobalLandPvgisGateway,
    EmpiricalGridMask(CoverageMask),
}

#[derive(Debug, Clone)]
struct PvPrediction {
    annual_energy_kwh: f64,
    annual_poa_kwh_m2: f64,
    annual_ghi_kwh_m2: f64,
    monthly: Vec<MonthlyPvPrediction>,
}

#[derive(Debug, Clone)]
struct MonthlyPvPrediction {
    month: u8,
    energy_kwh: f64,
    poa_kwh_m2: f64,
    ghi_kwh_m2: f64,
}

impl SourceModelEstimator {
    pub fn load_embedded() -> Result<Self> {
        let manifest = load_manifest_bytes(
            EMBEDDED_SOURCE_MODEL_MANIFEST_JSON,
            "embedded source-model-artifacts.json",
        )?;
        let geography_features =
            load_embedded_geography_features(manifest.geography_features.as_ref())?;
        validate_manifest(&manifest, geography_features.as_ref())?;
        let sources = load_embedded_sources(manifest.sources)?;
        Ok(Self {
            model_family: manifest.model_family,
            input_features: manifest.input_features,
            temporal_bins: manifest.temporal_bins,
            target_names: manifest.target_names,
            uncertainty_multiplier: manifest.uncertainty_multiplier,
            geography_features,
            sources,
        })
    }

    pub fn load(model_dir: impl AsRef<Path>, manifest_name: &str) -> Result<Self> {
        let model_dir = model_dir.as_ref();
        let manifest_path = model_dir.join(manifest_name);
        let manifest = load_manifest(&manifest_path)?;
        let geography_features =
            load_geography_features(model_dir, manifest.geography_features.as_ref())?;
        validate_manifest(&manifest, geography_features.as_ref())?;
        let sources = load_sources(model_dir, manifest.sources)?;
        Ok(Self {
            model_family: manifest.model_family,
            input_features: manifest.input_features,
            temporal_bins: manifest.temporal_bins,
            target_names: manifest.target_names,
            uncertainty_multiplier: manifest.uncertainty_multiplier,
            geography_features,
            sources,
        })
    }

    pub fn estimate(
        &mut self,
        request: &EstimateRequest,
    ) -> Result<SourceEnsembleEstimateDocument> {
        self.estimate_arrays(request, &[request.single_array()])
    }

    pub fn estimate_arrays(
        &mut self,
        request: &EstimateRequest,
        arrays: &[EstimateArray],
    ) -> Result<SourceEnsembleEstimateDocument> {
        validate_request_location_and_loss(request)?;
        validate_arrays(arrays)?;
        let features = encode_features(
            request.latitude,
            request.longitude,
            self.input_features,
            self.geography_features.as_ref(),
        )?;
        let mut source_estimates = Vec::new();
        let mut applicable_sources = Vec::new();
        let mut sarah3_applicable = false;

        for source in &mut self.sources {
            let applies = source.applies(request.latitude, request.longitude);
            if source.source_id == "pvgis_sarah3" {
                sarah3_applicable = applies;
            }
            if !applies {
                continue;
            }
            let climate = source.predict_climate(&features)?;
            let pv = estimate_pv_from_climate(&climate, request, arrays);
            applicable_sources.push(WeatherSourceId::new(&source.source_id)?);
            source_estimates.push(source_estimate(&source.source_id, pv)?);
        }

        let ensemble = AnnualPvEnsembleEstimate::from_source_estimates_with_uncertainty(
            source_estimates,
            self.uncertainty_multiplier,
        )
        .ok_or_else(|| {
            anyhow!(
                "no applicable source models for {},{}",
                request.latitude,
                request.longitude
            )
        })?;

        Ok(SourceEnsembleEstimateDocument {
            schema_version: 1,
            location: EstimateLocation {
                location_id: request.location_id.clone(),
                name: request.name.clone(),
                region: request.region.clone(),
                latitude: request.latitude,
                longitude: request.longitude,
            },
            system: EstimateSystem {
                peak_power_kwp: total_peak_power_kwp(arrays),
                loss_pct: request.loss_pct,
                tilt_deg: weighted_array_value(arrays, |array| array.tilt_deg),
                aspect_deg: weighted_array_value(arrays, |array| array.azimuth_deg),
            },
            coverage: EstimateCoverage {
                pvgis_sarah3_applicable: sarah3_applicable,
                applicable_sources,
            },
            ensemble_estimate: ensemble,
            references: json!({
                "model_family": self.model_family,
                "input_features": self.input_features,
                "temporal_bins": self.temporal_bins,
                "target_names": self.target_names,
                "arrays": arrays.iter().map(|array| {
                    json!({
                        "peak_power_kwp": array.peak_power_kwp,
                        "tilt_deg": array.tilt_deg,
                        "azimuth_deg": array.azimuth_deg,
                    })
                }).collect::<Vec<_>>(),
            }),
        })
    }
}

pub fn days_in_month(month: u8) -> Option<f64> {
    month
        .checked_sub(1)
        .and_then(|index| MONTH_DAYS.get(index as usize))
        .copied()
}

pub fn short_month_name(month: u8) -> Option<&'static str> {
    const NAMES: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    month
        .checked_sub(1)
        .and_then(|index| NAMES.get(index as usize))
        .copied()
}

pub fn validate_request(request: &EstimateRequest) -> Result<()> {
    validate_request_location_and_loss(request)?;
    validate_array(request.single_array(), "system")
}

fn validate_request_location_and_loss(request: &EstimateRequest) -> Result<()> {
    if !(-90.0..=90.0).contains(&request.latitude) {
        bail!("latitude must be in [-90, 90]");
    }
    if !(-180.0..=180.0).contains(&request.longitude) {
        bail!("longitude must be in [-180, 180]");
    }
    if !(0.0..100.0).contains(&request.loss_pct) {
        bail!("loss percent must be in [0, 100)");
    }
    Ok(())
}

pub fn validate_arrays(arrays: &[EstimateArray]) -> Result<()> {
    if arrays.is_empty() {
        bail!("at least one array is required");
    }
    for (index, array) in arrays.iter().copied().enumerate() {
        validate_array(array, &format!("array {}", index + 1))?;
    }
    Ok(())
}

fn validate_array(array: EstimateArray, label: &str) -> Result<()> {
    if array.peak_power_kwp <= 0.0 {
        bail!("{label} peak power must be positive");
    }
    if !(0.0..=90.0).contains(&array.tilt_deg) {
        bail!("{label} tilt must be in [0, 90]");
    }
    Ok(())
}

fn total_peak_power_kwp(arrays: &[EstimateArray]) -> f64 {
    arrays.iter().map(|array| array.peak_power_kwp).sum()
}

fn weighted_array_value(arrays: &[EstimateArray], value: impl Fn(EstimateArray) -> f64) -> f64 {
    let total_kwp = total_peak_power_kwp(arrays);
    arrays
        .iter()
        .map(|array| array.peak_power_kwp * value(*array))
        .sum::<f64>()
        / total_kwp
}

pub fn format_table(document: &SourceEnsembleEstimateDocument) -> String {
    let estimate = &document.ensemble_estimate;
    let annual = estimate.annual_energy.mean.as_kilowatt_hours();
    let uncertainty = estimate
        .uncertainty
        .annual_energy
        .map(|band| {
            format!(
                "{:.2}..{:.2} kWh",
                band.low.as_kilowatt_hours(),
                band.high.as_kilowatt_hours()
            )
        })
        .unwrap_or_else(|| "insufficient sources".to_string());
    let mut output = String::new();
    writeln!(&mut output, "PV estimate for {}", document.location.name).expect("writing string");
    writeln!(
        &mut output,
        "location: {:.4}, {:.4}",
        document.location.latitude, document.location.longitude
    )
    .expect("writing string");
    writeln!(
        &mut output,
        "system: {:.2} kWp, loss {:.1}%, tilt {:.1} deg, azimuth {:.1} deg",
        document.system.peak_power_kwp,
        document.system.loss_pct,
        document.system.tilt_deg,
        document.system.aspect_deg
    )
    .expect("writing string");
    writeln!(
        &mut output,
        "sources: {}",
        document
            .coverage
            .applicable_sources
            .iter()
            .map(|source| source.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    )
    .expect("writing string");
    writeln!(&mut output, "annual energy: {:.2} kWh", annual).expect("writing string");
    writeln!(&mut output, "display band: {uncertainty}").expect("writing string");
    writeln!(
        &mut output,
        "annual in-plane irradiation: {:.2} kWh/m2",
        estimate
            .annual_in_plane_irradiation
            .mean
            .as_kilowatt_hours_per_square_meter()
    )
    .expect("writing string");
    writeln!(&mut output).expect("writing string");
    writeln!(
        &mut output,
        "{:<5} | {:^40} | {:^44}",
        "", "Monthly kWh", "Daily kWh"
    )
    .expect("writing string");
    writeln!(
        &mut output,
        "{:<5} | {:>10}  {:>13}  {:>13} | {:>15}  {:>17}  {:>17}",
        "Month", "mean", "min", "max", "mean", "min", "max"
    )
    .expect("writing string");
    for monthly in &estimate.monthly_estimates {
        let month = monthly.month.value();
        let days = days_in_month(month).expect("valid month has a day count");
        let month_name = short_month_name(month).expect("valid month has a short name");
        let total_kwh = monthly.energy.mean.as_kilowatt_hours();
        let (total_min, total_max, daily_min, daily_max) = monthly
            .uncertainty
            .annual_energy
            .map(|band| {
                let low = band.low.as_kilowatt_hours();
                let high = band.high.as_kilowatt_hours();
                (
                    format!("{low:.2}"),
                    format!("{high:.2}"),
                    format!("{:.2}", low / days),
                    format!("{:.2}", high / days),
                )
            })
            .unwrap_or_else(|| {
                (
                    "insufficient".to_string(),
                    "insufficient".to_string(),
                    "insufficient".to_string(),
                    "insufficient".to_string(),
                )
            });
        writeln!(
            &mut output,
            "{:<5} | {:>10.2}  {:>13}  {:>13} | {:>15.2}  {:>17}  {:>17}",
            month_name,
            total_kwh,
            total_min,
            total_max,
            total_kwh / days,
            daily_min,
            daily_max
        )
        .expect("writing string");
    }
    output
}

fn load_manifest(path: &Path) -> Result<ArtifactManifest> {
    let manifest: ArtifactManifest = serde_json::from_reader(
        File::open(path)
            .with_context(|| format!("opening artifact manifest {}", path.display()))?,
    )
    .with_context(|| format!("parsing artifact manifest {}", path.display()))?;
    Ok(manifest)
}

fn load_manifest_bytes(bytes: &[u8], label: &str) -> Result<ArtifactManifest> {
    serde_json::from_reader(Cursor::new(bytes))
        .with_context(|| format!("parsing artifact manifest {label}"))
}

fn validate_manifest(
    manifest: &ArtifactManifest,
    geography_features: Option<&GeographyFeatureGrid>,
) -> Result<()> {
    if !matches!(
        manifest.schema_version,
        SUPPORTED_MANIFEST_SCHEMA_V1 | SUPPORTED_MANIFEST_SCHEMA_V2
    ) {
        bail!(
            "unsupported artifact manifest schema_version={}",
            manifest.schema_version
        );
    }
    if manifest.temporal_bins != TEMPORAL_BINS {
        bail!(
            "artifact manifest temporal_bins={} but runtime expects {TEMPORAL_BINS}",
            manifest.temporal_bins
        );
    }
    if manifest.target_names.len() != TARGETS {
        bail!(
            "artifact manifest has {} targets but runtime expects {TARGETS}",
            manifest.target_names.len()
        );
    }
    match (manifest.schema_version, geography_features) {
        (SUPPORTED_MANIFEST_SCHEMA_V1, None) if manifest.input_features == BASE_INPUT_FEATURES => {}
        (SUPPORTED_MANIFEST_SCHEMA_V2, Some(grid))
            if manifest.input_features == BASE_INPUT_FEATURES + grid.width() => {}
        (SUPPORTED_MANIFEST_SCHEMA_V2, None) => {
            bail!("schema v2 artifact requires geography_features")
        }
        _ => bail!(
            "artifact manifest input_features={} is incompatible with schema_version={}",
            manifest.input_features,
            manifest.schema_version
        ),
    }
    Ok(())
}

fn load_geography_features(
    model_dir: &Path,
    spec: Option<&ArtifactGeographyFeatures>,
) -> Result<Option<GeographyFeatureGrid>> {
    let Some(spec) = spec else {
        return Ok(None);
    };
    let contract_path = model_dir.join(&spec.contract_path);
    let grid_path = model_dir.join(&spec.grid_path);
    let contract: GeographyFeatureContract =
        serde_json::from_reader(File::open(&contract_path).with_context(|| {
            format!(
                "opening geography feature contract {}",
                contract_path.display()
            )
        })?)
        .with_context(|| {
            format!(
                "parsing geography feature contract {}",
                contract_path.display()
            )
        })?;
    GeographyFeatureGrid::load(contract, &grid_path).map(Some)
}

fn load_embedded_geography_features(
    spec: Option<&ArtifactGeographyFeatures>,
) -> Result<Option<GeographyFeatureGrid>> {
    if spec.is_some() {
        bail!("embedded source-model artifacts do not include geography feature grids yet");
    }
    Ok(None)
}

impl GeographyFeatureGrid {
    fn load(contract: GeographyFeatureContract, grid_path: &Path) -> Result<Self> {
        if contract.columns.is_empty() {
            bail!("geography feature contract has no columns");
        }
        if contract.mean.len() != contract.columns.len()
            || contract.std.len() != contract.columns.len()
        {
            bail!("geography feature contract stats do not match column count");
        }
        let mut reader = csv::Reader::from_path(grid_path)
            .with_context(|| format!("opening geography feature grid {}", grid_path.display()))?;
        let headers = reader.headers()?.clone();
        let latitude_index = header_index(&headers, "latitude")?;
        let longitude_index = header_index(&headers, "longitude")?;
        let column_indexes = contract
            .columns
            .iter()
            .map(|column| header_index(&headers, column))
            .collect::<Result<Vec<_>>>()?;
        let mut rows = Vec::new();
        for record in reader.records() {
            let record = record.with_context(|| {
                format!("reading geography feature grid {}", grid_path.display())
            })?;
            let latitude = parse_csv_f64(&record, latitude_index, "latitude")?;
            let longitude = parse_csv_f64(&record, longitude_index, "longitude")?;
            let values = column_indexes
                .iter()
                .zip(contract.columns.iter())
                .map(|(index, column)| parse_csv_f32(&record, *index, column))
                .collect::<Result<Vec<_>>>()?;
            rows.push(GeographyFeatureRow {
                latitude,
                longitude,
                values,
            });
        }
        if rows.is_empty() {
            bail!("geography feature grid has no rows");
        }
        Ok(Self {
            columns: contract.columns,
            mean: contract.mean,
            std: contract.std,
            clip_abs: contract.clip_abs,
            rows,
        })
    }

    fn width(&self) -> usize {
        self.columns.len()
    }

    fn normalized_features(&self, lat: f64, lon: f64) -> Result<Vec<f32>> {
        let raw = self.interpolate_raw(lat, lon)?;
        Ok(raw
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let std = self.std[index].max(1.0e-6);
                let normalized = (*value - self.mean[index]) / std;
                self.clip_abs
                    .map(|clip| normalized.clamp(-clip, clip))
                    .unwrap_or(normalized)
            })
            .collect())
    }

    fn interpolate_raw(&self, lat: f64, lon: f64) -> Result<Vec<f32>> {
        let mut nearest: Vec<(f64, &GeographyFeatureRow)> = self
            .rows
            .iter()
            .map(|row| {
                (
                    coordinate_distance_sq(lat, lon, row.latitude, row.longitude),
                    row,
                )
            })
            .collect();
        nearest.sort_by(|left, right| left.0.total_cmp(&right.0));
        let nearest = nearest.into_iter().take(4).collect::<Vec<_>>();
        let Some((distance, row)) = nearest.first().copied() else {
            bail!("geography feature grid has no rows");
        };
        if distance <= 1.0e-12 {
            return Ok(row.values.clone());
        }
        if distance.sqrt() > 5.0 {
            bail!("no geography features near coordinate {lat:.4},{lon:.4}");
        }
        let mut output = vec![0.0f64; self.width()];
        let mut weight_sum = 0.0;
        for (distance, row) in nearest {
            let weight = 1.0 / distance.max(1.0e-12).sqrt();
            weight_sum += weight;
            for (index, value) in row.values.iter().enumerate() {
                output[index] += f64::from(*value) * weight;
            }
        }
        Ok(output
            .into_iter()
            .map(|value| (value / weight_sum) as f32)
            .collect())
    }
}

fn header_index(headers: &csv::StringRecord, column: &str) -> Result<usize> {
    headers
        .iter()
        .position(|candidate| candidate == column)
        .ok_or_else(|| anyhow!("geography feature grid missing column {column}"))
}

fn parse_csv_f64(record: &csv::StringRecord, index: usize, column: &str) -> Result<f64> {
    record
        .get(index)
        .ok_or_else(|| anyhow!("missing {column}"))?
        .parse::<f64>()
        .with_context(|| format!("parsing {column}"))
}

fn parse_csv_f32(record: &csv::StringRecord, index: usize, column: &str) -> Result<f32> {
    record
        .get(index)
        .ok_or_else(|| anyhow!("missing {column}"))?
        .parse::<f32>()
        .with_context(|| format!("parsing {column}"))
}

fn coordinate_distance_sq(lat: f64, lon: f64, row_lat: f64, row_lon: f64) -> f64 {
    let lat_scale = lat.to_radians().cos().abs().max(0.1);
    let d_lat = lat - row_lat;
    let d_lon = (lon - row_lon) * lat_scale;
    d_lat * d_lat + d_lon * d_lon
}

fn load_sources(model_dir: &Path, sources: Vec<ArtifactSource>) -> Result<Vec<LoadedSource>> {
    sources
        .into_iter()
        .filter(|source| source.active)
        .map(|source| load_source(model_dir, source))
        .collect()
}

fn load_embedded_sources(sources: Vec<ArtifactSource>) -> Result<Vec<LoadedSource>> {
    sources
        .into_iter()
        .filter(|source| source.active)
        .map(load_embedded_source)
        .collect()
}

fn load_source(model_dir: &Path, source: ArtifactSource) -> Result<LoadedSource> {
    if source.target_mean.len() != TARGETS || source.target_std.len() != TARGETS {
        bail!(
            "source {} target stats must contain {TARGETS} values",
            source.source_id
        );
    }
    let model_path = model_dir.join(&source.onnx_path);
    let session = Session::builder()
        .context("creating ONNX Runtime session builder")?
        .commit_from_file(&model_path)
        .with_context(|| format!("loading ONNX model {}", model_path.display()))?;
    let coverage_rule = match source.coverage_rule {
        ArtifactCoverageRule::Global => LoadedCoverageRule::Global,
        ArtifactCoverageRule::GlobalLandPvgisGateway => LoadedCoverageRule::GlobalLandPvgisGateway,
        ArtifactCoverageRule::EmpiricalGridMask { mask_path } => {
            LoadedCoverageRule::EmpiricalGridMask(load_mask(&model_dir.join(mask_path))?)
        }
    };
    Ok(LoadedSource {
        source_id: source.source_id,
        label: source.label,
        session,
        input_name: source.input_name,
        output_name: source.output_name,
        coverage_rule,
        target_mean: source
            .target_mean
            .try_into()
            .expect("target mean length checked"),
        target_std: source
            .target_std
            .try_into()
            .expect("target std length checked"),
    })
}

fn load_embedded_source(source: ArtifactSource) -> Result<LoadedSource> {
    if source.target_mean.len() != TARGETS || source.target_std.len() != TARGETS {
        bail!(
            "source {} target stats must contain {TARGETS} values",
            source.source_id
        );
    }
    let onnx_bytes = embedded_onnx_bytes(&source.onnx_path)?;
    let session = Session::builder()
        .context("creating ONNX Runtime session builder")?
        .commit_from_memory(onnx_bytes)
        .with_context(|| {
            format!(
                "loading embedded ONNX model {} for {}",
                source.onnx_path.display(),
                source.source_id
            )
        })?;
    let coverage_rule = match source.coverage_rule {
        ArtifactCoverageRule::Global => LoadedCoverageRule::Global,
        ArtifactCoverageRule::GlobalLandPvgisGateway => LoadedCoverageRule::GlobalLandPvgisGateway,
        ArtifactCoverageRule::EmpiricalGridMask { mask_path } => {
            LoadedCoverageRule::EmpiricalGridMask(load_embedded_mask(&mask_path)?)
        }
    };
    Ok(LoadedSource {
        source_id: source.source_id,
        label: source.label,
        session,
        input_name: source.input_name,
        output_name: source.output_name,
        coverage_rule,
        target_mean: source
            .target_mean
            .try_into()
            .expect("target mean length checked"),
        target_std: source
            .target_std
            .try_into()
            .expect("target std length checked"),
    })
}

fn embedded_onnx_bytes(path: &Path) -> Result<&'static [u8]> {
    match path.to_str() {
        Some("nasa_power.onnx") => Ok(EMBEDDED_NASA_POWER_ONNX),
        Some("pvgis_era5.onnx") => Ok(EMBEDDED_PVGIS_ERA5_ONNX),
        Some("pvgis_sarah3.onnx") => Ok(EMBEDDED_PVGIS_SARAH3_ONNX),
        _ => bail!(
            "embedded source-model artifact is missing {}",
            path.display()
        ),
    }
}

fn load_mask(path: &Path) -> Result<CoverageMask> {
    serde_json::from_reader(
        File::open(path).with_context(|| format!("opening coverage mask {}", path.display()))?,
    )
    .with_context(|| format!("parsing coverage mask {}", path.display()))
}

fn load_embedded_mask(path: &Path) -> Result<CoverageMask> {
    let bytes = match path.to_str() {
        Some("coverage/pvgis_sarah3_empirical_grid_mask.json") => EMBEDDED_PVGIS_SARAH3_MASK_JSON,
        _ => bail!("embedded coverage mask is missing {}", path.display()),
    };
    serde_json::from_reader(Cursor::new(bytes))
        .with_context(|| format!("parsing embedded coverage mask {}", path.display()))
}

impl LoadedSource {
    fn applies(&self, lat: f64, lon: f64) -> bool {
        match &self.coverage_rule {
            LoadedCoverageRule::Global | LoadedCoverageRule::GlobalLandPvgisGateway => true,
            LoadedCoverageRule::EmpiricalGridMask(mask) => mask.contains(lat, lon),
        }
    }

    fn predict_climate(
        &mut self,
        features: &Array2<f32>,
    ) -> Result<[[f64; TARGETS]; TEMPORAL_BINS]> {
        let outputs = self
            .session
            .run(ort::inputs![self.input_name.as_str() => TensorRef::from_array_view(features)?])
            .with_context(|| format!("running source model {} ({})", self.source_id, self.label))?;
        let output = outputs
            .get(&self.output_name)
            .unwrap_or_else(|| &outputs[0]);
        let (shape, values) = output.try_extract_tensor::<f32>()?;
        if shape.num_elements() != TEMPORAL_BINS * TARGETS {
            bail!(
                "source {} returned {} values, expected {}",
                self.source_id,
                shape.num_elements(),
                TEMPORAL_BINS * TARGETS
            );
        }
        let mut climate = [[0.0; TARGETS]; TEMPORAL_BINS];
        for row in 0..TEMPORAL_BINS {
            for target in 0..TARGETS {
                let normalized = values[row * TARGETS + target];
                let denormalized = normalized * self.target_std[target] + self.target_mean[target];
                climate[row][target] = f64::from(denormalized.max(0.0));
            }
        }
        Ok(climate)
    }
}

impl CoverageMask {
    fn contains(&self, lat: f64, lon: f64) -> bool {
        self.rows.iter().any(|row| {
            row.lat_min <= lat
                && lat <= row.lat_max
                && row
                    .lon_intervals
                    .iter()
                    .any(|[min, max]| *min <= lon && lon <= *max)
        })
    }
}

fn encode_features(
    lat: f64,
    lon: f64,
    input_features: usize,
    geography_features: Option<&GeographyFeatureGrid>,
) -> Result<Array2<f32>> {
    let geography = match geography_features {
        Some(grid) => grid.normalized_features(lat, lon)?,
        None => Vec::new(),
    };
    let mut output = Array2::<f32>::zeros((TEMPORAL_BINS, input_features));
    for temporal_index in 0..TEMPORAL_BINS {
        let lat_norm = lat / 90.0;
        let lon_norm = lon / 180.0;
        let phase = (temporal_index / 24) as f64 + 0.5;
        let hour = (temporal_index % 24) as f64;
        let season_angle = 2.0 * PI * phase / 12.0;
        let hour_angle = 2.0 * PI * hour / 24.0;
        let mut col = 0;
        output[[temporal_index, col]] = lat_norm as f32;
        col += 1;
        output[[temporal_index, col]] = lon_norm as f32;
        col += 1;
        for harmonic in 1..=8 {
            output[[temporal_index, col]] = (PI * lat_norm * harmonic as f64).sin() as f32;
            col += 1;
            output[[temporal_index, col]] = (PI * lat_norm * harmonic as f64).cos() as f32;
            col += 1;
        }
        for harmonic in 1..=8 {
            output[[temporal_index, col]] = (PI * lon_norm * harmonic as f64).sin() as f32;
            col += 1;
            output[[temporal_index, col]] = (PI * lon_norm * harmonic as f64).cos() as f32;
            col += 1;
        }
        for harmonic in 1..=6 {
            output[[temporal_index, col]] = (season_angle * harmonic as f64).sin() as f32;
            col += 1;
            output[[temporal_index, col]] = (season_angle * harmonic as f64).cos() as f32;
            col += 1;
        }
        for harmonic in 1..=6 {
            output[[temporal_index, col]] = (hour_angle * harmonic as f64).sin() as f32;
            col += 1;
            output[[temporal_index, col]] = (hour_angle * harmonic as f64).cos() as f32;
            col += 1;
        }
        let season_sin = season_angle.sin();
        let season_cos = season_angle.cos();
        let hour_sin = hour_angle.sin();
        let hour_cos = hour_angle.cos();
        for value in [
            lat_norm * season_sin,
            lat_norm * season_cos,
            lon_norm * season_sin,
            lon_norm * season_cos,
            season_sin * hour_sin,
            season_sin * hour_cos,
            season_cos * hour_sin,
            season_cos * hour_cos,
        ] {
            output[[temporal_index, col]] = value as f32;
            col += 1;
        }
        for value in &geography {
            output[[temporal_index, col]] = *value;
            col += 1;
        }
        debug_assert_eq!(col, input_features);
    }
    Ok(output)
}

fn estimate_pv_from_climate(
    climate: &[[f64; TARGETS]; TEMPORAL_BINS],
    request: &EstimateRequest,
    arrays: &[EstimateArray],
) -> PvPrediction {
    let total_kwp = total_peak_power_kwp(arrays);
    let predictions = arrays
        .iter()
        .map(|array| estimate_array_pv_from_climate(climate, request, *array))
        .collect::<Vec<_>>();

    let monthly = (0..12)
        .map(|month| {
            let month_predictions = predictions
                .iter()
                .map(|prediction| &prediction.monthly[month])
                .collect::<Vec<_>>();
            MonthlyPvPrediction {
                month: month_predictions[0].month,
                energy_kwh: month_predictions
                    .iter()
                    .map(|prediction| prediction.energy_kwh)
                    .sum(),
                poa_kwh_m2: arrays
                    .iter()
                    .zip(month_predictions.iter())
                    .map(|(array, prediction)| array.peak_power_kwp * prediction.poa_kwh_m2)
                    .sum::<f64>()
                    / total_kwp,
                ghi_kwh_m2: arrays
                    .iter()
                    .zip(month_predictions.iter())
                    .map(|(array, prediction)| array.peak_power_kwp * prediction.ghi_kwh_m2)
                    .sum::<f64>()
                    / total_kwp,
            }
        })
        .collect::<Vec<_>>();

    PvPrediction {
        annual_energy_kwh: predictions
            .iter()
            .map(|prediction| prediction.annual_energy_kwh)
            .sum(),
        annual_poa_kwh_m2: arrays
            .iter()
            .zip(predictions.iter())
            .map(|(array, prediction)| array.peak_power_kwp * prediction.annual_poa_kwh_m2)
            .sum::<f64>()
            / total_kwp,
        annual_ghi_kwh_m2: arrays
            .iter()
            .zip(predictions.iter())
            .map(|(array, prediction)| array.peak_power_kwp * prediction.annual_ghi_kwh_m2)
            .sum::<f64>()
            / total_kwp,
        monthly,
    }
}

fn estimate_array_pv_from_climate(
    climate: &[[f64; TARGETS]; TEMPORAL_BINS],
    request: &EstimateRequest,
    array: EstimateArray,
) -> PvPrediction {
    let lat_rad = request.latitude.to_radians();
    let tilt = array.tilt_deg.to_radians();
    let surface_azimuth_from_north = ((180.0 + array.azimuth_deg) % 360.0).to_radians();
    let loss_factor = 1.0 - request.loss_pct / 100.0;
    let albedo = 0.2;
    let gamma = -0.0040;
    let noct_c = 45.0;
    let mut monthly = Vec::with_capacity(12);
    let mut annual_poa_kwh_m2 = 0.0;
    let mut annual_energy_kwh = 0.0;
    let mut annual_ghi_kwh_m2 = 0.0;

    for month in 0..12 {
        let mut poa_day = 0.0;
        let mut energy_day = 0.0;
        let mut ghi_day = 0.0;
        for hour in 0..24 {
            let idx = month * 24 + hour;
            let ghi = climate[idx][0];
            let dni = climate[idx][1];
            let dhi = climate[idx][2];
            let temp = climate[idx][3];
            let (cosz, cosi) = solar_cosines(
                lat_rad,
                request.longitude,
                surface_azimuth_from_north,
                tilt,
                MID_MONTH_DOY[month],
                hour as f64 + 0.5,
            );
            let beam = if cosz > 0.0 { dni * cosi } else { 0.0 };
            let diffuse = dhi * (1.0 + tilt.cos()) / 2.0;
            let ground = ghi * albedo * (1.0 - tilt.cos()) / 2.0;
            let poa = (beam + diffuse + ground).max(0.0);
            let cell_temp = temp + poa * (noct_c - 20.0) / 800.0;
            let temp_factor = (1.0 + gamma * (cell_temp - 25.0)).max(0.0);
            let energy = array.peak_power_kwp * (poa / 1000.0) * temp_factor * loss_factor;
            poa_day += poa;
            energy_day += energy;
            ghi_day += ghi;
        }
        let month_poa_kwh_m2 = poa_day * MONTH_DAYS[month] / 1000.0;
        let month_energy_kwh = energy_day * MONTH_DAYS[month];
        let month_ghi_kwh_m2 = ghi_day * MONTH_DAYS[month] / 1000.0;
        monthly.push(MonthlyPvPrediction {
            month: (month + 1) as u8,
            energy_kwh: month_energy_kwh,
            poa_kwh_m2: month_poa_kwh_m2,
            ghi_kwh_m2: month_ghi_kwh_m2,
        });
        annual_poa_kwh_m2 += month_poa_kwh_m2;
        annual_energy_kwh += month_energy_kwh;
        annual_ghi_kwh_m2 += month_ghi_kwh_m2;
    }

    PvPrediction {
        annual_energy_kwh,
        annual_poa_kwh_m2,
        annual_ghi_kwh_m2,
        monthly,
    }
}

fn solar_cosines(
    lat_rad: f64,
    lon: f64,
    surface_azimuth_from_north: f64,
    tilt: f64,
    doy: f64,
    utc_hour: f64,
) -> (f64, f64) {
    let gamma_day = 2.0 * PI / 365.0 * (doy - 1.0 + (utc_hour - 12.0) / 24.0);
    let eqtime = 229.18
        * (0.000075 + 0.001868 * gamma_day.cos()
            - 0.032077 * gamma_day.sin()
            - 0.014615 * (2.0 * gamma_day).cos()
            - 0.040849 * (2.0 * gamma_day).sin());
    let decl = 0.006918 - 0.399912 * gamma_day.cos() + 0.070257 * gamma_day.sin()
        - 0.006758 * (2.0 * gamma_day).cos()
        + 0.000907 * (2.0 * gamma_day).sin()
        - 0.002697 * (3.0 * gamma_day).cos()
        + 0.00148 * (3.0 * gamma_day).sin();
    let true_solar_time = (utc_hour * 60.0 + eqtime + 4.0 * lon).rem_euclid(1440.0);
    let hour_angle = (true_solar_time / 4.0 - 180.0).to_radians();
    let cosz =
        (lat_rad.sin() * decl.sin() + lat_rad.cos() * decl.cos() * hour_angle.cos()).max(0.0);
    if cosz <= 0.0 {
        return (0.0, 0.0);
    }
    let solar_altitude = cosz.clamp(-1.0, 1.0).asin();
    let cos_altitude = solar_altitude.cos().max(1e-9);
    let sin_azimuth = -hour_angle.sin() * decl.cos() / cos_altitude;
    let cos_azimuth =
        (decl.sin() - solar_altitude.sin() * lat_rad.sin()) / (cos_altitude * lat_rad.cos());
    let mut azimuth = sin_azimuth.atan2(cos_azimuth);
    if azimuth < 0.0 {
        azimuth += 2.0 * PI;
    }
    let cos_incidence =
        solar_altitude.cos() * tilt.sin() * (azimuth - surface_azimuth_from_north).cos()
            + solar_altitude.sin() * tilt.cos();
    (cosz, cos_incidence.max(0.0))
}

fn source_estimate(source_id: &str, prediction: PvPrediction) -> Result<SourceAnnualPvEstimate> {
    Ok(SourceAnnualPvEstimate {
        weather_source_id: WeatherSourceId::new(source_id)?,
        annual_energy: Energy::from_kilowatt_hours(prediction.annual_energy_kwh),
        annual_in_plane_irradiation: Irradiation::from_kilowatt_hours_per_square_meter(
            prediction.annual_poa_kwh_m2,
        ),
        annual_global_horizontal_irradiation: Irradiation::from_kilowatt_hours_per_square_meter(
            prediction.annual_ghi_kwh_m2,
        ),
        monthly_estimates: prediction
            .monthly
            .into_iter()
            .map(|monthly| {
                Ok(SourceMonthlyPvEstimate {
                    month: MonthOfYear::new(monthly.month)
                        .ok_or_else(|| anyhow!("invalid month {}", monthly.month))?,
                    energy: Energy::from_kilowatt_hours(monthly.energy_kwh),
                    in_plane_irradiation: Irradiation::from_kilowatt_hours_per_square_meter(
                        monthly.poa_kwh_m2,
                    ),
                    global_horizontal_irradiation:
                        Irradiation::from_kilowatt_hours_per_square_meter(monthly.ghi_kwh_m2),
                })
            })
            .collect::<Result<Vec<_>>>()?,
    })
}

fn default_uncertainty_multiplier() -> f64 {
    DEFAULT_UNCERTAINTY_MULTIPLIER
}

fn default_input_name() -> String {
    "features".to_string()
}

fn default_output_name() -> String {
    "normalized_targets".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_array_inputs() {
        validate_arrays(&[
            EstimateArray {
                peak_power_kwp: 1.5,
                tilt_deg: 30.0,
                azimuth_deg: 0.0,
            },
            EstimateArray {
                peak_power_kwp: 2.0,
                tilt_deg: 15.0,
                azimuth_deg: -90.0,
            },
        ])
        .expect("valid arrays");

        assert!(validate_arrays(&[]).is_err());
        assert!(
            validate_arrays(&[EstimateArray {
                peak_power_kwp: 1.0,
                tilt_deg: 91.0,
                azimuth_deg: 0.0,
            }])
            .is_err()
        );
    }

    #[test]
    fn location_loss_validation_ignores_legacy_array_fields() {
        let request = EstimateRequest {
            peak_power_kwp: -1.0,
            tilt_deg: 120.0,
            ..EstimateRequest::default()
        };

        validate_request_location_and_loss(&request)
            .expect("multi-array validation does not use legacy array fields");
        assert!(validate_request(&request).is_err());
    }

    #[test]
    fn feature_encoder_matches_expected_shape_and_first_values() {
        let features = encode_features(40.65, 15.643, BASE_INPUT_FEATURES, None).unwrap();
        assert_eq!(features.shape(), [TEMPORAL_BINS, BASE_INPUT_FEATURES]);
        assert!((features[[0, 0]] - (40.65 / 90.0) as f32).abs() < 1e-6);
        assert!((features[[0, 1]] - (15.643 / 180.0) as f32).abs() < 1e-6);
    }

    #[test]
    fn mask_contains_coordinates_inside_intervals() {
        let mask = CoverageMask {
            rows: vec![CoverageMaskRow {
                lat_min: 40.0,
                lat_max: 41.0,
                lon_intervals: vec![[15.0, 16.0]],
            }],
        };
        assert!(mask.contains(40.65, 15.643));
        assert!(!mask.contains(42.0, 15.643));
        assert!(!mask.contains(40.65, 17.0));
    }

    #[test]
    fn geography_features_use_exact_coordinate_match() {
        let grid = test_feature_grid();
        let features = grid.normalized_features(40.0, 15.0).unwrap();
        assert_eq!(features, vec![0.0, -1.0]);
    }

    #[test]
    fn geography_features_interpolate_nearest_rows() {
        let grid = test_feature_grid();
        let features = grid.normalized_features(40.5, 15.0).unwrap();
        assert!((features[0] - 0.5).abs() < 1.0e-6);
    }

    #[test]
    fn v2_feature_encoder_appends_geography_columns() {
        let grid = test_feature_grid();
        let features =
            encode_features(40.0, 15.0, BASE_INPUT_FEATURES + grid.width(), Some(&grid)).unwrap();
        assert_eq!(features.shape(), [TEMPORAL_BINS, BASE_INPUT_FEATURES + 2]);
        assert_eq!(features[[0, BASE_INPUT_FEATURES]], 0.0);
        assert_eq!(features[[0, BASE_INPUT_FEATURES + 1]], -1.0);
    }

    fn test_feature_grid() -> GeographyFeatureGrid {
        GeographyFeatureGrid {
            columns: vec!["elevation".to_string(), "water".to_string()],
            mean: vec![100.0, 0.5],
            std: vec![100.0, 0.5],
            clip_abs: Some(8.0),
            rows: vec![
                GeographyFeatureRow {
                    latitude: 40.0,
                    longitude: 15.0,
                    values: vec![100.0, 0.0],
                },
                GeographyFeatureRow {
                    latitude: 41.0,
                    longitude: 15.0,
                    values: vec![200.0, 1.0],
                },
            ],
        }
    }
}
