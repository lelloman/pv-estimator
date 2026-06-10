use std::fmt::Write as _;
use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use pv_data::{CityMatchKind, CitySearchResult};
use pv_model::{EstimateArray, EstimateRequest, SourceModelEstimator, format_table};
use serde::Serialize;

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
    Search(SearchArgs),
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
    #[arg(long = "storage-kwh")]
    storage_kwh: Option<f64>,
    #[arg(long = "array", value_name = "KWP,TILT,AZIMUTH")]
    arrays: Vec<String>,
    #[arg(long)]
    model_dir: Option<PathBuf>,
    #[arg(long, default_value = "source-model-artifacts.json")]
    manifest: String,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    format: OutputFormat,
}

#[derive(Debug, Parser)]
struct SearchArgs {
    query: String,
    #[arg(long, default_value_t = 10)]
    limit: usize,
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    format: OutputFormat,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat {
    Table,
    Json,
}

#[derive(Debug, Serialize)]
struct CitySearchJsonRow {
    geoname_id: u32,
    display_name: String,
    country_code: String,
    latitude: f64,
    longitude: f64,
    population: u32,
    feature_code: String,
    matched_name: String,
    match_kind: &'static str,
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
        Command::Search(args) => search(args),
    }
}

fn estimate_arrays(args: &EstimateArgs) -> Result<Vec<EstimateArray>> {
    if args.arrays.is_empty() {
        return Ok(vec![EstimateArray {
            peak_power_kwp: args.kwp,
            tilt_deg: args.tilt_deg,
            azimuth_deg: args.azimuth_deg,
        }]);
    }

    let arrays = args
        .arrays
        .iter()
        .flat_map(|value| value.split(';'))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .enumerate()
        .map(|(index, value)| parse_array_arg(index + 1, value))
        .collect::<Result<Vec<_>>>()?;
    if arrays.is_empty() {
        bail!("at least one --array entry is required");
    }
    Ok(arrays)
}

fn parse_array_arg(index: usize, value: &str) -> Result<EstimateArray> {
    let parts = value.split(',').map(str::trim).collect::<Vec<_>>();
    if parts.len() != 3 {
        bail!("array {index} must be KWP,TILT,AZIMUTH");
    }
    Ok(EstimateArray {
        peak_power_kwp: parts[0]
            .parse::<f64>()
            .map_err(|_| anyhow::anyhow!("array {index} KWP must be a number"))?,
        tilt_deg: parts[1]
            .parse::<f64>()
            .map_err(|_| anyhow::anyhow!("array {index} TILT must be a number"))?,
        azimuth_deg: parts[2]
            .parse::<f64>()
            .map_err(|_| anyhow::anyhow!("array {index} AZIMUTH must be a number"))?,
    })
}

fn estimate(args: EstimateArgs) -> Result<()> {
    let arrays = estimate_arrays(&args)?;
    let first_array = arrays[0];
    let request = EstimateRequest {
        latitude: args.lat,
        longitude: args.lon,
        location_id: args.location_id,
        name: args.name,
        region: args.region,
        peak_power_kwp: first_array.peak_power_kwp,
        loss_pct: args.loss_pct,
        tilt_deg: first_array.tilt_deg,
        azimuth_deg: first_array.azimuth_deg,
        storage_usable_kwh: args.storage_kwh,
    };
    let mut estimator = match &args.model_dir {
        Some(model_dir) => SourceModelEstimator::load(model_dir, &args.manifest)?,
        None => SourceModelEstimator::load_embedded()?,
    };
    let document = estimator.estimate_arrays(&request, &arrays)?;
    match args.format {
        OutputFormat::Json => write_json(&document),
        OutputFormat::Table => {
            print!("{}", format_table(&document));
            Ok(())
        }
    }
}

fn search(args: SearchArgs) -> Result<()> {
    let query = args.query.trim();
    if query.chars().count() < 2 {
        bail!("search query must contain at least 2 characters");
    }
    if !(1..=50).contains(&args.limit) {
        bail!("search limit must be in 1..=50");
    }

    let results = pv_data::search_cities(query, args.limit);
    match args.format {
        OutputFormat::Json => write_city_search_json(&results),
        OutputFormat::Table => {
            print!("{}", format_city_search_table(&results));
            Ok(())
        }
    }
}

fn write_json(document: &pv_core::source_model::SourceEnsembleEstimateDocument) -> Result<()> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, document)?;
    writeln!(stdout)?;
    Ok(())
}

fn write_city_search_json(results: &[CitySearchResult]) -> Result<()> {
    let rows = results.iter().map(city_search_json_row).collect::<Vec<_>>();
    let mut stdout = io::stdout().lock();
    serde_json::to_writer_pretty(&mut stdout, &rows)?;
    writeln!(stdout)?;
    Ok(())
}

fn city_search_json_row(result: &CitySearchResult) -> CitySearchJsonRow {
    CitySearchJsonRow {
        geoname_id: result.geoname_id,
        display_name: result.display_name.clone(),
        country_code: result.country_code.clone(),
        latitude: result.latitude_degrees,
        longitude: result.longitude_degrees,
        population: result.population,
        feature_code: result.feature_code.clone(),
        matched_name: result.matched_name.clone(),
        match_kind: city_match_kind_label(result.match_kind),
    }
}

fn format_city_search_table(results: &[CitySearchResult]) -> String {
    let mut output = String::new();
    writeln!(
        &mut output,
        "{:<32} {:<7} {:>10} {:>11} {:>12} {:<18}",
        "name", "country", "latitude", "longitude", "population", "match kind"
    )
    .expect("writing string");
    for result in results {
        writeln!(
            &mut output,
            "{:<32} {:<7} {:>10.4} {:>11.4} {:>12} {:<18}",
            truncate(&result.display_name, 32),
            result.country_code,
            result.latitude_degrees,
            result.longitude_degrees,
            result.population,
            city_match_kind_label(result.match_kind),
        )
        .expect("writing string");
    }
    output
}

fn city_match_kind_label(kind: CityMatchKind) -> &'static str {
    match kind {
        CityMatchKind::ExactPrimary => "exact_primary",
        CityMatchKind::ExactAlias => "exact_alias",
        CityMatchKind::PrefixPrimary => "prefix_primary",
        CityMatchKind::PrefixAlias => "prefix_alias",
        CityMatchKind::SubstringPrimary => "substring_primary",
        CityMatchKind::SubstringAlias => "substring_alias",
        CityMatchKind::FuzzyPrimary => "fuzzy_primary",
        CityMatchKind::FuzzyAlias => "fuzzy_alias",
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        truncated
    } else {
        value.to_string()
    }
}
