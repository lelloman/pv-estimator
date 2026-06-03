use std::collections::HashSet;
use std::env;
use std::error::Error;
use std::f32::consts::PI;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

const INPUTS: usize = 64;
const TARGETS: usize = 5;
const QUANTILES: [f32; 3] = [0.1, 0.5, 0.9];
const OUTPUTS: usize = TARGETS * 3;
const CLIMATOLOGY_BUCKETS: usize = 366 * 24;
const NORMAL_10_90_Z: f32 = 1.281_551_6;
const TARGET_NAMES: [&str; TARGETS] = [
    "ghi_w_m2",
    "dni_w_m2",
    "dhi_w_m2",
    "ambient_temperature_c",
    "wind_speed_m_s",
];
const DEFAULT_DATA: &str =
    "experiments/ml-weather/runs/global_grid_408/normalized/nasa_power_hourly.csv.gz";

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("train-weather-mlp") => train_weather_mlp(RunConfig::from_args(args.collect())?),
        Some("weather-climatology-baseline") => {
            weather_climatology_baseline(RunConfig::from_args(args.collect())?)
        }
        Some("normalize-nasa-power") => {
            normalize_nasa_power(NormalizeConfig::from_args(args.collect())?)
        }
        Some("geonames") => geonames(args.collect()),
        Some("help") | None => {
            print_help();
            Ok(())
        }
        Some(command) => Err(format!("unknown xtask command: {command}").into()),
    }
}

fn print_help() {
    println!("xtask commands:");
    println!("  train-weather-mlp --data <csv.gz> --out-dir <dir> [options]");
    println!("  weather-climatology-baseline --data <csv.gz> --out-dir <dir> [options]");
    println!("  normalize-nasa-power --raw-dir <dir> --out <csv.gz> [--workers <n>]");
    println!("  geonames fetch|profile|report [options]");
}

#[derive(Debug)]
struct RunConfig {
    data: PathBuf,
    out_dir: PathBuf,
    train_limit: usize,
    val_limit: usize,
    train_stride: usize,
    val_stride: usize,
    epochs: usize,
    batch_size: usize,
    learning_rate: f32,
    seed: u64,
    hidden_width: usize,
}

impl RunConfig {
    fn from_args(args: Vec<String>) -> Result<Self, Box<dyn Error>> {
        let mut config = Self {
            data: PathBuf::from(DEFAULT_DATA),
            out_dir: PathBuf::from(
                "experiments/ml-weather/runs/global_grid_408/models/weather_mlp",
            ),
            train_limit: 1_000_000,
            val_limit: 100_000,
            train_stride: 13,
            val_stride: 7,
            epochs: 3,
            batch_size: 256,
            learning_rate: 0.001,
            seed: 42,
            hidden_width: 64,
        };

        let mut index = 0;
        while index < args.len() {
            let key = &args[index];
            let value = args
                .get(index + 1)
                .ok_or_else(|| format!("missing value for {key}"))?;
            match key.as_str() {
                "--data" => config.data = PathBuf::from(value),
                "--out-dir" => config.out_dir = PathBuf::from(value),
                "--train-limit" => config.train_limit = value.parse()?,
                "--val-limit" => config.val_limit = value.parse()?,
                "--train-stride" => config.train_stride = value.parse()?,
                "--val-stride" => config.val_stride = value.parse()?,
                "--epochs" => config.epochs = value.parse()?,
                "--batch-size" => config.batch_size = value.parse()?,
                "--learning-rate" => config.learning_rate = value.parse()?,
                "--seed" => config.seed = value.parse()?,
                "--hidden-width" => config.hidden_width = value.parse()?,
                _ => return Err(format!("unknown xtask option: {key}").into()),
            }
            index += 2;
        }

        if config.batch_size == 0 {
            return Err("--batch-size must be positive".into());
        }
        if config.train_limit == 0 || config.val_limit == 0 {
            return Err("train and validation limits must be positive".into());
        }
        if config.train_stride == 0 || config.val_stride == 0 {
            return Err("train and validation strides must be positive".into());
        }
        if config.hidden_width == 0 {
            return Err("--hidden-width must be positive".into());
        }
        Ok(config)
    }
}

#[derive(Clone)]
struct Dataset {
    xs: Vec<[f32; INPUTS]>,
    ys: Vec<[f32; TARGETS]>,
    buckets: Vec<usize>,
}

impl Dataset {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            xs: Vec::with_capacity(capacity),
            ys: Vec::with_capacity(capacity),
            buckets: Vec::with_capacity(capacity),
        }
    }

    fn push(&mut self, sample: Sample) {
        self.xs.push(sample.x);
        self.ys.push(sample.y);
        self.buckets.push(sample.bucket);
    }

    fn len(&self) -> usize {
        self.ys.len()
    }
}

struct Sample<'a> {
    location_id: &'a str,
    x: [f32; INPUTS],
    y: [f32; TARGETS],
    bucket: usize,
}

#[derive(Clone, Copy)]
struct TargetStats {
    mean: [f32; TARGETS],
    std: [f32; TARGETS],
}

#[derive(Default, Clone, Copy)]
struct RunningStats {
    count: usize,
    sum: [f64; TARGETS],
    sum_sq: [f64; TARGETS],
}

impl RunningStats {
    fn add(&mut self, values: &[f32; TARGETS]) {
        self.count += 1;
        for (index, value) in values.iter().enumerate() {
            let value = f64::from(*value);
            self.sum[index] += value;
            self.sum_sq[index] += value * value;
        }
    }

    fn finish(&self) -> TargetStats {
        let mut mean = [0.0; TARGETS];
        let mut std = [1.0; TARGETS];
        let count = self.count.max(1) as f64;
        for index in 0..TARGETS {
            let field_mean = self.sum[index] / count;
            let variance = (self.sum_sq[index] / count - field_mean * field_mean).max(1.0e-12);
            mean[index] = field_mean as f32;
            std[index] = variance.sqrt().max(1.0) as f32;
        }
        TargetStats { mean, std }
    }
}

fn train_weather_mlp(config: RunConfig) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(&config.out_dir)?;
    println!("loading samples from {}", config.data.display());
    let (mut train, val, stats) = load_datasets(&config)?;
    println!("train_rows={} val_rows={}", train.len(), val.len());
    println!("target_mean={:?}", stats.mean);
    println!("target_std={:?}", stats.std);

    normalize_targets(&mut train, &stats);
    let mut model = Mlp::new(config.hidden_width, config.seed);
    println!("parameters={}", model.parameter_count());

    for epoch in 0..config.epochs {
        shuffle_dataset(&mut train, config.seed.wrapping_add(epoch as u64));
        let mut epoch_loss = 0.0f64;
        let mut batches = 0usize;
        for start in (0..train.len()).step_by(config.batch_size) {
            let end = (start + config.batch_size).min(train.len());
            epoch_loss += f64::from(model.train_batch(
                &train.xs[start..end],
                &train.ys[start..end],
                config.learning_rate,
            ));
            batches += 1;
        }
        let average_loss = epoch_loss / batches.max(1) as f64;
        let metrics = evaluate_mlp(&model, &val, &stats);
        println!(
            "epoch={} train_pinball={average_loss:.6} val_pinball={:.6} val_mae={:?}",
            epoch + 1,
            metrics.pinball_loss,
            metrics.mae
        );
    }

    let metrics = evaluate_mlp(&model, &val, &stats);
    write_metrics(
        &config,
        &stats,
        &metrics,
        &config.out_dir.join("metrics.json"),
        "weather_mlp",
        Some(model.parameter_count()),
    )?;
    write_model(&model, &stats, &config.out_dir.join("model.json"))?;
    println!("wrote {}", config.out_dir.join("metrics.json").display());
    println!("wrote {}", config.out_dir.join("model.json").display());
    Ok(())
}

fn weather_climatology_baseline(config: RunConfig) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(&config.out_dir)?;
    println!("loading samples from {}", config.data.display());
    let (train, val, global_stats) = load_datasets(&config)?;
    println!("train_rows={} val_rows={}", train.len(), val.len());

    let mut buckets = vec![RunningStats::default(); CLIMATOLOGY_BUCKETS];
    for (bucket, y) in train.buckets.iter().zip(&train.ys) {
        buckets[*bucket].add(y);
    }
    let bucket_stats = buckets
        .iter()
        .map(|bucket| {
            if bucket.count == 0 {
                global_stats
            } else {
                bucket.finish()
            }
        })
        .collect::<Vec<_>>();

    let metrics = evaluate_climatology(&val, &bucket_stats);
    write_metrics(
        &config,
        &global_stats,
        &metrics,
        &config.out_dir.join("metrics.json"),
        "weather_climatology_day_hour",
        None,
    )?;
    println!(
        "baseline_pinball={:.6} baseline_mae={:?}",
        metrics.pinball_loss, metrics.mae
    );
    println!("wrote {}", config.out_dir.join("metrics.json").display());
    Ok(())
}

fn load_datasets(config: &RunConfig) -> Result<(Dataset, Dataset, TargetStats), Box<dyn Error>> {
    let mut child = Command::new("gzip")
        .arg("-cd")
        .arg(&config.data)
        .stdout(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().ok_or("failed to capture gzip stdout")?;
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader.read_line(&mut line)?;

    let mut train = Dataset::with_capacity(config.train_limit);
    let mut val = Dataset::with_capacity(config.val_limit);
    let mut stats = RunningStats::default();
    let mut row_index = 0usize;
    let mut stopped_early = false;

    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        row_index += 1;
        if train.len() >= config.train_limit && val.len() >= config.val_limit {
            stopped_early = true;
            break;
        }

        let Some(sample) = parse_sample(line.trim_end()) else {
            continue;
        };
        let location_number = parse_grid_number(sample.location_id).unwrap_or(0);
        let is_validation_location = location_number != 0 && location_number.is_multiple_of(17);

        if is_validation_location {
            if val.len() < config.val_limit && row_index.is_multiple_of(config.val_stride) {
                val.push(sample);
            }
        } else if train.len() < config.train_limit && row_index.is_multiple_of(config.train_stride)
        {
            stats.add(&sample.y);
            train.push(sample);
        }
    }

    drop(reader);
    if stopped_early {
        let _ = child.kill();
    }
    let status = child.wait()?;
    if !stopped_early && !status.success() {
        return Err(format!("gzip exited with status {status}").into());
    }
    if train.len() == 0 || val.len() == 0 {
        return Err("training or validation sample is empty".into());
    }

    Ok((train, val, stats.finish()))
}

fn parse_sample(line: &str) -> Option<Sample<'_>> {
    let mut fields = line.split(',');
    let _source_id = fields.next()?;
    let _source_record_type = fields.next()?;
    let location_id = fields.next()?;
    let timestamp = fields.next()?;
    let latitude = fields.next()?.parse::<f32>().ok()?;
    let longitude = fields.next()?.parse::<f32>().ok()?;
    let elevation = fields.next()?.parse::<f32>().unwrap_or(0.0);
    let y = [
        fields.next()?.parse::<f32>().ok()?,
        fields.next()?.parse::<f32>().ok()?,
        fields.next()?.parse::<f32>().ok()?,
        fields.next()?.parse::<f32>().ok()?,
        fields.next()?.parse::<f32>().ok()?,
    ];
    let _wind_height = fields.next()?;
    let _flags = fields.next().unwrap_or_default();
    let (day_of_year, hour) = parse_timestamp(timestamp)?;
    Some(Sample {
        location_id,
        x: encode_features(latitude, longitude, elevation, day_of_year, hour),
        y,
        bucket: ((day_of_year as usize - 1) * 24 + hour as usize).min(CLIMATOLOGY_BUCKETS - 1),
    })
}

fn parse_grid_number(location_id: &str) -> Option<usize> {
    location_id.strip_prefix("grid_")?.parse().ok()
}

fn parse_timestamp(timestamp: &str) -> Option<(u32, u32)> {
    if timestamp.len() < 13 {
        return None;
    }
    let year = timestamp[0..4].parse::<i32>().ok()?;
    let month = timestamp[5..7].parse::<usize>().ok()?;
    let day = timestamp[8..10].parse::<u32>().ok()?;
    let hour = timestamp[11..13].parse::<u32>().ok()?;
    let days_before_month_common = [0u32, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let mut day_of_year = *days_before_month_common.get(month.checked_sub(1)?)? + day;
    if month > 2 && is_leap_year(year) {
        day_of_year += 1;
    }
    Some((day_of_year, hour))
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn encode_features(
    latitude: f32,
    longitude: f32,
    elevation: f32,
    day_of_year: u32,
    hour: u32,
) -> [f32; INPUTS] {
    let mut x = [0.0; INPUTS];
    let lat_norm = latitude / 90.0;
    let lon_norm = longitude / 180.0;
    let elev_norm = (elevation / 3000.0).clamp(-1.0, 3.0);
    let day_angle = 2.0 * PI * (day_of_year as f32 - 1.0) / 366.0;
    let hour_angle = 2.0 * PI * hour as f32 / 24.0;

    let base_day_sin = day_angle.sin();
    let base_day_cos = day_angle.cos();
    let base_hour_sin = hour_angle.sin();
    let base_hour_cos = hour_angle.cos();

    let mut index = 0;
    push_feature(&mut x, &mut index, lat_norm);
    push_feature(&mut x, &mut index, lon_norm);
    push_feature(&mut x, &mut index, elev_norm);
    push_feature(&mut x, &mut index, base_day_sin);
    push_feature(&mut x, &mut index, base_day_cos);
    push_feature(&mut x, &mut index, base_hour_sin);
    push_feature(&mut x, &mut index, base_hour_cos);

    for harmonic in 2..=8 {
        let angle = day_angle * harmonic as f32;
        push_feature(&mut x, &mut index, angle.sin());
        push_feature(&mut x, &mut index, angle.cos());
    }
    for harmonic in 2..=6 {
        let angle = hour_angle * harmonic as f32;
        push_feature(&mut x, &mut index, angle.sin());
        push_feature(&mut x, &mut index, angle.cos());
    }
    for harmonic in 1..=6 {
        let angle = PI * lat_norm * harmonic as f32;
        push_feature(&mut x, &mut index, angle.sin());
        push_feature(&mut x, &mut index, angle.cos());
    }
    for harmonic in 1..=6 {
        let angle = PI * lon_norm * harmonic as f32;
        push_feature(&mut x, &mut index, angle.sin());
        push_feature(&mut x, &mut index, angle.cos());
    }

    push_feature(&mut x, &mut index, lat_norm * base_day_sin);
    push_feature(&mut x, &mut index, lat_norm * base_day_cos);
    push_feature(&mut x, &mut index, lon_norm * base_day_sin);
    push_feature(&mut x, &mut index, lon_norm * base_day_cos);
    push_feature(&mut x, &mut index, base_day_sin * base_hour_sin);
    push_feature(&mut x, &mut index, base_day_sin * base_hour_cos);
    push_feature(&mut x, &mut index, base_day_cos * base_hour_sin);
    push_feature(&mut x, &mut index, base_day_cos * base_hour_cos);
    push_feature(&mut x, &mut index, elev_norm * base_day_sin);

    debug_assert_eq!(index, INPUTS);
    x
}

fn push_feature(x: &mut [f32; INPUTS], index: &mut usize, value: f32) {
    x[*index] = value;
    *index += 1;
}

fn normalize_targets(dataset: &mut Dataset, stats: &TargetStats) {
    for y in &mut dataset.ys {
        for (index, value) in y.iter_mut().enumerate() {
            *value = (*value - stats.mean[index]) / stats.std[index];
        }
    }
}

struct Mlp {
    hidden_width: usize,
    w1: Vec<f32>,
    b1: Vec<f32>,
    w2: Vec<f32>,
    b2: Vec<f32>,
    w3: Vec<f32>,
    b3: Vec<f32>,
}

impl Mlp {
    fn new(hidden_width: usize, seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        Self {
            hidden_width,
            w1: init_weights(&mut rng, INPUTS, hidden_width),
            b1: vec![0.0; hidden_width],
            w2: init_weights(&mut rng, hidden_width, hidden_width),
            b2: vec![0.0; hidden_width],
            w3: init_weights(&mut rng, hidden_width, OUTPUTS),
            b3: vec![0.0; OUTPUTS],
        }
    }

    fn parameter_count(&self) -> usize {
        self.w1.len()
            + self.b1.len()
            + self.w2.len()
            + self.b2.len()
            + self.w3.len()
            + self.b3.len()
    }

    fn predict(&self, x: &[f32; INPUTS]) -> [f32; OUTPUTS] {
        let h = self.hidden_width;
        let mut z1 = vec![0.0; h];
        let mut a1 = vec![0.0; h];
        let mut z2 = vec![0.0; h];
        let mut a2 = vec![0.0; h];
        let mut out = [0.0; OUTPUTS];

        dense_dynamic(x, INPUTS, h, &self.w1, &self.b1, &mut z1);
        for index in 0..h {
            a1[index] = silu(z1[index]);
        }
        dense_dynamic(&a1, h, h, &self.w2, &self.b2, &mut z2);
        for index in 0..h {
            a2[index] = silu(z2[index]);
        }
        dense_dynamic(&a2, h, OUTPUTS, &self.w3, &self.b3, &mut out);
        out
    }

    #[allow(clippy::needless_range_loop)]
    fn train_batch(
        &mut self,
        xs: &[[f32; INPUTS]],
        ys: &[[f32; TARGETS]],
        learning_rate: f32,
    ) -> f32 {
        let h = self.hidden_width;
        let mut gradients = Gradients::new(h);
        let mut loss = 0.0f32;

        let mut z1 = vec![0.0; h];
        let mut a1 = vec![0.0; h];
        let mut z2 = vec![0.0; h];
        let mut a2 = vec![0.0; h];
        let mut d_a2 = vec![0.0; h];
        let mut d_z2 = vec![0.0; h];
        let mut d_a1 = vec![0.0; h];
        let mut d_z1 = vec![0.0; h];

        for (x, y) in xs.iter().zip(ys) {
            z1.fill(0.0);
            a1.fill(0.0);
            z2.fill(0.0);
            a2.fill(0.0);
            d_a2.fill(0.0);
            d_z2.fill(0.0);
            d_a1.fill(0.0);
            d_z1.fill(0.0);
            let mut out = [0.0; OUTPUTS];
            let mut d_out = [0.0; OUTPUTS];

            dense_dynamic(x, INPUTS, h, &self.w1, &self.b1, &mut z1);
            for index in 0..h {
                a1[index] = silu(z1[index]);
            }
            dense_dynamic(&a1, h, h, &self.w2, &self.b2, &mut z2);
            for index in 0..h {
                a2[index] = silu(z2[index]);
            }
            dense_dynamic(&a2, h, OUTPUTS, &self.w3, &self.b3, &mut out);

            for target in 0..TARGETS {
                for (quantile_index, quantile) in QUANTILES.iter().enumerate() {
                    let output_index = target * 3 + quantile_index;
                    let error = y[target] - out[output_index];
                    loss += if error >= 0.0 {
                        quantile * error
                    } else {
                        (quantile - 1.0) * error
                    };
                    d_out[output_index] = if error >= 0.0 {
                        -quantile
                    } else {
                        1.0 - quantile
                    } / OUTPUTS as f32;
                }
            }

            for hidden in 0..h {
                for output in 0..OUTPUTS {
                    gradients.w3[hidden * OUTPUTS + output] += a2[hidden] * d_out[output];
                    d_a2[hidden] += self.w3[hidden * OUTPUTS + output] * d_out[output];
                }
            }
            for output in 0..OUTPUTS {
                gradients.b3[output] += d_out[output];
            }

            for hidden in 0..h {
                d_z2[hidden] = d_a2[hidden] * silu_derivative(z2[hidden]);
            }

            for previous in 0..h {
                for hidden in 0..h {
                    gradients.w2[previous * h + hidden] += a1[previous] * d_z2[hidden];
                    d_a1[previous] += self.w2[previous * h + hidden] * d_z2[hidden];
                }
            }
            for hidden in 0..h {
                gradients.b2[hidden] += d_z2[hidden];
            }

            for hidden in 0..h {
                d_z1[hidden] = d_a1[hidden] * silu_derivative(z1[hidden]);
            }

            for input in 0..INPUTS {
                for hidden in 0..h {
                    gradients.w1[input * h + hidden] += x[input] * d_z1[hidden];
                }
            }
            for hidden in 0..h {
                gradients.b1[hidden] += d_z1[hidden];
            }
        }

        let batch_scale = 1.0 / xs.len().max(1) as f32;
        let rate = learning_rate * batch_scale;
        clip_and_apply(&mut self.w1, &gradients.w1, rate);
        clip_and_apply(&mut self.b1, &gradients.b1, rate);
        clip_and_apply(&mut self.w2, &gradients.w2, rate);
        clip_and_apply(&mut self.b2, &gradients.b2, rate);
        clip_and_apply(&mut self.w3, &gradients.w3, rate);
        clip_and_apply(&mut self.b3, &gradients.b3, rate);
        loss * batch_scale / OUTPUTS as f32
    }
}

struct Gradients {
    w1: Vec<f32>,
    b1: Vec<f32>,
    w2: Vec<f32>,
    b2: Vec<f32>,
    w3: Vec<f32>,
    b3: Vec<f32>,
}

impl Gradients {
    fn new(hidden_width: usize) -> Self {
        Self {
            w1: vec![0.0; INPUTS * hidden_width],
            b1: vec![0.0; hidden_width],
            w2: vec![0.0; hidden_width * hidden_width],
            b2: vec![0.0; hidden_width],
            w3: vec![0.0; hidden_width * OUTPUTS],
            b3: vec![0.0; OUTPUTS],
        }
    }
}

fn dense_dynamic<T: AsRef<[f32]>, U: AsMut<[f32]>>(
    input: T,
    inputs: usize,
    outputs: usize,
    weights: &[f32],
    bias: &[f32],
    mut output: U,
) {
    let input = input.as_ref();
    let output = output.as_mut();
    for out_index in 0..outputs {
        let mut value = bias[out_index];
        for in_index in 0..inputs {
            value += input[in_index] * weights[in_index * outputs + out_index];
        }
        output[out_index] = value;
    }
}

fn init_weights(rng: &mut Rng, inputs: usize, outputs: usize) -> Vec<f32> {
    let scale = (2.0 / (inputs + outputs) as f32).sqrt();
    (0..inputs * outputs)
        .map(|_| (rng.next_f32() * 2.0 - 1.0) * scale)
        .collect()
}

fn silu(value: f32) -> f32 {
    value / (1.0 + (-value).exp())
}

fn silu_derivative(value: f32) -> f32 {
    let sigmoid = 1.0 / (1.0 + (-value).exp());
    sigmoid * (1.0 + value * (1.0 - sigmoid))
}

fn clip_and_apply(parameters: &mut [f32], gradients: &[f32], rate: f32) {
    for (parameter, gradient) in parameters.iter_mut().zip(gradients) {
        *parameter -= rate * gradient.clamp(-5.0, 5.0);
    }
}

#[derive(Clone, Copy)]
struct Metrics {
    pinball_loss: f64,
    mae: [f64; TARGETS],
    rmse: [f64; TARGETS],
    coverage_p10_p90: [f64; TARGETS],
    crossing_rate: f64,
}

fn evaluate_mlp(model: &Mlp, val: &Dataset, stats: &TargetStats) -> Metrics {
    evaluate_predictions(val, |sample_index, target| {
        let out = model.predict(&val.xs[sample_index]);
        [
            denormalize(out[target * 3], target, stats),
            denormalize(out[target * 3 + 1], target, stats),
            denormalize(out[target * 3 + 2], target, stats),
        ]
    })
}

fn evaluate_climatology(val: &Dataset, bucket_stats: &[TargetStats]) -> Metrics {
    evaluate_predictions(val, |sample_index, target| {
        let stats = bucket_stats[val.buckets[sample_index]];
        [
            stats.mean[target] - NORMAL_10_90_Z * stats.std[target],
            stats.mean[target],
            stats.mean[target] + NORMAL_10_90_Z * stats.std[target],
        ]
    })
}

fn evaluate_predictions<F>(val: &Dataset, mut predict: F) -> Metrics
where
    F: FnMut(usize, usize) -> [f32; 3],
{
    let mut pinball_loss = 0.0f64;
    let mut abs_error = [0.0f64; TARGETS];
    let mut squared_error = [0.0f64; TARGETS];
    let mut covered = [0usize; TARGETS];
    let mut crossings = 0usize;
    let mut quantile_sets = 0usize;

    for sample_index in 0..val.len() {
        let y = &val.ys[sample_index];
        for target in 0..TARGETS {
            let actual = y[target];
            let [mut q10, q50, mut q90] = predict(sample_index, target);
            if q10 > q50 || q50 > q90 {
                crossings += 1;
                let mut sorted = [q10, q50, q90];
                sorted.sort_by(|a, b| a.total_cmp(b));
                q10 = sorted[0];
                q90 = sorted[2];
            }
            let error = actual - q50;
            abs_error[target] += f64::from(error.abs());
            squared_error[target] += f64::from(error * error);
            if actual >= q10 && actual <= q90 {
                covered[target] += 1;
            }

            for (prediction, quantile) in [q10, q50, q90].iter().zip(QUANTILES) {
                let q_error = actual - prediction;
                let loss = if q_error >= 0.0 {
                    quantile * q_error
                } else {
                    (quantile - 1.0) * q_error
                };
                pinball_loss += f64::from(loss);
            }
            quantile_sets += 1;
        }
    }

    let count = val.len().max(1) as f64;
    let mut mae = [0.0; TARGETS];
    let mut rmse = [0.0; TARGETS];
    let mut coverage = [0.0; TARGETS];
    for target in 0..TARGETS {
        mae[target] = abs_error[target] / count;
        rmse[target] = (squared_error[target] / count).sqrt();
        coverage[target] = covered[target] as f64 / count;
    }

    Metrics {
        pinball_loss: pinball_loss / (quantile_sets.max(1) * QUANTILES.len()) as f64,
        mae,
        rmse,
        coverage_p10_p90: coverage,
        crossing_rate: crossings as f64 / quantile_sets.max(1) as f64,
    }
}

fn denormalize(value: f32, target: usize, stats: &TargetStats) -> f32 {
    value * stats.std[target] + stats.mean[target]
}

fn shuffle_dataset(dataset: &mut Dataset, seed: u64) {
    let mut rng = Rng::new(seed);
    for index in (1..dataset.len()).rev() {
        let swap_with = (rng.next_u64() as usize) % (index + 1);
        dataset.xs.swap(index, swap_with);
        dataset.ys.swap(index, swap_with);
        dataset.buckets.swap(index, swap_with);
    }
}

struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Self { state: seed.max(1) }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn next_f32(&mut self) -> f32 {
        let value = self.next_u64() >> 40;
        value as f32 / (1u64 << 24) as f32
    }
}

fn write_metrics(
    config: &RunConfig,
    stats: &TargetStats,
    metrics: &Metrics,
    path: &Path,
    model_name: &str,
    parameters: Option<usize>,
) -> Result<(), Box<dyn Error>> {
    let mut writer = BufWriter::new(File::create(path)?);
    writeln!(writer, "{{")?;
    writeln!(writer, "  \"model\": \"{model_name}\",")?;
    writeln!(writer, "  \"input_features\": {INPUTS},")?;
    writeln!(writer, "  \"hidden_width\": {},", config.hidden_width)?;
    writeln!(writer, "  \"outputs\": {OUTPUTS},")?;
    if let Some(parameters) = parameters {
        writeln!(writer, "  \"parameters\": {parameters},")?;
    }
    writeln!(writer, "  \"epochs\": {},", config.epochs)?;
    writeln!(writer, "  \"batch_size\": {},", config.batch_size)?;
    writeln!(writer, "  \"learning_rate\": {},", config.learning_rate)?;
    writeln!(writer, "  \"train_limit\": {},", config.train_limit)?;
    writeln!(writer, "  \"val_limit\": {},", config.val_limit)?;
    writeln!(
        writer,
        "  \"target_names\": {},",
        json_string_array(&TARGET_NAMES)
    )?;
    writeln!(
        writer,
        "  \"target_mean\": {},",
        json_f32_array(&stats.mean)
    )?;
    writeln!(writer, "  \"target_std\": {},", json_f32_array(&stats.std))?;
    writeln!(writer, "  \"pinball_loss\": {:.6},", metrics.pinball_loss)?;
    writeln!(writer, "  \"mae\": {},", json_f64_array(&metrics.mae))?;
    writeln!(writer, "  \"rmse\": {},", json_f64_array(&metrics.rmse))?;
    writeln!(
        writer,
        "  \"coverage_p10_p90\": {},",
        json_f64_array(&metrics.coverage_p10_p90)
    )?;
    writeln!(writer, "  \"crossing_rate\": {:.6}", metrics.crossing_rate)?;
    writeln!(writer, "}}")?;
    Ok(())
}

fn write_model(model: &Mlp, stats: &TargetStats, path: &Path) -> Result<(), Box<dyn Error>> {
    let mut writer = BufWriter::new(File::create(path)?);
    writeln!(writer, "{{")?;
    writeln!(writer, "  \"model\": \"weather_mlp\",")?;
    writeln!(writer, "  \"activation\": \"silu\",")?;
    writeln!(writer, "  \"input_features\": {INPUTS},")?;
    writeln!(writer, "  \"hidden_width\": {},", model.hidden_width)?;
    writeln!(writer, "  \"outputs\": {OUTPUTS},")?;
    writeln!(
        writer,
        "  \"target_names\": {},",
        json_string_array(&TARGET_NAMES)
    )?;
    writeln!(
        writer,
        "  \"target_mean\": {},",
        json_f32_array(&stats.mean)
    )?;
    writeln!(writer, "  \"target_std\": {},", json_f32_array(&stats.std))?;
    writeln!(writer, "  \"w1\": {},", json_vec(&model.w1))?;
    writeln!(writer, "  \"b1\": {},", json_vec(&model.b1))?;
    writeln!(writer, "  \"w2\": {},", json_vec(&model.w2))?;
    writeln!(writer, "  \"b2\": {},", json_vec(&model.b2))?;
    writeln!(writer, "  \"w3\": {},", json_vec(&model.w3))?;
    writeln!(writer, "  \"b3\": {}", json_vec(&model.b3))?;
    writeln!(writer, "}}")?;
    Ok(())
}

fn json_vec(values: &[f32]) -> String {
    let mut output = String::from("[");
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push_str(&format!("{value:.8}"));
    }
    output.push(']');
    output
}

fn json_f32_array<const N: usize>(values: &[f32; N]) -> String {
    json_vec(values)
}

fn json_f64_array<const N: usize>(values: &[f64; N]) -> String {
    let mut output = String::from("[");
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push_str(&format!("{value:.6}"));
    }
    output.push(']');
    output
}

fn json_string_array(values: &[&str]) -> String {
    let mut output = String::from("[");
    for (index, value) in values.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        output.push('"');
        output.push_str(value);
        output.push('"');
    }
    output.push(']');
    output
}

const NASA_SOURCE_ID: &str = "nasa_power_hourly";
const NASA_SOURCE_RECORD_TYPE: &str = "historical";
const NASA_CSV_HEADER: &str = "source_id,source_record_type,location_id,timestamp_utc,latitude,longitude,elevation_m,ghi_w_m2,dni_w_m2,dhi_w_m2,ambient_temperature_c,wind_speed_m_s,wind_speed_height_m,quality_flags\n";
const NASA_FIELDS: [(&str, &str); 5] = [
    ("ALLSKY_SFC_SW_DWN", "ghi_w_m2"),
    ("ALLSKY_SFC_SW_DNI", "dni_w_m2"),
    ("ALLSKY_SFC_SW_DIFF", "dhi_w_m2"),
    ("T2M", "ambient_temperature_c"),
    ("WS2M", "wind_speed_m_s"),
];

#[derive(Debug)]
struct NormalizeConfig {
    raw_dir: PathBuf,
    out: PathBuf,
    workers: usize,
    pigz_threads: usize,
    keep_shards: bool,
}

impl NormalizeConfig {
    fn from_args(args: Vec<String>) -> Result<Self, Box<dyn Error>> {
        let default_workers = thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(4)
            .clamp(1, 16);
        let mut config = Self {
            raw_dir: PathBuf::from(
                "experiments/ml-weather/runs/global_grid_7056/raw/nasa_power_hourly",
            ),
            out: PathBuf::from(
                "experiments/ml-weather/runs/global_grid_7056/normalized/nasa_power_hourly.csv.gz",
            ),
            workers: default_workers,
            pigz_threads: 1,
            keep_shards: true,
        };

        let mut index = 0;
        while index < args.len() {
            let key = &args[index];
            match key.as_str() {
                "--keep-shards" => {
                    config.keep_shards = true;
                    index += 1;
                }
                "--discard-shards" => {
                    config.keep_shards = false;
                    index += 1;
                }
                "--raw-dir" | "--out" | "--workers" | "--pigz-threads" => {
                    let value = args
                        .get(index + 1)
                        .ok_or_else(|| format!("missing value for {key}"))?;
                    match key.as_str() {
                        "--raw-dir" => config.raw_dir = PathBuf::from(value),
                        "--out" => config.out = PathBuf::from(value),
                        "--workers" => config.workers = value.parse()?,
                        "--pigz-threads" => config.pigz_threads = value.parse()?,
                        _ => unreachable!(),
                    }
                    index += 2;
                }
                _ => return Err(format!("unknown normalize-nasa-power option: {key}").into()),
            }
        }

        if config.workers == 0 {
            return Err("--workers must be positive".into());
        }
        if config.pigz_threads == 0 {
            return Err("--pigz-threads must be positive".into());
        }
        Ok(config)
    }
}

#[derive(Debug)]
struct NormalizeShardSummary {
    index: usize,
    path: PathBuf,
    files: usize,
    rows: usize,
    missing_values: usize,
    bytes: u64,
}

#[derive(Debug, Default)]
struct NormalizeFileSummary {
    rows: usize,
    missing_values: usize,
}

fn normalize_nasa_power(config: NormalizeConfig) -> Result<(), Box<dyn Error>> {
    ensure_pigz_available()?;
    let mut raw_files = fs::read_dir(&config.raw_dir)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    raw_files.retain(|path| {
        path.extension()
            .is_some_and(|extension| extension == "json")
            && path.file_name().is_none_or(|name| name != "manifest.json")
    });
    raw_files.sort();
    if raw_files.is_empty() {
        return Err(format!(
            "no NASA POWER JSON files found in {}",
            config.raw_dir.display()
        )
        .into());
    }

    if let Some(parent) = config.out.parent() {
        fs::create_dir_all(parent)?;
    }

    if config.out.exists() {
        let backup = interrupted_output_path(&config.out);
        fs::rename(&config.out, &backup)?;
        println!(
            "moved existing {} to {}",
            config.out.display(),
            backup.display()
        );
    }

    let stamp = unix_timestamp();
    let out_name = config
        .out
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("--out must have a UTF-8 file name")?;
    let shard_dir = config
        .out
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(".{out_name}.shards-{stamp}"));
    fs::create_dir_all(&shard_dir)?;

    let workers = config.workers.min(raw_files.len()).max(1);
    let mut assignments = vec![Vec::new(); workers];
    for (index, path) in raw_files.into_iter().enumerate() {
        assignments[index % workers].push(path);
    }

    println!(
        "normalizing {} files with workers={} pigz_threads={} shard_dir={}",
        assignments.iter().map(Vec::len).sum::<usize>(),
        workers,
        config.pigz_threads,
        shard_dir.display()
    );

    let mut handles = Vec::with_capacity(workers);
    for (index, files) in assignments.into_iter().enumerate() {
        let shard_path = shard_dir.join(format!("part_{index:04}.csv.gz"));
        let pigz_threads = config.pigz_threads;
        handles.push(thread::spawn(move || {
            normalize_shard(index, files, shard_path, index == 0, pigz_threads)
                .map_err(|error| error.to_string())
        }));
    }

    let mut summaries = Vec::with_capacity(workers);
    for handle in handles {
        let summary = handle.join().map_err(|_| "normalizer worker panicked")??;
        println!(
            "shard={} files={} rows={} missing_values={} bytes={}",
            summary.index, summary.files, summary.rows, summary.missing_values, summary.bytes
        );
        summaries.push(summary);
    }
    summaries.sort_by_key(|summary| summary.index);

    let tmp_out = config.out.with_file_name(format!("{out_name}.tmp"));
    concatenate_shards(&summaries, &tmp_out)?;
    fs::rename(&tmp_out, &config.out)?;

    let rows = summaries.iter().map(|summary| summary.rows).sum::<usize>();
    let missing_values = summaries
        .iter()
        .map(|summary| summary.missing_values)
        .sum::<usize>();
    let files = summaries.iter().map(|summary| summary.files).sum::<usize>();
    let summary_path = config
        .out
        .with_file_name(format!("{out_name}.summary.json"));
    write_normalize_summary(
        &config,
        &summaries,
        rows,
        missing_values,
        files,
        &summary_path,
    )?;

    if !config.keep_shards {
        fs::remove_dir_all(&shard_dir)?;
    }

    println!("wrote {}", config.out.display());
    println!("wrote {}", summary_path.display());
    println!("files={files} rows={rows} missing_values={missing_values}");
    Ok(())
}

fn normalize_shard(
    index: usize,
    files: Vec<PathBuf>,
    shard_path: PathBuf,
    include_header: bool,
    pigz_threads: usize,
) -> Result<NormalizeShardSummary, Box<dyn Error>> {
    let shard_file = File::create(&shard_path)?;
    let mut child = Command::new("pigz")
        .arg("-c")
        .arg("-p")
        .arg(pigz_threads.to_string())
        .stdin(Stdio::piped())
        .stdout(Stdio::from(shard_file))
        .spawn()?;
    let stdin = child.stdin.take().ok_or("failed to open pigz stdin")?;
    let mut writer = BufWriter::with_capacity(1024 * 1024, stdin);

    if include_header {
        writer.write_all(NASA_CSV_HEADER.as_bytes())?;
    }

    let mut rows = 0usize;
    let mut missing_values = 0usize;
    for path in &files {
        let summary = normalize_nasa_power_file(path, &mut writer)?;
        rows += summary.rows;
        missing_values += summary.missing_values;
    }
    writer.flush()?;
    drop(writer);

    let status = child.wait()?;
    if !status.success() {
        return Err(format!("pigz exited with status {status} for shard {index}").into());
    }
    let bytes = shard_path.metadata()?.len();
    Ok(NormalizeShardSummary {
        index,
        path: shard_path,
        files: files.len(),
        rows,
        missing_values,
        bytes,
    })
}

fn normalize_nasa_power_file(
    path: &Path,
    writer: &mut impl Write,
) -> Result<NormalizeFileSummary, Box<dyn Error>> {
    let file = File::open(path)?;
    let data: Value = serde_json::from_reader(BufReader::new(file))?;
    let parameters = data
        .get("properties")
        .and_then(|properties| properties.get("parameter"))
        .and_then(Value::as_object)
        .ok_or_else(|| format!("missing properties.parameter in {}", path.display()))?;
    let coordinates = data
        .get("geometry")
        .and_then(|geometry| geometry.get("coordinates"))
        .and_then(Value::as_array)
        .ok_or_else(|| format!("missing geometry.coordinates in {}", path.display()))?;
    let longitude = coordinates
        .first()
        .map(json_value_to_csv)
        .ok_or_else(|| format!("missing longitude in {}", path.display()))?;
    let latitude = coordinates
        .get(1)
        .map(json_value_to_csv)
        .ok_or_else(|| format!("missing latitude in {}", path.display()))?;
    let elevation = coordinates
        .get(2)
        .map(json_value_to_csv)
        .unwrap_or_default();
    let location_id = location_id_from_json_path(path);

    let field_maps = NASA_FIELDS
        .iter()
        .map(|(source_field, _)| {
            parameters
                .get(*source_field)
                .and_then(Value::as_object)
                .ok_or_else(|| format!("missing parameter {source_field} in {}", path.display()))
        })
        .collect::<Result<Vec<&Map<String, Value>>, _>>()?;

    let mut keys = field_maps[0].keys().collect::<Vec<_>>();
    keys.sort_unstable();

    let prefix = format!("{NASA_SOURCE_ID},{NASA_SOURCE_RECORD_TYPE},{location_id},");
    let suffix_before_targets = format!(",{latitude},{longitude},{elevation}");
    let mut line = String::with_capacity(256);
    let mut rows = 0usize;
    let mut missing_values = 0usize;

    for key in keys {
        line.clear();
        line.push_str(&prefix);
        push_timestamp_utc(key, &mut line)?;
        line.push_str(&suffix_before_targets);

        let mut flags = Vec::new();
        for ((_, target_field), field_map) in NASA_FIELDS.iter().zip(&field_maps) {
            line.push(',');
            let value = field_map.get(key);
            if is_missing_value(value) {
                missing_values += 1;
                flags.push(*target_field);
            } else if let Some(value) = value {
                line.push_str(&json_value_to_csv(value));
            }
        }
        line.push_str(",2,");
        for (index, flag) in flags.iter().enumerate() {
            if index > 0 {
                line.push(';');
            }
            line.push_str("missing:");
            line.push_str(flag);
        }
        line.push('\n');
        writer.write_all(line.as_bytes())?;
        rows += 1;
    }

    Ok(NormalizeFileSummary {
        rows,
        missing_values,
    })
}

fn json_value_to_csv(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn is_missing_value(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::Number(number)) => number
            .as_f64()
            .is_some_and(|value| matches!(value as i64, -999 | -9999)),
        _ => false,
    }
}

fn push_timestamp_utc(key: &str, out: &mut String) -> Result<(), Box<dyn Error>> {
    if key.len() != 10 {
        return Err(format!("invalid NASA POWER timestamp key: {key}").into());
    }
    out.push_str(&key[0..4]);
    out.push('-');
    out.push_str(&key[4..6]);
    out.push('-');
    out.push_str(&key[6..8]);
    out.push('T');
    out.push_str(&key[8..10]);
    out.push_str(":00:00Z");
    Ok(())
}

fn location_id_from_json_path(path: &Path) -> String {
    let name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    let mut parts = name.rsplitn(3, '_').collect::<Vec<_>>();
    if parts.len() == 3
        && parts[0].chars().all(|char| char.is_ascii_digit())
        && parts[1].chars().all(|char| char.is_ascii_digit())
    {
        parts.reverse();
        parts[0].to_string()
    } else {
        name.to_string()
    }
}

fn concatenate_shards(
    summaries: &[NormalizeShardSummary],
    out: &Path,
) -> Result<(), Box<dyn Error>> {
    let mut writer = BufWriter::with_capacity(1024 * 1024, File::create(out)?);
    for summary in summaries {
        let mut reader = BufReader::with_capacity(1024 * 1024, File::open(&summary.path)?);
        io::copy(&mut reader, &mut writer)?;
    }
    writer.flush()?;
    Ok(())
}

fn write_normalize_summary(
    config: &NormalizeConfig,
    summaries: &[NormalizeShardSummary],
    rows: usize,
    missing_values: usize,
    files: usize,
    summary_path: &Path,
) -> Result<(), Box<dyn Error>> {
    let shards = summaries
        .iter()
        .map(|summary| {
            json!({
                "index": summary.index,
                "path": summary.path,
                "files": summary.files,
                "rows": summary.rows,
                "missing_values": summary.missing_values,
                "bytes": summary.bytes,
            })
        })
        .collect::<Vec<_>>();
    let summary = json!({
        "source_id": NASA_SOURCE_ID,
        "created_at_unix_seconds": unix_timestamp(),
        "raw_dir": config.raw_dir,
        "output_path": config.out,
        "workers": config.workers,
        "pigz_threads": config.pigz_threads,
        "files": files,
        "rows": rows,
        "missing_values": missing_values,
        "shards": shards,
    });
    fs::write(summary_path, serde_json::to_string_pretty(&summary)? + "\n")?;
    Ok(())
}

fn ensure_pigz_available() -> Result<(), Box<dyn Error>> {
    let status = Command::new("pigz")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err("pigz is not available or returned an error".into())
    }
}

fn interrupted_output_path(out: &Path) -> PathBuf {
    let stamp = unix_timestamp();
    let name = out
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("output");
    out.with_file_name(format!("{name}.interrupted_{stamp}"))
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

const GEONAMES_BASE_URL: &str = "https://download.geonames.org/export/dump";
const GEONAMES_CITIES_FILE: &str = "cities1000.zip";
const GEONAMES_CITIES_TXT: &str = "cities1000.txt";
const GEONAMES_DEFAULT_RAW_DIR: &str = "artifacts/geonames/raw";
const GEONAMES_DEFAULT_OUT_DIR: &str = "artifacts/geonames/curated";
const GEONAMES_SAMPLE_QUERIES: [&str; 5] = ["Roma", "Milan", "Miln", "Springfield", "Paris"];

fn geonames(args: Vec<String>) -> Result<(), Box<dyn Error>> {
    let Some((command, rest)) = args.split_first() else {
        print_geonames_help();
        return Ok(());
    };

    match command.as_str() {
        "fetch" => geonames_fetch(GeonamesFetchConfig::from_args(rest)?),
        "profile" => geonames_profile(GeonamesProfileConfig::from_args(rest)?),
        "report" => geonames_report(GeonamesReportConfig::from_args(rest)?),
        "help" => {
            print_geonames_help();
            Ok(())
        }
        other => Err(format!("unknown geonames command: {other}").into()),
    }
}

fn print_geonames_help() {
    println!("xtask geonames commands:");
    println!("  geonames fetch [--raw-dir <dir>] [--force]");
    println!("  geonames profile [--raw-dir <dir>] [--out-dir <dir>] [--max-rows <n>]");
    println!("  geonames report [--out-dir <dir>]");
}

#[derive(Debug)]
struct GeonamesFetchConfig {
    raw_dir: PathBuf,
    force: bool,
}

impl GeonamesFetchConfig {
    fn from_args(args: &[String]) -> Result<Self, Box<dyn Error>> {
        let mut config = Self {
            raw_dir: PathBuf::from(GEONAMES_DEFAULT_RAW_DIR),
            force: false,
        };
        let mut index = 0;
        while index < args.len() {
            match args[index].as_str() {
                "--raw-dir" => {
                    let value = args.get(index + 1).ok_or("missing value for --raw-dir")?;
                    config.raw_dir = PathBuf::from(value);
                    index += 2;
                }
                "--force" => {
                    config.force = true;
                    index += 1;
                }
                key => return Err(format!("unknown geonames fetch option: {key}").into()),
            }
        }
        Ok(config)
    }
}

#[derive(Debug)]
struct GeonamesProfileConfig {
    raw_dir: PathBuf,
    out_dir: PathBuf,
    max_rows: Option<usize>,
}

impl GeonamesProfileConfig {
    fn from_args(args: &[String]) -> Result<Self, Box<dyn Error>> {
        let mut config = Self {
            raw_dir: PathBuf::from(GEONAMES_DEFAULT_RAW_DIR),
            out_dir: PathBuf::from(GEONAMES_DEFAULT_OUT_DIR),
            max_rows: None,
        };
        let mut index = 0;
        while index < args.len() {
            let key = &args[index];
            match key.as_str() {
                "--raw-dir" | "--out-dir" | "--max-rows" => {
                    let value = args
                        .get(index + 1)
                        .ok_or_else(|| format!("missing value for {key}"))?;
                    match key.as_str() {
                        "--raw-dir" => config.raw_dir = PathBuf::from(value),
                        "--out-dir" => config.out_dir = PathBuf::from(value),
                        "--max-rows" => config.max_rows = Some(value.parse()?),
                        _ => unreachable!(),
                    }
                    index += 2;
                }
                _ => return Err(format!("unknown geonames profile option: {key}").into()),
            }
        }
        Ok(config)
    }
}

#[derive(Debug)]
struct GeonamesReportConfig {
    out_dir: PathBuf,
}

impl GeonamesReportConfig {
    fn from_args(args: &[String]) -> Result<Self, Box<dyn Error>> {
        let mut config = Self {
            out_dir: PathBuf::from(GEONAMES_DEFAULT_OUT_DIR),
        };
        let mut index = 0;
        while index < args.len() {
            match args[index].as_str() {
                "--out-dir" => {
                    let value = args.get(index + 1).ok_or("missing value for --out-dir")?;
                    config.out_dir = PathBuf::from(value);
                    index += 2;
                }
                key => return Err(format!("unknown geonames report option: {key}").into()),
            }
        }
        Ok(config)
    }
}

fn geonames_fetch(config: GeonamesFetchConfig) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(&config.raw_dir)?;
    let out = config.raw_dir.join(GEONAMES_CITIES_FILE);
    if out.exists() && !config.force {
        println!(
            "{} already exists; use --force to download again",
            out.display()
        );
        return Ok(());
    }

    let tmp = out.with_extension("zip.tmp");
    let url = format!("{GEONAMES_BASE_URL}/{GEONAMES_CITIES_FILE}");
    println!("downloading {url}");
    let status = Command::new("curl")
        .arg("--fail")
        .arg("--location")
        .arg("--show-error")
        .arg("--output")
        .arg(&tmp)
        .arg(&url)
        .status()?;
    if !status.success() {
        return Err(format!("curl exited with status {status}").into());
    }
    fs::rename(&tmp, &out)?;
    println!("wrote {}", out.display());
    Ok(())
}

fn geonames_profile(config: GeonamesProfileConfig) -> Result<(), Box<dyn Error>> {
    fs::create_dir_all(&config.out_dir)?;
    let zip_path = config.raw_dir.join(GEONAMES_CITIES_FILE);
    if !zip_path.exists() {
        return Err(format!(
            "missing {}; run `cargo run -p xtask -- geonames fetch` first",
            zip_path.display()
        )
        .into());
    }

    println!("reading {}", zip_path.display());
    let cities = load_geonames_cities(&zip_path, config.max_rows)?;
    if cities.is_empty() {
        return Err("GeoNames catalog is empty".into());
    }
    println!("loaded {} cities", cities.len());

    let variants = geonames_variant_specs();
    let mut variant_reports = Vec::with_capacity(variants.len());
    for spec in &variants {
        let encoded = encode_city_catalog(&cities, spec)?;
        let compressed = zstd::stream::encode_all(encoded.as_slice(), 19)?;
        let bin_path = config.out_dir.join(format!("{}.bin", spec.id));
        let zst_path = config.out_dir.join(format!("{}.bin.zst", spec.id));
        fs::write(&bin_path, &encoded)?;
        fs::write(&zst_path, &compressed)?;
        let search_results = GEONAMES_SAMPLE_QUERIES
            .iter()
            .map(|query| {
                let results = search_geonames_cities(&cities, spec, query, 5)
                    .into_iter()
                    .map(|result| {
                        json!({
                            "name": result.name,
                            "country_code": result.country_code,
                            "latitude": result.latitude,
                            "longitude": result.longitude,
                            "population": result.population,
                            "score": result.score,
                        })
                    })
                    .collect::<Vec<_>>();
                json!({ "query": query, "results": results })
            })
            .collect::<Vec<_>>();

        variant_reports.push(json!({
            "id": spec.id,
            "description": spec.description,
            "alias_cap": spec.alias_cap,
            "include_ascii_name": spec.include_ascii_name,
            "include_rank_fields": spec.include_rank_fields,
            "records": cities.len(),
            "uncompressed_bytes": encoded.len(),
            "zstd_bytes": compressed.len(),
            "search_samples": search_results,
        }));
        println!(
            "variant={} uncompressed={} zstd={}",
            spec.id,
            encoded.len(),
            compressed.len()
        );
    }

    let summary = json!({
        "source": "GeoNames cities1000.zip",
        "source_url": format!("{GEONAMES_BASE_URL}/{GEONAMES_CITIES_FILE}"),
        "created_at_unix_seconds": unix_timestamp(),
        "city_count": cities.len(),
        "variants": variant_reports,
    });
    let summary_path = config.out_dir.join("summary.json");
    fs::write(
        &summary_path,
        serde_json::to_string_pretty(&summary)? + "\n",
    )?;
    write_geonames_markdown_report(&summary, &config.out_dir.join("report.md"))?;
    println!("wrote {}", summary_path.display());
    println!("wrote {}", config.out_dir.join("report.md").display());
    Ok(())
}

fn geonames_report(config: GeonamesReportConfig) -> Result<(), Box<dyn Error>> {
    let report_path = config.out_dir.join("report.md");
    if report_path.exists() {
        let report = fs::read_to_string(&report_path)?;
        print!("{report}");
        return Ok(());
    }
    let summary_path = config.out_dir.join("summary.json");
    if !summary_path.exists() {
        return Err(format!(
            "missing {}; run `cargo run -p xtask -- geonames profile` first",
            summary_path.display()
        )
        .into());
    }
    let summary: Value = serde_json::from_reader(BufReader::new(File::open(&summary_path)?))?;
    write_geonames_markdown_report(&summary, &report_path)?;
    let report = fs::read_to_string(&report_path)?;
    print!("{report}");
    Ok(())
}

#[derive(Debug, Clone)]
struct GeonamesCity {
    geoname_id: u32,
    name: String,
    ascii_name: String,
    alternate_names: String,
    latitude: f64,
    longitude: f64,
    feature_code: String,
    country_code: String,
    population: u64,
}

#[derive(Debug, Clone, Copy)]
struct CityCatalogVariantSpec {
    id: &'static str,
    description: &'static str,
    include_ascii_name: bool,
    include_rank_fields: bool,
    alias_cap: usize,
}

#[derive(Debug, Clone)]
struct CitySearchResult {
    name: String,
    country_code: String,
    latitude: f64,
    longitude: f64,
    population: u64,
    score: usize,
}

fn geonames_variant_specs() -> Vec<CityCatalogVariantSpec> {
    vec![
        CityCatalogVariantSpec {
            id: "name_lat_lon",
            description: "display name and coordinates only",
            include_ascii_name: false,
            include_rank_fields: false,
            alias_cap: 0,
        },
        CityCatalogVariantSpec {
            id: "ascii_lat_lon",
            description: "display name, ASCII name, and coordinates",
            include_ascii_name: true,
            include_rank_fields: false,
            alias_cap: 0,
        },
        CityCatalogVariantSpec {
            id: "ranked_no_aliases",
            description: "ASCII name plus country, population, and feature code",
            include_ascii_name: true,
            include_rank_fields: true,
            alias_cap: 0,
        },
        CityCatalogVariantSpec {
            id: "ranked_aliases_3",
            description: "ranking fields plus up to 3 built-in aliases per city",
            include_ascii_name: true,
            include_rank_fields: true,
            alias_cap: 3,
        },
        CityCatalogVariantSpec {
            id: "ranked_aliases_6",
            description: "ranking fields plus up to 6 built-in aliases per city",
            include_ascii_name: true,
            include_rank_fields: true,
            alias_cap: 6,
        },
        CityCatalogVariantSpec {
            id: "ranked_aliases_10",
            description: "ranking fields plus up to 10 built-in aliases per city",
            include_ascii_name: true,
            include_rank_fields: true,
            alias_cap: 10,
        },
    ]
}

fn load_geonames_cities(
    zip_path: &Path,
    max_rows: Option<usize>,
) -> Result<Vec<GeonamesCity>, Box<dyn Error>> {
    let file = File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;
    let zip_file = archive.by_name(GEONAMES_CITIES_TXT)?;
    let mut reader = BufReader::with_capacity(1024 * 1024, zip_file);
    let mut line = String::new();
    let mut cities = Vec::new();

    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        if let Some(city) = parse_geonames_city(line.trim_end())? {
            cities.push(city);
            if max_rows.is_some_and(|limit| cities.len() >= limit) {
                break;
            }
        }
    }
    Ok(cities)
}

fn parse_geonames_city(line: &str) -> Result<Option<GeonamesCity>, Box<dyn Error>> {
    if line.trim().is_empty() {
        return Ok(None);
    }
    let fields = line.split('\t').collect::<Vec<_>>();
    if fields.len() < 19 {
        return Err(format!("invalid GeoNames city row with {} fields", fields.len()).into());
    }
    Ok(Some(GeonamesCity {
        geoname_id: fields[0].parse()?,
        name: fields[1].to_string(),
        ascii_name: fields[2].to_string(),
        alternate_names: fields[3].to_string(),
        latitude: fields[4].parse()?,
        longitude: fields[5].parse()?,
        feature_code: fields[7].to_string(),
        country_code: fields[8].to_string(),
        population: fields[14].parse().unwrap_or(0),
    }))
}

fn encode_city_catalog(
    cities: &[GeonamesCity],
    spec: &CityCatalogVariantSpec,
) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut out = Vec::with_capacity(cities.len() * 48);
    out.extend_from_slice(b"PVCITYCAT1\n");
    out.extend_from_slice(spec.id.as_bytes());
    out.push(0);
    out.push(u8::from(spec.include_ascii_name));
    out.push(u8::from(spec.include_rank_fields));
    write_u16(spec.alias_cap.try_into()?, &mut out);
    write_u32(cities.len().try_into()?, &mut out);

    for city in cities {
        write_u32(city.geoname_id, &mut out);
        write_string(&city.name, &mut out)?;
        if spec.include_ascii_name {
            write_string(&city.ascii_name, &mut out)?;
        }
        write_i32((city.latitude * 1_000_000.0).round() as i32, &mut out);
        write_i32((city.longitude * 1_000_000.0).round() as i32, &mut out);
        if spec.include_rank_fields {
            write_string(&city.country_code, &mut out)?;
            write_u32(city.population.min(u64::from(u32::MAX)) as u32, &mut out);
            write_string(&city.feature_code, &mut out)?;
        }
        let aliases = prune_city_aliases(city, spec.alias_cap);
        write_u16(aliases.len().try_into()?, &mut out);
        for alias in aliases {
            write_string(&alias, &mut out)?;
        }
    }
    Ok(out)
}

fn write_string(value: &str, out: &mut Vec<u8>) -> Result<(), Box<dyn Error>> {
    let bytes = value.as_bytes();
    let len: u16 = bytes
        .len()
        .try_into()
        .map_err(|_| format!("string is too long for catalog field: {value}"))?;
    write_u16(len, out);
    out.extend_from_slice(bytes);
    Ok(())
}

fn write_u16(value: u16, out: &mut Vec<u8>) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_u32(value: u32, out: &mut Vec<u8>) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_i32(value: i32, out: &mut Vec<u8>) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn prune_city_aliases(city: &GeonamesCity, alias_cap: usize) -> Vec<String> {
    if alias_cap == 0 || city.alternate_names.is_empty() {
        return Vec::new();
    }
    let mut seen = HashSet::new();
    seen.insert(normalize_search_text(&city.name));
    seen.insert(normalize_search_text(&city.ascii_name));

    let mut aliases = city
        .alternate_names
        .split(',')
        .filter_map(|alias| {
            let alias = alias.trim();
            if !is_useful_city_alias(alias) {
                return None;
            }
            let normalized = normalize_search_text(alias);
            if normalized.is_empty() || !seen.insert(normalized) {
                return None;
            }
            Some(alias.to_string())
        })
        .collect::<Vec<_>>();
    aliases.sort_by(|left, right| left.len().cmp(&right.len()).then_with(|| left.cmp(right)));
    aliases.truncate(alias_cap);
    aliases
}

fn is_useful_city_alias(alias: &str) -> bool {
    let chars = alias.chars().count();
    (2..=48).contains(&chars)
        && !alias.starts_with("http")
        && alias.chars().any(char::is_alphabetic)
        && alias
            .chars()
            .all(|char| !char.is_control() && char != '\t' && char != ',')
}

fn search_geonames_cities(
    cities: &[GeonamesCity],
    spec: &CityCatalogVariantSpec,
    query: &str,
    limit: usize,
) -> Vec<CitySearchResult> {
    let normalized_query = normalize_search_text(query);
    if normalized_query.is_empty() || limit == 0 {
        return Vec::new();
    }

    let mut results = cities
        .iter()
        .filter_map(|city| {
            let score = city_search_score(city, spec, &normalized_query)?;
            Some(CitySearchResult {
                name: city.name.clone(),
                country_code: if spec.include_rank_fields {
                    city.country_code.clone()
                } else {
                    String::new()
                },
                latitude: city.latitude,
                longitude: city.longitude,
                population: if spec.include_rank_fields {
                    city.population
                } else {
                    0
                },
                score,
            })
        })
        .collect::<Vec<_>>();
    results.sort_by(|left, right| {
        left.score
            .cmp(&right.score)
            .then_with(|| right.population.cmp(&left.population))
            .then_with(|| left.name.cmp(&right.name))
    });
    results.truncate(limit);
    results
}

fn city_search_score(
    city: &GeonamesCity,
    spec: &CityCatalogVariantSpec,
    normalized_query: &str,
) -> Option<usize> {
    let mut names = vec![city.name.as_str()];
    if spec.include_ascii_name {
        names.push(city.ascii_name.as_str());
    }
    let aliases = prune_city_aliases(city, spec.alias_cap);
    for alias in &aliases {
        names.push(alias.as_str());
    }
    names
        .into_iter()
        .filter_map(|name| name_match_score(name, normalized_query))
        .min()
}

fn name_match_score(name: &str, normalized_query: &str) -> Option<usize> {
    let normalized_name = normalize_search_text(name);
    if normalized_name == normalized_query {
        Some(0)
    } else if normalized_name.starts_with(normalized_query) {
        Some(100 + normalized_name.len().saturating_sub(normalized_query.len()))
    } else if normalized_name.contains(normalized_query) {
        Some(200 + normalized_name.len().saturating_sub(normalized_query.len()))
    } else {
        let distance = levenshtein_bounded(&normalized_name, normalized_query, 2)?;
        Some(300 + distance * 10 + normalized_name.len().abs_diff(normalized_query.len()))
    }
}

fn normalize_search_text(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|char| char.is_alphanumeric())
        .collect()
}

fn levenshtein_bounded(left: &str, right: &str, max_distance: usize) -> Option<usize> {
    if left.len().abs_diff(right.len()) > max_distance {
        return None;
    }
    let right_chars = right.chars().collect::<Vec<_>>();
    let mut previous = (0..=right_chars.len()).collect::<Vec<_>>();
    let mut current = vec![0; right_chars.len() + 1];

    for (left_index, left_char) in left.chars().enumerate() {
        current[0] = left_index + 1;
        let mut row_min = current[0];
        for (right_index, right_char) in right_chars.iter().enumerate() {
            let substitution_cost = usize::from(left_char != *right_char);
            current[right_index + 1] = (previous[right_index + 1] + 1)
                .min(current[right_index] + 1)
                .min(previous[right_index] + substitution_cost);
            row_min = row_min.min(current[right_index + 1]);
        }
        if row_min > max_distance {
            return None;
        }
        std::mem::swap(&mut previous, &mut current);
    }
    let distance = previous[right_chars.len()];
    (distance <= max_distance).then_some(distance)
}

fn write_geonames_markdown_report(summary: &Value, path: &Path) -> Result<(), Box<dyn Error>> {
    let mut writer = BufWriter::new(File::create(path)?);
    writeln!(writer, "# GeoNames City Catalog Profile")?;
    writeln!(writer)?;
    writeln!(
        writer,
        "Source: {}",
        summary
            .get("source_url")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
    )?;
    writeln!(
        writer,
        "Cities: {}",
        summary
            .get("city_count")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    )?;
    writeln!(writer)?;
    writeln!(writer, "| Variant | Records | Raw | zstd | Description |")?;
    writeln!(writer, "| --- | ---: | ---: | ---: | --- |")?;
    if let Some(variants) = summary.get("variants").and_then(Value::as_array) {
        for variant in variants {
            writeln!(
                writer,
                "| `{}` | {} | {} | {} | {} |",
                variant
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown"),
                variant.get("records").and_then(Value::as_u64).unwrap_or(0),
                format_bytes(
                    variant
                        .get("uncompressed_bytes")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                ),
                format_bytes(
                    variant
                        .get("zstd_bytes")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                ),
                variant
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            )?;
        }
    }
    writeln!(writer)?;
    writeln!(writer, "## Search Samples")?;
    if let Some(variants) = summary.get("variants").and_then(Value::as_array) {
        for variant in variants {
            writeln!(
                writer,
                "\n### `{}`",
                variant
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
            )?;
            if let Some(samples) = variant.get("search_samples").and_then(Value::as_array) {
                for sample in samples {
                    let query = sample.get("query").and_then(Value::as_str).unwrap_or("");
                    let names = sample
                        .get("results")
                        .and_then(Value::as_array)
                        .map(|results| {
                            results
                                .iter()
                                .map(|result| {
                                    let name = result
                                        .get("name")
                                        .and_then(Value::as_str)
                                        .unwrap_or("unknown");
                                    let country = result
                                        .get("country_code")
                                        .and_then(Value::as_str)
                                        .unwrap_or("");
                                    if country.is_empty() {
                                        name.to_string()
                                    } else {
                                        format!("{name} {country}")
                                    }
                                })
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_default();
                    writeln!(writer, "- `{query}`: {names}")?;
                }
            }
        }
    }
    Ok(())
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    if bytes >= 1024 * 1024 {
        format!("{:.2} MiB", bytes as f64 / MIB)
    } else if bytes >= 1024 {
        format!("{:.1} KiB", bytes as f64 / KIB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(test)]
mod geonames_tests {
    use super::*;

    fn city_line() -> &'static str {
        "3173435\tMilan\tMilan\tMilano,Mediolanum,Milan,MI\t45.46427\t9.18951\tP\tPPLA\tIT\t\t09\t015\t\t\t1236837\t122\t120\tEurope/Rome\t2025-01-01"
    }

    #[test]
    fn parses_geonames_city_line() {
        let city = parse_geonames_city(city_line())
            .expect("valid row")
            .expect("city row");
        assert_eq!(city.geoname_id, 3_173_435);
        assert_eq!(city.name, "Milan");
        assert_eq!(city.country_code, "IT");
        assert_eq!(city.feature_code, "PPLA");
        assert_eq!(city.population, 1_236_837);
        assert!((city.latitude - 45.46427).abs() < 0.000001);
    }

    #[test]
    fn prunes_aliases_with_deduplication_and_cap() {
        let city = parse_geonames_city(city_line())
            .expect("valid row")
            .expect("city row");
        let aliases = prune_city_aliases(&city, 2);
        assert_eq!(aliases, vec!["MI", "Milano"]);
    }

    #[test]
    fn city_catalog_encoding_is_deterministic() {
        let city = parse_geonames_city(city_line())
            .expect("valid row")
            .expect("city row");
        let spec = geonames_variant_specs()
            .into_iter()
            .find(|spec| spec.id == "ranked_aliases_3")
            .expect("variant exists");
        let left = encode_city_catalog(std::slice::from_ref(&city), &spec).expect("encode");
        let right = encode_city_catalog(&[city], &spec).expect("encode again");
        assert_eq!(left, right);
    }

    #[test]
    fn search_uses_aliases_and_fuzzy_matching() {
        let milan = parse_geonames_city(city_line())
            .expect("valid row")
            .expect("city row");
        let rome = parse_geonames_city(
            "3169070\tRome\tRome\tRoma,Rome\t41.89193\t12.51133\tP\tPPLC\tIT\t\t07\t058\t\t\t2318895\t20\t52\tEurope/Rome\t2025-01-01",
        )
        .expect("valid row")
        .expect("city row");
        let spec = geonames_variant_specs()
            .into_iter()
            .find(|spec| spec.id == "ranked_aliases_3")
            .expect("variant exists");
        let cities = vec![milan, rome];
        let roma = search_geonames_cities(&cities, &spec, "Roma", 1);
        assert_eq!(roma[0].name, "Rome");
        let miln = search_geonames_cities(&cities, &spec, "Miln", 1);
        assert_eq!(miln[0].name, "Milan");
    }
}
