use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;

const HOURS_PER_YEAR: usize = 8760;
#[cfg(test)]
const MONTH_DAYS: [usize; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
const DEFAULT_RUNS: usize = 10_000;
const DEFAULT_SEED: u64 = 0x9e37_79b9_7f4a_7c15;
const ROUND_TRIP_EFFICIENCY: f64 = 0.95;
const SIMULATION_BATCH_RUNS: usize = 1000;
const PV_DAILY_VARIABILITY: f64 = 0.35;
const LOAD_DAILY_VARIABILITY: f64 = 0.20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProductionProfile {
    pub annual_mean_kwh: f64,
    pub annual_low_kwh: f64,
    pub annual_high_kwh: f64,
    pub hourly_mean_kwh: Vec<f64>,
    pub monthly: Vec<MonthlyProductionBand>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MonthlyProductionBand {
    pub month: u8,
    pub mean_kwh: f64,
    pub low_kwh: f64,
    pub high_kwh: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LoadProfile {
    AnnualKwh { annual_kwh: f64, shape: LoadShape },
    DailyKwh { daily_kwh: f64, shape: LoadShape },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LoadShape {
    BuiltIn { shape_id: BuiltInLoadShapeId },
    HourlyWeights { weights: Vec<f64> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltInLoadShapeId {
    ResidentialDefault,
    Flat,
    Daytime,
    Evening,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StorageConfig {
    pub usable_capacity_kwh: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimulationOptions {
    pub runs: usize,
    pub seed: Option<u64>,
}

impl Default for SimulationOptions {
    fn default() -> Self {
        Self {
            runs: DEFAULT_RUNS,
            seed: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimulationRequest {
    pub production: ProductionProfile,
    pub load: LoadProfile,
    pub storage: Option<StorageConfig>,
    #[serde(default)]
    pub options: SimulationOptions,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimulationResult {
    pub requested_runs: usize,
    pub completed_runs: usize,
    pub cancelled: bool,
    pub summaries: SimulationMetricSummaries,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SimulationRunMetrics {
    pub production_kwh: f64,
    pub load_kwh: f64,
    pub self_consumed_kwh: f64,
    pub grid_import_kwh: f64,
    pub grid_export_kwh: f64,
    pub battery_losses_kwh: f64,
    pub ending_soc_kwh: f64,
    pub self_consumption_ratio: f64,
    pub self_sufficiency_ratio: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SimulationMetricSummaries {
    pub production_kwh: MetricSummary,
    pub load_kwh: MetricSummary,
    pub self_consumed_kwh: MetricSummary,
    pub grid_import_kwh: MetricSummary,
    pub grid_export_kwh: MetricSummary,
    pub battery_losses_kwh: MetricSummary,
    pub ending_soc_kwh: MetricSummary,
    pub self_consumption_ratio: MetricSummary,
    pub self_sufficiency_ratio: MetricSummary,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct MetricSummary {
    pub p10: f64,
    pub p50: f64,
    pub p90: f64,
    pub mean: f64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimulationError {
    message: String,
}

impl SimulationError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for SimulationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for SimulationError {}

pub fn simulate(request: &SimulationRequest) -> Result<SimulationResult, SimulationError> {
    simulate_with_progress(request, || false, |_| {})
}

pub fn simulate_with_cancellation(
    request: &SimulationRequest,
    cancelled: impl FnMut() -> bool,
) -> Result<SimulationResult, SimulationError> {
    simulate_with_progress(request, cancelled, |_| {})
}

pub fn simulate_with_progress(
    request: &SimulationRequest,
    mut cancelled: impl FnMut() -> bool,
    mut progress: impl FnMut(usize),
) -> Result<SimulationResult, SimulationError> {
    validate_request(request)?;
    let load = hourly_load_profile(&request.load)?;
    let capacity = request
        .storage
        .map(|storage| storage.usable_capacity_kwh)
        .unwrap_or(0.0);
    let requested_runs = request.options.runs;
    let base_seed = request.options.seed.unwrap_or(DEFAULT_SEED);
    let mut run_slots = vec![None; requested_runs];
    let mut completed_runs = 0;

    run_slots[0] = Some(simulate_run_index(
        &request.production,
        &load,
        capacity,
        base_seed,
        0,
    ));
    completed_runs += 1;
    progress(completed_runs);

    if requested_runs == 1 {
        return Ok(simulation_result(requested_runs, false, &run_slots));
    }
    if cancelled() {
        return Ok(simulation_result(requested_runs, true, &run_slots));
    }

    let cancel_flag = AtomicBool::new(false);
    let next_run = AtomicUsize::new(1);
    let worker_count = simulation_worker_count(requested_runs - 1);
    let (sender, receiver) = mpsc::channel::<Vec<(usize, SimulationRunMetrics)>>();

    thread::scope(|scope| {
        for _ in 0..worker_count {
            let sender = sender.clone();
            let production = &request.production;
            let load = &load;
            let cancel_flag = &cancel_flag;
            let next_run = &next_run;
            scope.spawn(move || {
                loop {
                    if cancel_flag.load(Ordering::Relaxed) {
                        break;
                    }
                    let start = next_run.fetch_add(SIMULATION_BATCH_RUNS, Ordering::Relaxed);
                    if start >= requested_runs {
                        break;
                    }
                    let end = start
                        .saturating_add(SIMULATION_BATCH_RUNS)
                        .min(requested_runs);
                    let batch = (start..end)
                        .map(|run_index| {
                            (
                                run_index,
                                simulate_run_index(
                                    production, load, capacity, base_seed, run_index,
                                ),
                            )
                        })
                        .collect::<Vec<_>>();
                    if sender.send(batch).is_err() {
                        break;
                    }
                }
            });
        }
        drop(sender);

        while completed_runs < requested_runs {
            let Ok(batch) = receiver.recv() else {
                break;
            };
            let mut batch_completed = 0;
            for (run_index, metrics) in batch {
                if run_slots[run_index].is_none() {
                    run_slots[run_index] = Some(metrics);
                    batch_completed += 1;
                }
            }
            if batch_completed > 0 {
                completed_runs += batch_completed;
                progress(completed_runs);
            }
            if completed_runs < requested_runs && cancelled() {
                cancel_flag.store(true, Ordering::Relaxed);
                break;
            }
        }
    });

    Ok(simulation_result(
        requested_runs,
        completed_runs < requested_runs,
        &run_slots,
    ))
}

fn simulation_worker_count(remaining_runs: usize) -> usize {
    thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .min(remaining_runs)
        .max(1)
}

fn simulate_run_index(
    profile: &ProductionProfile,
    base_load: &[f64],
    capacity_kwh: f64,
    base_seed: u64,
    run_index: usize,
) -> SimulationRunMetrics {
    let mut rng = SmallRng::new(run_seed(base_seed, run_index));
    let production = stochastic_hourly_production(profile, &mut rng);
    let load = stochastic_hourly_load(base_load, &mut rng);
    dispatch_run(&production, &load, capacity_kwh)
}

fn run_seed(base_seed: u64, run_index: usize) -> u64 {
    let mut value = base_seed ^ (run_index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn simulation_result(
    requested_runs: usize,
    cancelled: bool,
    run_slots: &[Option<SimulationRunMetrics>],
) -> SimulationResult {
    let runs = run_slots.iter().flatten().copied().collect::<Vec<_>>();
    SimulationResult {
        requested_runs,
        completed_runs: runs.len(),
        cancelled,
        summaries: summarize_runs(&runs),
    }
}

fn validate_request(request: &SimulationRequest) -> Result<(), SimulationError> {
    if request.options.runs == 0 {
        return Err(SimulationError::new("simulation runs must be positive"));
    }
    if !request.production.annual_mean_kwh.is_finite()
        || !request.production.annual_low_kwh.is_finite()
        || !request.production.annual_high_kwh.is_finite()
        || request.production.annual_mean_kwh < 0.0
        || request.production.annual_low_kwh < 0.0
        || request.production.annual_high_kwh < 0.0
        || request.production.annual_low_kwh > request.production.annual_high_kwh
    {
        return Err(SimulationError::new(
            "annual production band must be finite, non-negative, and ordered",
        ));
    }
    if request.production.hourly_mean_kwh.len() != HOURS_PER_YEAR {
        return Err(SimulationError::new(
            "production profile must contain 8760 hourly values",
        ));
    }
    if request.production.monthly.len() != 12 {
        return Err(SimulationError::new(
            "production profile must contain 12 monthly bands",
        ));
    }
    for (index, value) in request.production.hourly_mean_kwh.iter().enumerate() {
        if !value.is_finite() || *value < 0.0 {
            return Err(SimulationError::new(format!(
                "production hour {} must be finite and non-negative",
                index + 1
            )));
        }
    }
    for (index, band) in request.production.monthly.iter().enumerate() {
        if band.month as usize != index + 1 {
            return Err(SimulationError::new(
                "monthly production bands must be ordered 1..=12",
            ));
        }
        if !band.mean_kwh.is_finite()
            || !band.low_kwh.is_finite()
            || !band.high_kwh.is_finite()
            || band.mean_kwh < 0.0
            || band.low_kwh < 0.0
            || band.high_kwh < 0.0
            || band.low_kwh > band.high_kwh
        {
            return Err(SimulationError::new(
                "monthly production bands must be finite, non-negative, and ordered",
            ));
        }
    }
    if let Some(storage) = request.storage {
        if !storage.usable_capacity_kwh.is_finite() || storage.usable_capacity_kwh <= 0.0 {
            return Err(SimulationError::new(
                "storage usable capacity must be positive",
            ));
        }
    }
    Ok(())
}

fn hourly_load_profile(load: &LoadProfile) -> Result<Vec<f64>, SimulationError> {
    let (energy_kwh, shape) = match load {
        LoadProfile::AnnualKwh { annual_kwh, shape } => (*annual_kwh, shape),
        LoadProfile::DailyKwh { daily_kwh, shape } => (*daily_kwh * 365.0, shape),
    };
    if !energy_kwh.is_finite() || energy_kwh <= 0.0 {
        return Err(SimulationError::new("load energy must be positive"));
    }
    let weights = yearly_shape_weights(shape)?;
    let sum = weights.iter().sum::<f64>();
    if sum <= 0.0 {
        return Err(SimulationError::new(
            "load shape weights must contain positive energy",
        ));
    }
    Ok(weights
        .into_iter()
        .map(|weight| energy_kwh * weight / sum)
        .collect())
}

fn yearly_shape_weights(shape: &LoadShape) -> Result<Vec<f64>, SimulationError> {
    match shape {
        LoadShape::BuiltIn { shape_id } => {
            Ok(repeat_daily_weights(built_in_daily_weights(*shape_id)))
        }
        LoadShape::HourlyWeights { weights } if weights.len() == 24 => {
            Ok(repeat_daily_weights(weights))
        }
        LoadShape::HourlyWeights { weights } if weights.len() == HOURS_PER_YEAR => {
            validate_weights(weights)?;
            Ok(weights.clone())
        }
        LoadShape::HourlyWeights { .. } => Err(SimulationError::new(
            "hourly load weights must contain either 24 or 8760 values",
        )),
    }
}

fn repeat_daily_weights(weights: &[f64]) -> Vec<f64> {
    validate_weights(weights).expect("built-in weights are valid");
    let mut output = Vec::with_capacity(HOURS_PER_YEAR);
    for _ in 0..365 {
        output.extend_from_slice(weights);
    }
    output
}

fn validate_weights(weights: &[f64]) -> Result<(), SimulationError> {
    if weights.is_empty() {
        return Err(SimulationError::new("load shape weights are required"));
    }
    for value in weights {
        if !value.is_finite() || *value < 0.0 {
            return Err(SimulationError::new(
                "load shape weights must be finite and non-negative",
            ));
        }
    }
    Ok(())
}

fn stochastic_hourly_production(profile: &ProductionProfile, rng: &mut SmallRng) -> Vec<f64> {
    let mut output = profile.hourly_mean_kwh.clone();
    let run_weight = production_run_weight(profile, rng);
    apply_daily_variability(&mut output, 365, PV_DAILY_VARIABILITY, rng);
    scale_slice_by(&mut output, run_weight);
    output
}

fn production_run_weight(profile: &ProductionProfile, rng: &mut SmallRng) -> f64 {
    if profile.annual_mean_kwh <= 0.0 || profile.annual_high_kwh <= profile.annual_low_kwh {
        return 1.0;
    }

    let low_weight = profile.annual_low_kwh / profile.annual_mean_kwh;
    let high_weight = profile.annual_high_kwh / profile.annual_mean_kwh;
    low_weight + (high_weight - low_weight) * rng.next_f64()
}

fn stochastic_hourly_load(base_load: &[f64], rng: &mut SmallRng) -> Vec<f64> {
    let mut output = base_load.to_vec();
    apply_daily_variability(&mut output, 365, LOAD_DAILY_VARIABILITY, rng);
    scale_slice_to_total(&mut output, base_load.iter().sum::<f64>());
    output
}

fn apply_daily_variability(hours: &mut [f64], days: usize, variability: f64, rng: &mut SmallRng) {
    for day in 0..days {
        let start = day * 24;
        let end = start + 24;
        let multiplier = daily_multiplier(variability, rng);
        for value in &mut hours[start..end] {
            *value *= multiplier;
        }
    }
}

fn daily_multiplier(variability: f64, rng: &mut SmallRng) -> f64 {
    let spread = variability.max(0.0);
    (1.0 + (rng.next_f64() * 2.0 - 1.0) * spread).max(0.0)
}

fn scale_slice_by(values: &mut [f64], scale: f64) {
    for value in values {
        *value *= scale;
    }
}

fn scale_slice_to_total(values: &mut [f64], target: f64) {
    let current = values.iter().sum::<f64>();
    if current <= 0.0 {
        return;
    }
    let scale = target / current;
    for value in values.iter_mut() {
        *value *= scale;
    }
    adjust_slice_to_total(values, target);
}

fn adjust_slice_to_total(values: &mut [f64], target: f64) {
    let adjusted = values.iter().sum::<f64>();
    if let Some(last) = values.iter_mut().rev().find(|value| **value > 0.0) {
        *last = (*last + target - adjusted).max(0.0);
    }
}

fn dispatch_run(production: &[f64], load: &[f64], capacity_kwh: f64) -> SimulationRunMetrics {
    let charge_efficiency = ROUND_TRIP_EFFICIENCY.sqrt();
    let discharge_efficiency = ROUND_TRIP_EFFICIENCY.sqrt();
    let mut soc = capacity_kwh * 0.5;
    let initial_soc = soc;
    let mut metrics = SimulationRunMetrics {
        production_kwh: 0.0,
        load_kwh: 0.0,
        self_consumed_kwh: 0.0,
        grid_import_kwh: 0.0,
        grid_export_kwh: 0.0,
        battery_losses_kwh: 0.0,
        ending_soc_kwh: 0.0,
        self_consumption_ratio: 0.0,
        self_sufficiency_ratio: 0.0,
    };

    for (&pv, &demand) in production.iter().zip(load.iter()) {
        metrics.production_kwh += pv;
        metrics.load_kwh += demand;
        let direct = pv.min(demand);
        metrics.self_consumed_kwh += direct;
        let surplus = pv - direct;
        let deficit = demand - direct;

        if capacity_kwh > 0.0 && surplus > 0.0 {
            let room = (capacity_kwh - soc).max(0.0);
            let accepted = surplus.min(room / charge_efficiency);
            let stored = accepted * charge_efficiency;
            soc += stored;
            metrics.battery_losses_kwh += accepted - stored;
            metrics.grid_export_kwh += surplus - accepted;
        } else {
            metrics.grid_export_kwh += surplus;
        }

        if capacity_kwh > 0.0 && deficit > 0.0 {
            let delivered = deficit.min(soc * discharge_efficiency);
            let removed = delivered / discharge_efficiency;
            soc -= removed;
            metrics.self_consumed_kwh += delivered;
            metrics.battery_losses_kwh += removed - delivered;
            metrics.grid_import_kwh += deficit - delivered;
        } else {
            metrics.grid_import_kwh += deficit;
        }
    }

    metrics.ending_soc_kwh = soc;
    metrics.self_consumption_ratio = ratio(metrics.self_consumed_kwh, metrics.production_kwh);
    metrics.self_sufficiency_ratio = ratio(metrics.self_consumed_kwh, metrics.load_kwh);
    debug_assert!(initial_soc >= 0.0);
    metrics
}

fn ratio(numerator: f64, denominator: f64) -> f64 {
    if denominator > 0.0 {
        numerator / denominator
    } else {
        0.0
    }
}

fn summarize_runs(runs: &[SimulationRunMetrics]) -> SimulationMetricSummaries {
    SimulationMetricSummaries {
        production_kwh: summarize_metric(runs.iter().map(|run| run.production_kwh)),
        load_kwh: summarize_metric(runs.iter().map(|run| run.load_kwh)),
        self_consumed_kwh: summarize_metric(runs.iter().map(|run| run.self_consumed_kwh)),
        grid_import_kwh: summarize_metric(runs.iter().map(|run| run.grid_import_kwh)),
        grid_export_kwh: summarize_metric(runs.iter().map(|run| run.grid_export_kwh)),
        battery_losses_kwh: summarize_metric(runs.iter().map(|run| run.battery_losses_kwh)),
        ending_soc_kwh: summarize_metric(runs.iter().map(|run| run.ending_soc_kwh)),
        self_consumption_ratio: summarize_metric(runs.iter().map(|run| run.self_consumption_ratio)),
        self_sufficiency_ratio: summarize_metric(runs.iter().map(|run| run.self_sufficiency_ratio)),
    }
}

fn summarize_metric(values: impl IntoIterator<Item = f64>) -> MetricSummary {
    let mut values = values.into_iter().collect::<Vec<_>>();
    if values.is_empty() {
        return MetricSummary::default();
    }
    values.sort_by(f64::total_cmp);
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    MetricSummary {
        p10: percentile(&values, 0.10),
        p50: percentile(&values, 0.50),
        p90: percentile(&values, 0.90),
        mean,
    }
}

fn percentile(sorted: &[f64], percentile: f64) -> f64 {
    let index = (percentile * (sorted.len().saturating_sub(1)) as f64).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

#[derive(Debug, Clone, Copy)]
struct SmallRng {
    state: u64,
}

impl SmallRng {
    fn new(seed: u64) -> Self {
        Self { state: seed.max(1) }
    }

    fn next_f64(&mut self) -> f64 {
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        let value = self.state.wrapping_mul(0x2545_f491_4f6c_dd1d);
        ((value >> 11) as f64) / ((1u64 << 53) as f64)
    }
}

fn built_in_daily_weights(shape_id: BuiltInLoadShapeId) -> &'static [f64; 24] {
    match shape_id {
        BuiltInLoadShapeId::ResidentialDefault => &RESIDENTIAL_DEFAULT_WEIGHTS,
        BuiltInLoadShapeId::Flat => &FLAT_WEIGHTS,
        BuiltInLoadShapeId::Daytime => &DAYTIME_WEIGHTS,
        BuiltInLoadShapeId::Evening => &EVENING_WEIGHTS,
    }
}

const RESIDENTIAL_DEFAULT_WEIGHTS: [f64; 24] = [
    0.55, 0.45, 0.40, 0.38, 0.40, 0.55, 0.85, 1.05, 0.95, 0.80, 0.72, 0.70, 0.76, 0.78, 0.82, 0.90,
    1.08, 1.35, 1.55, 1.45, 1.20, 0.95, 0.78, 0.65,
];
const FLAT_WEIGHTS: [f64; 24] = [1.0; 24];
const DAYTIME_WEIGHTS: [f64; 24] = [
    0.35, 0.30, 0.28, 0.28, 0.30, 0.45, 0.70, 1.00, 1.20, 1.35, 1.45, 1.50, 1.50, 1.45, 1.35, 1.25,
    1.10, 0.90, 0.75, 0.65, 0.55, 0.48, 0.42, 0.38,
];
const EVENING_WEIGHTS: [f64; 24] = [
    0.45, 0.38, 0.34, 0.32, 0.35, 0.50, 0.75, 0.85, 0.65, 0.50, 0.45, 0.45, 0.50, 0.55, 0.65, 0.80,
    1.05, 1.45, 1.80, 1.75, 1.45, 1.10, 0.80, 0.60,
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn flat_profile(monthly_kwh: f64) -> ProductionProfile {
        let mut hourly = Vec::with_capacity(HOURS_PER_YEAR);
        let mut monthly = Vec::new();
        for (index, days) in MONTH_DAYS.iter().copied().enumerate() {
            let value = monthly_kwh / (days * 24) as f64;
            hourly.extend(std::iter::repeat_n(value, days * 24));
            monthly.push(MonthlyProductionBand {
                month: (index + 1) as u8,
                mean_kwh: monthly_kwh,
                low_kwh: monthly_kwh,
                high_kwh: monthly_kwh,
            });
        }
        ProductionProfile {
            annual_mean_kwh: monthly_kwh * 12.0,
            annual_low_kwh: monthly_kwh * 12.0,
            annual_high_kwh: monthly_kwh * 12.0,
            hourly_mean_kwh: hourly,
            monthly,
        }
    }

    fn request(production: ProductionProfile, storage: Option<f64>) -> SimulationRequest {
        SimulationRequest {
            production,
            load: LoadProfile::AnnualKwh {
                annual_kwh: 3650.0,
                shape: LoadShape::HourlyWeights {
                    weights: vec![1.0; 24],
                },
            },
            storage: storage.map(|usable_capacity_kwh| StorageConfig {
                usable_capacity_kwh,
            }),
            options: SimulationOptions {
                runs: 3,
                seed: Some(42),
            },
        }
    }

    #[test]
    fn no_storage_balances_production_load_import_and_export() {
        let result = simulate(&request(flat_profile(120.0), None)).expect("simulation succeeds");
        let summaries = result.summaries;
        let left = summaries.production_kwh.mean + summaries.grid_import_kwh.mean;
        let right = summaries.load_kwh.mean + summaries.grid_export_kwh.mean;

        assert!((left - right).abs() < 1.0e-9);
    }

    #[test]
    fn storage_reduces_import_when_surplus_precedes_deficit() {
        let mut profile = flat_profile(0.0);
        profile.hourly_mean_kwh[0] = 10.0;
        profile.monthly[0].mean_kwh = 10.0;
        profile.monthly[0].low_kwh = 10.0;
        profile.monthly[0].high_kwh = 10.0;
        let without = simulate(&request(profile.clone(), None)).expect("simulation succeeds");
        let with = simulate(&request(profile, Some(5.0))).expect("simulation succeeds");

        assert!(with.summaries.grid_import_kwh.mean < without.summaries.grid_import_kwh.mean);
    }

    #[test]
    fn battery_efficiency_creates_losses() {
        let mut profile = flat_profile(0.0);
        profile.hourly_mean_kwh[0] = 10.0;
        profile.monthly[0].mean_kwh = 10.0;
        profile.monthly[0].low_kwh = 10.0;
        profile.monthly[0].high_kwh = 10.0;
        let result = simulate(&request(profile, Some(5.0))).expect("simulation succeeds");

        assert!(result.summaries.battery_losses_kwh.mean > 0.0);
    }

    #[test]
    fn same_seed_produces_identical_stochastic_summary() {
        let mut profile = flat_profile(100.0);
        for band in &mut profile.monthly {
            band.low_kwh = 80.0;
            band.high_kwh = 120.0;
        }
        let first = simulate(&request(profile.clone(), None)).expect("simulation succeeds");
        let second = simulate(&request(profile, None)).expect("simulation succeeds");

        assert_eq!(first, second);
    }

    #[test]
    fn stochastic_production_varies_daily_without_monthly_rescale() {
        let profile = flat_profile(120.0);
        let mut rng = SmallRng::new(42);
        let output = stochastic_hourly_production(&profile, &mut rng);
        let january = &output[..MONTH_DAYS[0] * 24];
        let january_total = january.iter().sum::<f64>();
        let first_day = january[..24].iter().sum::<f64>();
        let has_different_day = january
            .chunks_exact(24)
            .map(|day| day.iter().sum::<f64>())
            .any(|day_total| (day_total - first_day).abs() > 1.0e-9);

        assert!((january_total - 120.0).abs() > 1.0e-6);
        assert!(has_different_day);
    }

    #[test]
    fn stochastic_production_samples_annual_run_weight() {
        let mut profile = flat_profile(100.0);
        profile.annual_low_kwh = profile.annual_mean_kwh * 0.8;
        profile.annual_high_kwh = profile.annual_mean_kwh * 1.2;
        let mut rng = SmallRng::new(42);
        let run_weight = production_run_weight(&profile, &mut rng);

        assert!((0.8..=1.2).contains(&run_weight));
        assert!((run_weight - 1.0).abs() > 1.0e-9);
    }

    #[test]
    fn stochastic_load_varies_daily_and_preserves_annual_total() {
        let base_load = vec![1.0; HOURS_PER_YEAR];
        let mut rng = SmallRng::new(42);
        let output = stochastic_hourly_load(&base_load, &mut rng);
        let annual_total = output.iter().sum::<f64>();
        let first_day = output[..24].iter().sum::<f64>();
        let has_different_day = output
            .chunks_exact(24)
            .map(|day| day.iter().sum::<f64>())
            .any(|day_total| (day_total - first_day).abs() > 1.0e-9);

        assert!((annual_total - HOURS_PER_YEAR as f64).abs() < 1.0e-9);
        assert!(has_different_day);
    }

    #[test]
    fn cancellation_returns_partial_result() {
        let calls = Cell::new(0);
        let result = simulate_with_cancellation(&request(flat_profile(100.0), None), || {
            calls.set(calls.get() + 1);
            true
        })
        .expect("simulation succeeds");

        assert!(result.cancelled);
        assert_eq!(result.completed_runs, 1);
    }

    #[test]
    fn progress_reports_completed_runs() {
        let request = request(flat_profile(100.0), None);
        let expected_runs = request.options.runs;
        let mut completed = Vec::new();
        let result = simulate_with_progress(&request, || false, |runs| completed.push(runs))
            .expect("simulation succeeds");

        assert_eq!(result.completed_runs, expected_runs);
        assert_eq!(completed.first(), Some(&1));
        assert_eq!(completed.last(), Some(&expected_runs));
        assert!(completed.windows(2).all(|window| window[0] < window[1]));
    }

    #[test]
    fn progress_reports_batched_worker_completion() {
        let mut request = request(flat_profile(100.0), None);
        request.options.runs = SIMULATION_BATCH_RUNS + 2;
        let mut completed = Vec::new();
        let result = simulate_with_progress(&request, || false, |runs| completed.push(runs))
            .expect("simulation succeeds");

        assert_eq!(result.completed_runs, SIMULATION_BATCH_RUNS + 2);
        assert_eq!(completed.first(), Some(&1));
        assert_eq!(completed.last(), Some(&(SIMULATION_BATCH_RUNS + 2)));
        assert!(
            completed
                .iter()
                .any(|runs| *runs >= SIMULATION_BATCH_RUNS + 1)
        );
    }

    #[test]
    fn built_in_load_shapes_are_available_and_valid() {
        let shapes = [
            BuiltInLoadShapeId::ResidentialDefault,
            BuiltInLoadShapeId::Flat,
            BuiltInLoadShapeId::Daytime,
            BuiltInLoadShapeId::Evening,
        ];

        for shape_id in shapes {
            let weights = built_in_daily_weights(shape_id);
            assert_eq!(weights.len(), 24);
            assert!(
                weights
                    .iter()
                    .all(|weight| weight.is_finite() && *weight >= 0.0)
            );
            assert!(weights.iter().sum::<f64>() > 0.0);
        }
    }

    #[test]
    fn quantile_summaries_are_stable_on_fixture() {
        let summary = summarize_metric([10.0, 20.0, 30.0, 40.0, 50.0]);

        assert_eq!(summary.p10, 10.0);
        assert_eq!(summary.p50, 30.0);
        assert_eq!(summary.p90, 50.0);
        assert_eq!(summary.mean, 30.0);
    }
}
