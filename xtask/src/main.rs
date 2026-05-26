use std::env;
use std::error::Error;
use std::f32::consts::PI;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
