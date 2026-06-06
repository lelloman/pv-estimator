use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

const TARGETS: usize = 5;
const MONTH_HOURS: usize = 12 * 24;
const DAY_HOURS: usize = 366 * 24;

#[derive(Debug)]
struct Config {
    data: PathBuf,
    out_dir: PathBuf,
    progress_every: usize,
    temporal_bins: TemporalBins,
}

#[derive(Debug, Clone, Copy)]
enum TemporalBins {
    MonthHour,
    DayHour,
}

impl TemporalBins {
    fn count(self) -> usize {
        match self {
            Self::MonthHour => MONTH_HOURS,
            Self::DayHour => DAY_HOURS,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::MonthHour => "month_hour",
            Self::DayHour => "day_hour",
        }
    }
}

#[derive(Clone, Copy)]
struct RunningBin {
    count: u32,
    sum: [f64; TARGETS],
    sum_sq: [f64; TARGETS],
}

impl Default for RunningBin {
    fn default() -> Self {
        Self {
            count: 0,
            sum: [0.0; TARGETS],
            sum_sq: [0.0; TARGETS],
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let config = parse_args()?;
    fs::create_dir_all(&config.out_dir)?;
    let started = Instant::now();
    let mut child = Command::new("gzip")
        .arg("-cd")
        .arg(&config.data)
        .stdout(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().ok_or("failed to capture gzip stdout")?;
    let mut reader = BufReader::with_capacity(1024 * 1024, stdout);
    let mut line = String::new();
    reader.read_line(&mut line)?;

    let mut location_to_index: HashMap<i64, usize> = HashMap::new();
    let mut location_keys: Vec<i64> = Vec::new();
    let temporal_bin_count = config.temporal_bins.count();
    let mut bins: Vec<RunningBin> = Vec::new();
    let mut rows = 0usize;
    let mut skipped = 0usize;

    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        rows += 1;
        let Some(parsed) = parse_row(line.trim_end()) else {
            skipped += 1;
            continue;
        };
        let location_index = match location_to_index.get(&parsed.location_key) {
            Some(index) => *index,
            None => {
                let index = location_keys.len();
                location_to_index.insert(parsed.location_key, index);
                location_keys.push(parsed.location_key);
                bins.extend((0..temporal_bin_count).map(|_| RunningBin::default()));
                index
            }
        };
        let temporal_index = match config.temporal_bins {
            TemporalBins::MonthHour => parsed.month_hour,
            TemporalBins::DayHour => parsed.day_hour,
        };
        let bin_index = location_index * temporal_bin_count + temporal_index;
        let bin = &mut bins[bin_index];
        bin.count = bin.count.saturating_add(1);
        for target in 0..TARGETS {
            let value = f64::from(parsed.targets[target]);
            bin.sum[target] += value;
            bin.sum_sq[target] += value * value;
        }
        if rows == 1 || rows % config.progress_every == 0 {
            let elapsed = started.elapsed().as_secs_f64();
            let rate = rows as f64 / elapsed.max(0.001);
            println!(
                "rows={rows} skipped={skipped} locations={} rate={rate:.0}/s elapsed_min={:.1}",
                location_keys.len(),
                elapsed / 60.0,
            );
        }
    }
    drop(reader);
    let status = child.wait()?;
    if !status.success() {
        return Err(format!("gzip exited with status {status}").into());
    }

    let location_count = location_keys.len();
    let mut means = vec![0.0f32; location_count * temporal_bin_count * TARGETS];
    let mut stds = vec![1.0f32; location_count * temporal_bin_count * TARGETS];
    let mut counts = vec![0i32; location_count * temporal_bin_count];
    let mut populated = 0usize;
    for location in 0..location_count {
        for temporal_index in 0..temporal_bin_count {
            let bin = bins[location * temporal_bin_count + temporal_index];
            counts[location * temporal_bin_count + temporal_index] = bin.count as i32;
            if bin.count == 0 {
                continue;
            }
            populated += 1;
            let count = f64::from(bin.count);
            for target in 0..TARGETS {
                let mean = bin.sum[target] / count;
                let variance = (bin.sum_sq[target] / count - mean * mean).max(1.0e-6);
                let out_index = (location * temporal_bin_count + temporal_index) * TARGETS + target;
                means[out_index] = mean as f32;
                stds[out_index] = variance.sqrt() as f32;
            }
        }
    }

    write_npy_f32(&config.out_dir.join("climate_normals.npy"), &[location_count, temporal_bin_count, TARGETS], &means)?;
    write_npy_f32(&config.out_dir.join("climate_normal_std.npy"), &[location_count, temporal_bin_count, TARGETS], &stds)?;
    write_npy_i32(&config.out_dir.join("climate_normal_counts.npy"), &[location_count, temporal_bin_count], &counts)?;
    write_npy_i64(&config.out_dir.join("location_keys.npy"), &[location_count], &location_keys)?;
    let metadata = format!(
        "{{\n  \"source_csv\": \"{}\",\n  \"rows\": {},\n  \"skipped_rows\": {},\n  \"location_count\": {},\n  \"temporal_bins\": \"{}\",\n  \"temporal_bin_count\": {},\n  \"target_count\": {},\n  \"populated_bins\": {},\n  \"elapsed_seconds\": {:.3}\n}}\n",
        escape_json(&config.data.display().to_string()),
        rows,
        skipped,
        location_count,
        config.temporal_bins.label(),
        temporal_bin_count,
        TARGETS,
        populated,
        started.elapsed().as_secs_f64(),
    );
    fs::write(config.out_dir.join("metadata.json"), metadata)?;
    println!(
        "wrote {} locations={} rows={} skipped={} populated_bins={} elapsed_min={:.1}",
        config.out_dir.display(),
        location_count,
        rows,
        skipped,
        populated,
        started.elapsed().as_secs_f64() / 60.0,
    );
    Ok(())
}

struct ParsedRow {
    location_key: i64,
    month_hour: usize,
    day_hour: usize,
    targets: [f32; TARGETS],
}

fn parse_row(line: &str) -> Option<ParsedRow> {
    let mut fields = line.split(',');
    let _source_id = fields.next()?;
    let _source_record_type = fields.next()?;
    let _location_id = fields.next()?;
    let timestamp = fields.next()?;
    let latitude = fields.next()?.parse::<f64>().ok()?;
    let longitude = fields.next()?.parse::<f64>().ok()?;
    let _elevation = fields.next()?;
    let targets = [
        fields.next()?.parse::<f32>().ok()?,
        fields.next()?.parse::<f32>().ok()?,
        fields.next()?.parse::<f32>().ok()?,
        fields.next()?.parse::<f32>().ok()?,
        fields.next()?.parse::<f32>().ok()?,
    ];
    let (month, day_of_year, hour) = parse_temporal(timestamp)?;
    Some(ParsedRow {
        location_key: pack_location_key(latitude, longitude),
        month_hour: month * 24 + hour,
        day_hour: (day_of_year - 1) * 24 + hour,
        targets,
    })
}

fn parse_temporal(timestamp: &str) -> Option<(usize, usize, usize)> {
    if timestamp.len() < 13 {
        return None;
    }
    let year = timestamp[0..4].parse::<i32>().ok()?;
    let month_one_based = timestamp[5..7].parse::<usize>().ok()?;
    let day = timestamp[8..10].parse::<usize>().ok()?;
    let hour = timestamp[11..13].parse::<usize>().ok()?;
    if !(1..=12).contains(&month_one_based) || hour >= 24 {
        return None;
    }
    let days_before_month_common = [0usize, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let mut day_of_year = days_before_month_common[month_one_based - 1] + day;
    if month_one_based > 2 && is_leap_year(year) {
        day_of_year += 1;
    }
    if !(1..=366).contains(&day_of_year) {
        return None;
    }
    Some((month_one_based - 1, day_of_year, hour))
}

fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn pack_location_key(latitude: f64, longitude: f64) -> i64 {
    let lat_key = (latitude * 1000.0).round() as i64;
    let lon_key = (longitude * 1000.0).round() as i64;
    ((lat_key + 90_000) << 32) | (lon_key + 180_000)
}

fn parse_args() -> Result<Config, Box<dyn Error>> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let mut config = Config {
        data: PathBuf::from("data/nasa_power_hourly_global_grid_7056.csv.gz"),
        out_dir: PathBuf::from("runs/full_climate_normals"),
        progress_every: 10_000_000,
        temporal_bins: TemporalBins::MonthHour,
    };
    let mut index = 0;
    while index < args.len() {
        let key = &args[index];
        let value = args.get(index + 1).ok_or_else(|| format!("missing value for {key}"))?;
        match key.as_str() {
            "--data" => config.data = PathBuf::from(value),
            "--out-dir" => config.out_dir = PathBuf::from(value),
            "--progress-every" => config.progress_every = value.parse()?,
            "--temporal-bins" => {
                config.temporal_bins = match value.as_str() {
                    "month-hour" => TemporalBins::MonthHour,
                    "day-hour" => TemporalBins::DayHour,
                    _ => return Err(format!("unsupported --temporal-bins: {value}").into()),
                };
            }
            _ => return Err(format!("unknown option: {key}").into()),
        }
        index += 2;
    }
    Ok(config)
}

fn write_npy_f32(path: &Path, shape: &[usize], values: &[f32]) -> Result<(), Box<dyn Error>> {
    let mut writer = npy_writer(path, "<f4", shape)?;
    for value in values {
        writer.write_all(&value.to_le_bytes())?;
    }
    Ok(())
}

fn write_npy_i32(path: &Path, shape: &[usize], values: &[i32]) -> Result<(), Box<dyn Error>> {
    let mut writer = npy_writer(path, "<i4", shape)?;
    for value in values {
        writer.write_all(&value.to_le_bytes())?;
    }
    Ok(())
}

fn write_npy_i64(path: &Path, shape: &[usize], values: &[i64]) -> Result<(), Box<dyn Error>> {
    let mut writer = npy_writer(path, "<i8", shape)?;
    for value in values {
        writer.write_all(&value.to_le_bytes())?;
    }
    Ok(())
}

fn npy_writer(path: &Path, descr: &str, shape: &[usize]) -> Result<BufWriter<File>, Box<dyn Error>> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);
    let shape_text = if shape.len() == 1 {
        format!("({},)", shape[0])
    } else {
        format!("({})", shape.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", "))
    };
    let mut header = format!("{{'descr': '{}', 'fortran_order': False, 'shape': {}, }}", descr, shape_text).into_bytes();
    let prefix_len = 10usize;
    let padding = (16 - ((prefix_len + header.len() + 1) % 16)) % 16;
    header.extend(std::iter::repeat(b' ').take(padding));
    header.push(b'\n');
    if header.len() > u16::MAX as usize {
        return Err("npy header too large".into());
    }
    writer.write_all(b"\x93NUMPY")?;
    writer.write_all(&[1, 0])?;
    writer.write_all(&(header.len() as u16).to_le_bytes())?;
    writer.write_all(&header)?;
    Ok(writer)
}

fn escape_json(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
