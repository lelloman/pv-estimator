use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use pv_model::{EstimateRequest, SourceModelEstimator, format_table};

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
    let request = EstimateRequest {
        latitude: args.lat,
        longitude: args.lon,
        location_id: args.location_id,
        name: args.name,
        region: args.region,
        peak_power_kwp: args.kwp,
        loss_pct: args.loss_pct,
        tilt_deg: args.tilt_deg,
        azimuth_deg: args.azimuth_deg,
    };
    let mut estimator = SourceModelEstimator::load(&args.model_dir, &args.manifest)?;
    let document = estimator.estimate(&request)?;
    match args.format {
        OutputFormat::Json => write_json(&document),
        OutputFormat::Table => {
            print!("{}", format_table(&document));
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
