use std::f64::consts::PI;
use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand, ValueEnum};
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

const INPUT_FEATURES: usize = 66;
const TARGETS: usize = 10;
const TEMPORAL_BINS: usize = 288;
const MONTH_DAYS: [f64; 12] = [
    31.0, 28.0, 31.0, 30.0, 31.0, 30.0, 31.0, 31.0, 30.0, 31.0, 30.0, 31.0,
];
const MID_MONTH_DOY: [f64; 12] = [
    15.0, 46.0, 74.0, 105.0, 135.0, 166.0, 196.0, 227.0, 258.0, 288.0, 319.0, 349.0,
];
const DEFAULT_UNCERTAINTY_MULTIPLIER: f64 = 2.0;

#[derive(Debug, Parser)]
#[command(name = "pv")]
#[command(about = "PV estimator command line tools")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Estimate(EstimateArgs),
}

#[derive(Debug, Parser)]
struct EstimateArgs {
    #[arg(long)]
    lat: f64,
    #[arg(long)]
    lon: f64,
    #[arg(long, default_value = "custom")]
    location_id: String,
    #[arg(long, default_value = "Custom location")]
    name: String,
    #[arg(long, default_value = "")]
    region: String,
    #[arg(long, default_value_t = 1.0)]
    kwp: f64,
    #[arg(long, default_value_t = 14.0)]
    loss_pct: f64,
    #[arg(long, default_value_t = 30.0)]
    tilt_deg: f64,
    #[arg(long, default_value_t = 0.0)]
    azimuth_deg: f64,
    #[arg(long)]
    model_dir: PathBuf,
    #[arg(long, default_value = "source-model-artifacts.json")]
    manifest: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    format: OutputFormat,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat {
    Table,
    Json,
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

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Estimate(args) => estimate(args),
    }
}

fn estimate(args: EstimateArgs) -> Result<()> {
    validate_args(&args)?;
    let manifest_path = args.model_dir.join(&args.manifest);
    let manifest = load_manifest(&manifest_path)?;
    let sources = load_sources(&args.model_dir, manifest.sources)?;
    let features = encode_features(args.lat, args.lon);

    let mut source_estimates = Vec::new();
    let mut applicable_sources = Vec::new();
    let mut sarah3_applicable = false;

    for mut source in sources {
        let applies = source.applies(args.lat, args.lon);
        if source.source_id == "pvgis_sarah3" {
            sarah3_applicable = applies;
        }
        if !applies {
            continue;
        }
        let climate = source.predict_climate(&features)?;
        let pv = estimate_pv_from_climate(&climate, &args);
        applicable_sources.push(WeatherSourceId::new(&source.source_id)?);
        source_estimates.push(source_estimate(&source.source_id, pv)?);
    }

    let ensemble = AnnualPvEnsembleEstimate::from_source_estimates_with_uncertainty(
        source_estimates,
        manifest.uncertainty_multiplier,
    )
    .ok_or_else(|| anyhow!("no applicable source models for {},{}", args.lat, args.lon))?;

    let document = SourceEnsembleEstimateDocument {
        schema_version: 1,
        location: EstimateLocation {
            location_id: args.location_id,
            name: args.name,
            region: args.region,
            latitude: args.lat,
            longitude: args.lon,
        },
        system: EstimateSystem {
            peak_power_kwp: args.kwp,
            loss_pct: args.loss_pct,
            tilt_deg: args.tilt_deg,
            aspect_deg: args.azimuth_deg,
        },
        coverage: EstimateCoverage {
            pvgis_sarah3_applicable: sarah3_applicable,
            applicable_sources,
        },
        ensemble_estimate: ensemble,
        references: json!({
            "model_family": manifest.model_family,
            "input_features": manifest.input_features,
            "temporal_bins": manifest.temporal_bins,
            "target_names": manifest.target_names,
        }),
    };

    match args.format {
        OutputFormat::Json => write_json(&document),
        OutputFormat::Table => write_table(&document),
    }
}

fn validate_args(args: &EstimateArgs) -> Result<()> {
    if !(-90.0..=90.0).contains(&args.lat) {
        bail!("--lat must be in [-90, 90]");
    }
    if !(-180.0..=180.0).contains(&args.lon) {
        bail!("--lon must be in [-180, 180]");
    }
    if args.kwp <= 0.0 {
        bail!("--kwp must be positive");
    }
    if !(0.0..100.0).contains(&args.loss_pct) {
        bail!("--loss-pct must be in [0, 100)");
    }
    if !(0.0..=90.0).contains(&args.tilt_deg) {
        bail!("--tilt-deg must be in [0, 90]");
    }
    Ok(())
}

fn load_manifest(path: &Path) -> Result<ArtifactManifest> {
    let manifest: ArtifactManifest = serde_json::from_reader(
        File::open(path)
            .with_context(|| format!("opening artifact manifest {}", path.display()))?,
    )
    .with_context(|| format!("parsing artifact manifest {}", path.display()))?;
    if manifest.schema_version != 1 {
        bail!(
            "unsupported artifact manifest schema_version={}",
            manifest.schema_version
        );
    }
    if manifest.input_features != INPUT_FEATURES {
        bail!(
            "artifact manifest input_features={} but CLI expects {INPUT_FEATURES}",
            manifest.input_features
        );
    }
    if manifest.temporal_bins != TEMPORAL_BINS {
        bail!(
            "artifact manifest temporal_bins={} but CLI expects {TEMPORAL_BINS}",
            manifest.temporal_bins
        );
    }
    if manifest.target_names.len() != TARGETS {
        bail!(
            "artifact manifest has {} targets but CLI expects {TARGETS}",
            manifest.target_names.len()
        );
    }
    Ok(manifest)
}

fn load_sources(model_dir: &Path, sources: Vec<ArtifactSource>) -> Result<Vec<LoadedSource>> {
    sources
        .into_iter()
        .filter(|source| source.active)
        .map(|source| load_source(model_dir, source))
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

fn load_mask(path: &Path) -> Result<CoverageMask> {
    serde_json::from_reader(
        File::open(path).with_context(|| format!("opening coverage mask {}", path.display()))?,
    )
    .with_context(|| format!("parsing coverage mask {}", path.display()))
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

fn encode_features(lat: f64, lon: f64) -> Array2<f32> {
    let mut output = Array2::<f32>::zeros((TEMPORAL_BINS, INPUT_FEATURES));
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
        debug_assert_eq!(col, INPUT_FEATURES);
    }
    output
}

fn estimate_pv_from_climate(
    climate: &[[f64; TARGETS]; TEMPORAL_BINS],
    args: &EstimateArgs,
) -> PvPrediction {
    let lat_rad = args.lat.to_radians();
    let tilt = args.tilt_deg.to_radians();
    let surface_azimuth_from_north = ((180.0 + args.azimuth_deg) % 360.0).to_radians();
    let loss_factor = 1.0 - args.loss_pct / 100.0;
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
                args.lon,
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
            let energy = args.kwp * (poa / 1000.0) * temp_factor * loss_factor;
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

fn write_json(document: &SourceEnsembleEstimateDocument) -> Result<()> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, document)?;
    writeln!(stdout)?;
    Ok(())
}

fn write_table(document: &SourceEnsembleEstimateDocument) -> Result<()> {
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
    println!("PV estimate for {}", document.location.name);
    println!(
        "location: {:.4}, {:.4}",
        document.location.latitude, document.location.longitude
    );
    println!(
        "system: {:.2} kWp, loss {:.1}%, tilt {:.1} deg, azimuth {:.1} deg",
        document.system.peak_power_kwp,
        document.system.loss_pct,
        document.system.tilt_deg,
        document.system.aspect_deg
    );
    println!(
        "sources: {}",
        document
            .coverage
            .applicable_sources
            .iter()
            .map(|source| source.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("annual energy: {:.2} kWh", annual);
    println!("display band: {uncertainty}");
    println!(
        "annual in-plane irradiation: {:.2} kWh/m2",
        estimate
            .annual_in_plane_irradiation
            .mean
            .as_kilowatt_hours_per_square_meter()
    );
    println!();
    println!("month  energy_kwh  band_kwh");
    for monthly in &estimate.monthly_estimates {
        let band = monthly
            .uncertainty
            .annual_energy
            .map(|band| {
                format!(
                    "{:.2}..{:.2}",
                    band.low.as_kilowatt_hours(),
                    band.high.as_kilowatt_hours()
                )
            })
            .unwrap_or_else(|| "insufficient".to_string());
        println!(
            "{:>5}  {:>10.2}  {}",
            monthly.month.value(),
            monthly.energy.mean.as_kilowatt_hours(),
            band
        );
    }
    Ok(())
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
    fn feature_encoder_matches_expected_shape_and_first_values() {
        let features = encode_features(40.65, 15.643);
        assert_eq!(features.shape(), [TEMPORAL_BINS, INPUT_FEATURES]);
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
}
