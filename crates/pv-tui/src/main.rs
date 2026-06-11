use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
    MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode, size,
};
use directories::ProjectDirs;
use pv_core::simulation::{
    BuiltInLoadShapeId, LoadProfile, LoadShape, MetricSummary, SimulationMetricSummaries,
    SimulationOptions, SimulationRequest, SimulationResult, StorageConfig, simulate_with_progress,
};
use pv_core::source_model::SourceEnsembleEstimateDocument;
use pv_data::CitySearchResult;
use pv_model::{
    EstimateArray, EstimateRequest, SourceModelEstimator, days_in_month, short_month_name,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap};
use serde::{Deserialize, Serialize};

#[derive(Debug, Parser)]
#[command(name = "pv-tui")]
#[command(about = "Interactive PV estimator terminal UI")]
struct Args {
    #[arg(long)]
    model_dir: Option<PathBuf>,
    #[arg(long, default_value = "source-model-artifacts.json")]
    manifest: String,
}

const TUI_STATE_SCHEMA_VERSION: u32 = 1;
const PRICE_FIELD_INDEX: usize = 5;
const STORAGE_FIELD_INDEX: usize = 6;
const ARRAY_FIELD_INDEX: usize = 7;
const FIELD_LABEL_WIDTH: u16 = 13;
const CONSUMER_ANNUAL_FIELD_INDEX: usize = 0;
const CONSUMER_DAILY_FIELD_INDEX: usize = 1;
const CONSUMER_SHAPE_FIELD_INDEX: usize = 2;
const SIMULATION_RUNS_FIELD_INDEX: usize = 0;
const SIMULATION_SEED_FIELD_INDEX: usize = 1;
const SIMULATION_RUN_ROW_INDEX: usize = 2;
const ESTIMATE_LABEL_WIDTH: usize = 11;
const SEARCH_LABEL_WIDTH: u16 = 8;
const LOCATION_RESULT_HEADER_ROWS: u16 = 3;
const ARRAY_EDITOR_HEADER_ROWS: u16 = 3;
const ARRAY_TABLE_WIDTHS: [u16; 9] = [4, 1, 8, 1, 8, 1, 9, 1, 9];
const ARRAY_CELL_WIDTHS: [usize; 3] = [8, 8, 9];
const ARRAY_CELL_STARTS: [u16; 3] = [7, 18, 29];
const SHAPE_EDITOR_HEADER_ROWS: u16 = 4;
const SHAPE_EDITOR_COLUMNS: usize = 4;
const SHAPE_EDITOR_ROWS: usize = 6;
const SHAPE_CELL_WIDTH: usize = 13;
const SHAPE_VALUE_WIDTH: usize = 8;
const SHAPE_CELL_STARTS: [u16; SHAPE_EDITOR_COLUMNS] = [0, 18, 36, 54];
const SHAPE_VALUE_OFFSET: u16 = 4;
const SHAPE_PRESET_PICKER_WIDTH: u16 = 28;
const SHAPE_PRESET_PICKER_HEIGHT: u16 = 6;
const BUILT_IN_LOAD_SHAPES: [BuiltInLoadShapeId; 4] = [
    BuiltInLoadShapeId::ResidentialDefault,
    BuiltInLoadShapeId::Flat,
    BuiltInLoadShapeId::Daytime,
    BuiltInLoadShapeId::Evening,
];
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
const MONTHLY_TABLE_HEADERS: [&str; 7] = ["Month", "mean", "min", "max", "mean", "min", "max"];
const MONTHLY_TABLE_HEADER_ROWS: usize = 3;
const ESTIMATE_SCROLL_PAGE_ROWS: usize = 6;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TuiState {
    schema_version: u32,
    selected_location_id: String,
    location_query: String,
    fields: Vec<TuiFieldState>,
    #[serde(default)]
    consumer_fields: Vec<TuiFieldState>,
    #[serde(default)]
    consumer_shape: ConsumerShapeState,
    #[serde(default)]
    simulation_fields: Vec<TuiFieldState>,
    #[serde(default)]
    panel_visibility: PanelVisibility,
    #[serde(default)]
    focused_panel: Panel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TuiFieldState {
    label: String,
    value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Panel {
    System,
    Consumer,
    Simulation,
    Estimate,
}

impl Default for Panel {
    fn default() -> Self {
        Self::System
    }
}

impl Panel {
    const ALL: [Self; 4] = [
        Self::System,
        Self::Consumer,
        Self::Simulation,
        Self::Estimate,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Consumer => "Consumer",
            Self::Simulation => "Simulation",
            Self::Estimate => "Estimate",
        }
    }

    fn toggle_key(self) -> char {
        match self {
            Self::System => '1',
            Self::Consumer => '2',
            Self::Simulation => '3',
            Self::Estimate => '4',
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct PanelVisibility {
    system: bool,
    consumer: bool,
    simulation: bool,
    estimate: bool,
}

impl Default for PanelVisibility {
    fn default() -> Self {
        Self {
            system: true,
            consumer: false,
            simulation: false,
            estimate: true,
        }
    }
}

impl PanelVisibility {
    fn is_visible(self, panel: Panel) -> bool {
        match panel {
            Panel::System => self.system,
            Panel::Consumer => self.consumer,
            Panel::Simulation => self.simulation,
            Panel::Estimate => self.estimate,
        }
    }

    fn set_visible(&mut self, panel: Panel, visible: bool) {
        match panel {
            Panel::System => self.system = visible,
            Panel::Consumer => self.consumer = visible,
            Panel::Simulation => self.simulation = visible,
            Panel::Estimate => self.estimate = visible,
        }
    }

    fn visible_count(self) -> usize {
        Panel::ALL
            .iter()
            .filter(|panel| self.is_visible(**panel))
            .count()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    Edit,
    Location,
    Arrays,
    Shape,
    SimulationRun,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ConsumerShapeState {
    BuiltIn { shape_id: BuiltInLoadShapeId },
    HourlyWeights { weights: Vec<f64> },
}

impl Default for ConsumerShapeState {
    fn default() -> Self {
        Self::BuiltIn {
            shape_id: BuiltInLoadShapeId::ResidentialDefault,
        }
    }
}

impl ConsumerShapeState {
    fn label(&self) -> String {
        match self {
            Self::BuiltIn { shape_id } => format!("[Edit]  {}", built_in_shape_label(*shape_id)),
            Self::HourlyWeights { .. } => "[Edit]  custom hourly".to_string(),
        }
    }

    fn mode_label(&self) -> &'static str {
        match self {
            Self::BuiltIn { .. } => "Preset",
            Self::HourlyWeights { .. } => "Custom",
        }
    }

    fn load_shape(&self) -> Result<LoadShape> {
        match self {
            Self::BuiltIn { shape_id } => Ok(LoadShape::BuiltIn {
                shape_id: *shape_id,
            }),
            Self::HourlyWeights { weights } => {
                validate_shape_weights(weights)?;
                Ok(LoadShape::HourlyWeights {
                    weights: weights.clone(),
                })
            }
        }
    }

    fn weights(&self) -> Vec<f64> {
        match self {
            Self::BuiltIn { shape_id } => built_in_shape_weights(*shape_id).to_vec(),
            Self::HourlyWeights { weights } => weights.clone(),
        }
    }
}

#[derive(Debug)]
struct Field {
    label: &'static str,
    value: String,
    cursor: usize,
}

#[derive(Debug)]
struct App {
    fields: Vec<Field>,
    consumer_fields: Vec<Field>,
    simulation_fields: Vec<Field>,
    selected: usize,
    consumer_selected: usize,
    simulation_selected: usize,
    mode: Mode,
    status: String,
    estimate: Option<SourceEnsembleEstimateDocument>,
    simulation_result: Option<SimulationResult>,
    simulation_run: Option<SimulationRunState>,
    selected_location_id: String,
    location_query: Field,
    location_results: Vec<CitySearchResult>,
    location_selected: usize,
    array_selected: usize,
    array_column: usize,
    array_editing: bool,
    array_cell: Field,
    consumer_shape: ConsumerShapeState,
    shape_selected_hour: usize,
    shape_editing: bool,
    shape_preset_selecting: bool,
    shape_preset_selected: usize,
    shape_cell: Field,
    estimate_scroll: usize,
    panel_visibility: PanelVisibility,
    focused_panel: Panel,
}

#[derive(Debug)]
struct SimulationRunState {
    requested_runs: usize,
    completed_runs: Arc<AtomicUsize>,
    cancel: Arc<AtomicBool>,
    started_at: Instant,
    finished_at: Option<Instant>,
    receiver: mpsc::Receiver<Result<SimulationResult, String>>,
    finished: bool,
    cancelling: bool,
    error: Option<String>,
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = Args::parse();
    let mut estimator = match &args.model_dir {
        Some(model_dir) => SourceModelEstimator::load(model_dir, &args.manifest)
            .with_context(|| format!("loading model artifacts from {}", model_dir.display()))?,
        None => {
            SourceModelEstimator::load_embedded().context("loading embedded model artifacts")?
        }
    };

    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    let _guard = TerminalGuard;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut app = App::new();
    app.load_saved_state();
    app.recompute(&mut estimator);
    run_app(&mut terminal, &mut app, &mut estimator)?;
    app.save_state();
    terminal.show_cursor()?;
    Ok(())
}

fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    estimator: &mut SourceModelEstimator,
) -> Result<()> {
    loop {
        app.poll_simulation_run();
        terminal.draw(|frame| render(frame, app))?;
        if event::poll(Duration::from_millis(200))? {
            match event::read()? {
                Event::Key(key) if handle_key(key, app, estimator)? => break,
                Event::Mouse(mouse) => handle_mouse(mouse, app, estimator)?,
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
    Ok(())
}

impl App {
    fn new() -> Self {
        let _ = thread::spawn(|| {
            let _ = pv_data::city_catalog_metadata();
        });
        Self {
            fields: vec![
                Field::new("Name", "Custom location"),
                Field::new("Region", ""),
                Field::new("Latitude", "40.650"),
                Field::new("Longitude", "15.643"),
                Field::new("Loss %", "14.0"),
                Field::new("EUR/kWh", ""),
                Field::new("Storage kWh", ""),
                Field::new("Arrays", "1.0,30.0,0.0"),
            ],
            consumer_fields: vec![
                Field::new("Annual kWh", "4200"),
                Field::new("Daily kWh", ""),
                Field::new("Shape", "residential_default"),
            ],
            simulation_fields: vec![Field::new("Runs", "10000"), Field::new("Seed", "")],
            selected: 2,
            consumer_selected: 0,
            simulation_selected: 0,
            mode: Mode::Normal,
            status: "Ready".to_string(),
            estimate: None,
            simulation_result: None,
            simulation_run: None,
            selected_location_id: "custom".to_string(),
            location_query: Field::new("Find", ""),
            location_results: Vec::new(),
            location_selected: 0,
            array_selected: 0,
            array_column: 0,
            array_editing: false,
            array_cell: Field::new("Array", ""),
            consumer_shape: ConsumerShapeState::default(),
            shape_selected_hour: 0,
            shape_editing: false,
            shape_preset_selecting: false,
            shape_preset_selected: 0,
            shape_cell: Field::new("Weight", ""),
            estimate_scroll: 0,
            panel_visibility: PanelVisibility::default(),
            focused_panel: Panel::System,
        }
    }

    fn recompute(&mut self, estimator: &mut SourceModelEstimator) {
        self.simulation_result = None;
        match self
            .energy_price_eur_per_kwh()
            .and_then(|_| self.request_and_arrays())
            .and_then(|(request, arrays)| estimator.estimate_arrays(&request, &arrays))
        {
            Ok(document) => {
                self.status = "Estimate updated".to_string();
                self.estimate = Some(document);
                self.save_state();
            }
            Err(error) => {
                self.status = format!("{error:#}");
            }
        }
    }

    fn load_saved_state(&mut self) {
        let Some(path) = tui_state_path() else {
            return;
        };
        let Ok(bytes) = fs::read(&path) else {
            return;
        };
        let Ok(state) = serde_json::from_slice::<TuiState>(&bytes) else {
            self.status = format!("Ignored invalid state file: {}", path.display());
            return;
        };
        if state.schema_version != TUI_STATE_SCHEMA_VERSION {
            self.status = format!("Ignored old state file: {}", path.display());
            return;
        }
        for field in &mut self.fields {
            if let Some(saved) = state.fields.iter().find(|saved| saved.label == field.label) {
                field.set_value(&saved.value);
            }
        }
        for field in &mut self.consumer_fields {
            if let Some(saved) = state
                .consumer_fields
                .iter()
                .find(|saved| saved.label == field.label)
            {
                field.set_value(&saved.value);
            }
        }
        self.selected_location_id = state.selected_location_id;
        self.location_query.set_value(&state.location_query);
        self.consumer_shape = state.consumer_shape;
        self.sync_consumer_shape_field();
        for field in &mut self.simulation_fields {
            if let Some(saved) = state
                .simulation_fields
                .iter()
                .find(|saved| saved.label == field.label)
            {
                field.set_value(&saved.value);
            }
        }
        self.panel_visibility = state.panel_visibility;
        self.focused_panel = state.focused_panel;
        self.ensure_panel_focus();
        self.refresh_location_results();
        self.status = format!("Loaded {}", path.display());
    }

    fn save_state(&mut self) {
        let Some(path) = tui_state_path() else {
            self.status = "Could not resolve local state path".to_string();
            return;
        };
        let state = TuiState {
            schema_version: TUI_STATE_SCHEMA_VERSION,
            selected_location_id: self.selected_location_id.clone(),
            location_query: self.location_query.value.clone(),
            fields: self
                .fields
                .iter()
                .map(|field| TuiFieldState {
                    label: field.label.to_string(),
                    value: field.value.clone(),
                })
                .collect(),
            consumer_fields: self
                .consumer_fields
                .iter()
                .map(|field| TuiFieldState {
                    label: field.label.to_string(),
                    value: field.value.clone(),
                })
                .collect(),
            consumer_shape: self.consumer_shape.clone(),
            simulation_fields: self
                .simulation_fields
                .iter()
                .map(|field| TuiFieldState {
                    label: field.label.to_string(),
                    value: field.value.clone(),
                })
                .collect(),
            panel_visibility: self.panel_visibility,
            focused_panel: self.focused_panel,
        };
        let result = (|| -> Result<()> {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            let bytes = serde_json::to_vec_pretty(&state)?;
            fs::write(&path, bytes)?;
            Ok(())
        })();
        if let Err(error) = result {
            self.status = format!("Could not save state: {error:#}");
        }
    }

    fn apply_active_edit(&mut self, estimator: &mut SourceModelEstimator, movement: i32) {
        match self.focused_panel {
            Panel::System => {
                self.mode = Mode::Normal;
                self.recompute(estimator);
                match movement.cmp(&0) {
                    std::cmp::Ordering::Greater => {
                        self.selected = (self.selected + 1).min(self.fields.len() - 1);
                    }
                    std::cmp::Ordering::Less => {
                        self.selected = self.selected.saturating_sub(1);
                    }
                    std::cmp::Ordering::Equal => {}
                }
            }
            Panel::Consumer => match self.sync_consumer_energy_fields() {
                Ok(()) => {
                    self.mode = Mode::Normal;
                    self.simulation_result = None;
                    self.status = "Consumer updated".to_string();
                    self.save_state();
                    match movement.cmp(&0) {
                        std::cmp::Ordering::Greater => {
                            self.consumer_selected =
                                (self.consumer_selected + 1).min(self.consumer_fields.len() - 1);
                        }
                        std::cmp::Ordering::Less => {
                            self.consumer_selected = self.consumer_selected.saturating_sub(1);
                        }
                        std::cmp::Ordering::Equal => {}
                    }
                }
                Err(error) => {
                    self.status = format!("{error:#}");
                }
            },
            Panel::Simulation => match self.simulation_options() {
                Ok(_) => {
                    self.mode = Mode::Normal;
                    self.simulation_result = None;
                    self.status = "Simulation options updated".to_string();
                    self.save_state();
                    match movement.cmp(&0) {
                        std::cmp::Ordering::Greater => {
                            self.simulation_selected =
                                (self.simulation_selected + 1).min(SIMULATION_RUN_ROW_INDEX);
                        }
                        std::cmp::Ordering::Less => {
                            self.simulation_selected = self.simulation_selected.saturating_sub(1);
                        }
                        std::cmp::Ordering::Equal => {}
                    }
                }
                Err(error) => {
                    self.status = format!("{error:#}");
                }
            },
            _ => {
                self.mode = Mode::Normal;
            }
        }
    }

    fn visible_panels(&self) -> Vec<Panel> {
        Panel::ALL
            .iter()
            .copied()
            .filter(|panel| self.panel_visibility.is_visible(*panel))
            .collect()
    }

    fn ensure_panel_focus(&mut self) {
        if self.panel_visibility.visible_count() == 0 {
            self.panel_visibility.set_visible(Panel::System, true);
        }
        if !self.panel_visibility.is_visible(self.focused_panel) {
            self.focused_panel = self
                .visible_panels()
                .first()
                .copied()
                .unwrap_or(Panel::System);
        }
    }

    fn toggle_panel(&mut self, panel: Panel) {
        let visible = self.panel_visibility.is_visible(panel);
        if visible && self.panel_visibility.visible_count() == 1 {
            self.status = "At least one panel must stay visible".to_string();
            return;
        }
        self.panel_visibility.set_visible(panel, !visible);
        if !visible {
            self.focused_panel = panel;
        }
        self.ensure_panel_focus();
        self.status = format!(
            "{} panel {}",
            panel.title(),
            if visible { "hidden" } else { "shown" }
        );
    }

    fn focus_panel(&mut self, panel: Panel) {
        if self.panel_visibility.is_visible(panel) {
            self.focused_panel = panel;
            self.status = format!("{} panel focused", panel.title());
        }
    }

    fn focus_next_panel(&mut self, direction: i32) {
        let panels = self.visible_panels();
        if panels.is_empty() {
            self.ensure_panel_focus();
            return;
        }
        let current = panels
            .iter()
            .position(|panel| *panel == self.focused_panel)
            .unwrap_or(0);
        let next = if direction < 0 {
            current.checked_sub(1).unwrap_or(panels.len() - 1)
        } else {
            (current + 1) % panels.len()
        };
        self.focused_panel = panels[next];
        self.status = format!("{} panel focused", self.focused_panel.title());
    }

    fn energy_price_eur_per_kwh(&self) -> Result<Option<f64>> {
        parse_optional_f64(&self.fields[PRICE_FIELD_INDEX])
    }

    fn storage_usable_kwh(&self) -> Result<Option<f64>> {
        parse_optional_positive_f64(&self.fields[STORAGE_FIELD_INDEX])
    }

    fn request_and_arrays(&self) -> Result<(EstimateRequest, Vec<EstimateArray>)> {
        let arrays = parse_arrays(&self.fields[ARRAY_FIELD_INDEX])?;
        let first_array = arrays[0];
        Ok((
            EstimateRequest {
                location_id: self.selected_location_id.clone(),
                name: self.fields[0].value.clone(),
                region: self.fields[1].value.clone(),
                latitude: parse_f64(&self.fields[2])?,
                longitude: parse_f64(&self.fields[3])?,
                peak_power_kwp: first_array.peak_power_kwp,
                loss_pct: parse_f64(&self.fields[4])?,
                tilt_deg: first_array.tilt_deg,
                azimuth_deg: first_array.azimuth_deg,
                storage_usable_kwh: self.storage_usable_kwh()?,
            },
            arrays,
        ))
    }

    fn active_field_mut(&mut self) -> Option<&mut Field> {
        match self.focused_panel {
            Panel::System => Some(&mut self.fields[self.selected]),
            Panel::Consumer if self.consumer_selected != CONSUMER_SHAPE_FIELD_INDEX => {
                Some(&mut self.consumer_fields[self.consumer_selected])
            }
            Panel::Consumer => None,
            Panel::Simulation if self.simulation_selected < self.simulation_fields.len() => {
                Some(&mut self.simulation_fields[self.simulation_selected])
            }
            Panel::Simulation => None,
            _ => None,
        }
    }

    fn sync_consumer_energy_fields(&mut self) -> Result<()> {
        match self.consumer_selected {
            CONSUMER_ANNUAL_FIELD_INDEX => {
                let annual_kwh =
                    parse_positive_f64(&self.consumer_fields[CONSUMER_ANNUAL_FIELD_INDEX])?;
                let daily_kwh = annual_kwh / 365.0;
                self.consumer_fields[CONSUMER_DAILY_FIELD_INDEX]
                    .set_value(&format_energy_field_value(daily_kwh));
            }
            CONSUMER_DAILY_FIELD_INDEX => {
                let daily_kwh =
                    parse_positive_f64(&self.consumer_fields[CONSUMER_DAILY_FIELD_INDEX])?;
                let annual_kwh = daily_kwh * 365.0;
                self.consumer_fields[CONSUMER_ANNUAL_FIELD_INDEX]
                    .set_value(&format_energy_field_value(annual_kwh));
            }
            CONSUMER_SHAPE_FIELD_INDEX => {
                let annual = parse_optional_positive_f64(
                    &self.consumer_fields[CONSUMER_ANNUAL_FIELD_INDEX],
                )?;
                let daily =
                    parse_optional_positive_f64(&self.consumer_fields[CONSUMER_DAILY_FIELD_INDEX])?;
                match (annual, daily) {
                    (Some(annual_kwh), _) => {
                        self.consumer_fields[CONSUMER_DAILY_FIELD_INDEX]
                            .set_value(&format_energy_field_value(annual_kwh / 365.0));
                    }
                    (None, Some(daily_kwh)) => {
                        self.consumer_fields[CONSUMER_ANNUAL_FIELD_INDEX]
                            .set_value(&format_energy_field_value(daily_kwh * 365.0));
                    }
                    (None, None) => {
                        anyhow::bail!("Consumer Annual kWh or Daily kWh is required");
                    }
                }
            }
            _ => {}
        }
        let _ = self.consumer_load_profile()?;
        Ok(())
    }

    fn consumer_load_profile(&self) -> Result<LoadProfile> {
        let shape = self.consumer_shape.load_shape()?;
        let annual =
            parse_optional_positive_f64(&self.consumer_fields[CONSUMER_ANNUAL_FIELD_INDEX])?;
        let daily = parse_optional_positive_f64(&self.consumer_fields[CONSUMER_DAILY_FIELD_INDEX])?;
        match (annual, daily) {
            (Some(annual_kwh), _) => Ok(LoadProfile::AnnualKwh { annual_kwh, shape }),
            (None, Some(daily_kwh)) => Ok(LoadProfile::DailyKwh { daily_kwh, shape }),
            (None, None) => anyhow::bail!("Consumer Annual kWh or Daily kWh is required"),
        }
    }

    fn simulation_options(&self) -> Result<SimulationOptions> {
        let runs = self.simulation_fields[SIMULATION_RUNS_FIELD_INDEX]
            .value
            .trim()
            .parse::<usize>()
            .with_context(|| "Runs must be a positive integer")?;
        if runs == 0 {
            anyhow::bail!("Runs must be a positive integer");
        }
        let seed_value = self.simulation_fields[SIMULATION_SEED_FIELD_INDEX]
            .value
            .trim();
        let seed = if seed_value.is_empty() {
            None
        } else {
            Some(
                seed_value
                    .parse::<u64>()
                    .with_context(|| "Seed must be empty or a non-negative integer")?,
            )
        };
        Ok(SimulationOptions { runs, seed })
    }

    fn simulation_request(
        &self,
        estimator: &mut SourceModelEstimator,
    ) -> Result<SimulationRequest> {
        let (request, arrays) = self.request_and_arrays()?;
        let production = estimator.production_profile_arrays(&request, &arrays)?;
        let load = self.consumer_load_profile()?;
        let storage = self
            .storage_usable_kwh()?
            .map(|usable_capacity_kwh| StorageConfig {
                usable_capacity_kwh,
            });
        Ok(SimulationRequest {
            production,
            load,
            storage,
            options: self.simulation_options()?,
        })
    }

    fn start_simulation_run(&mut self, estimator: &mut SourceModelEstimator) {
        self.simulation_result = None;
        let request = match self.simulation_request(estimator) {
            Ok(request) => request,
            Err(error) => {
                self.status = format!("{error:#}");
                return;
            }
        };
        let requested_runs = request.options.runs;
        let completed_runs = Arc::new(AtomicUsize::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let thread_completed = Arc::clone(&completed_runs);
        let thread_cancel = Arc::clone(&cancel);
        let (sender, receiver) = mpsc::channel();

        thread::spawn(move || {
            let result = simulate_with_progress(
                &request,
                || thread_cancel.load(Ordering::Relaxed),
                |runs| thread_completed.store(runs, Ordering::Relaxed),
            )
            .map_err(|error| error.to_string());
            let _ = sender.send(result);
        });

        self.simulation_run = Some(SimulationRunState {
            requested_runs,
            completed_runs,
            cancel,
            started_at: Instant::now(),
            finished_at: None,
            receiver,
            finished: false,
            cancelling: false,
            error: None,
        });
        self.mode = Mode::SimulationRun;
        self.status = "Simulation running".to_string();
    }

    fn poll_simulation_run(&mut self) {
        let Some(run) = self.simulation_run.as_mut() else {
            return;
        };
        if run.finished {
            return;
        }
        let message = match run.receiver.try_recv() {
            Ok(message) => message,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                run.finished = true;
                run.finished_at = Some(Instant::now());
                run.error = Some("Simulation worker disconnected".to_string());
                self.status = "Simulation failed".to_string();
                return;
            }
        };
        run.finished = true;
        run.finished_at = Some(Instant::now());
        match message {
            Ok(result) => {
                run.completed_runs
                    .store(result.completed_runs, Ordering::Relaxed);
                self.status = if result.cancelled {
                    "Simulation cancelled".to_string()
                } else {
                    "Simulation completed".to_string()
                };
                self.simulation_result = Some(result);
            }
            Err(error) => {
                run.error = Some(error);
                self.status = "Simulation failed".to_string();
            }
        }
    }

    fn cancel_or_close_simulation_run(&mut self) {
        let Some(run) = self.simulation_run.as_mut() else {
            self.mode = Mode::Normal;
            return;
        };
        if run.finished {
            self.mode = Mode::Normal;
            self.simulation_run = None;
        } else {
            run.cancelling = true;
            run.cancel.store(true, Ordering::Relaxed);
            self.status = "Cancelling simulation".to_string();
        }
    }

    fn mark_custom_location_if_needed(&mut self) {
        if self.focused_panel == Panel::System && self.selected <= 3 {
            self.selected_location_id = "custom".to_string();
        }
    }

    fn refresh_location_results(&mut self) {
        if self.location_query.value.chars().count() < 2 {
            self.location_results.clear();
        } else {
            self.location_results = pv_data::search_cities(&self.location_query.value, 30);
        }
        self.clamp_location_selection();
    }

    fn open_location_search(&mut self) {
        self.mode = Mode::Location;
        self.location_selected = 0;
        self.refresh_location_results();
        self.status = "Search and select a location".to_string();
    }

    fn cancel_location_search(&mut self) {
        self.mode = Mode::Normal;
        self.status = "Location search cancelled".to_string();
    }

    fn clamp_location_selection(&mut self) {
        if self.location_results.is_empty() {
            self.location_selected = 0;
        } else {
            self.location_selected = self.location_selected.min(self.location_results.len() - 1);
        }
    }

    fn apply_selected_location(&mut self, estimator: &mut SourceModelEstimator) {
        let Some(location) = self.location_results.get(self.location_selected).cloned() else {
            self.status = "No matching location".to_string();
            return;
        };
        self.apply_location_fields(&location);
        self.mode = Mode::Normal;
        self.recompute(estimator);
    }

    fn apply_location_fields(&mut self, location: &CitySearchResult) {
        self.fields[0].set_value(&location.display_name);
        self.fields[1].set_value(&location.country_code);
        self.fields[2].set_value(&format!("{:.4}", location.latitude_degrees));
        self.fields[3].set_value(&format!("{:.4}", location.longitude_degrees));
        self.selected_location_id = format!("geonames-{}", location.geoname_id);
        self.status = format!(
            "Selected {}, {}",
            location.display_name, location.country_code
        );
    }
    fn open_array_editor(&mut self) {
        self.mode = Mode::Arrays;
        self.array_editing = false;
        self.clamp_array_selection();
        self.status = "Edit system arrays".to_string();
    }

    fn close_array_editor(&mut self) {
        self.mode = Mode::Normal;
        self.array_editing = false;
        self.status = "Arrays editor closed".to_string();
    }

    fn open_shape_editor(&mut self) {
        self.mode = Mode::Shape;
        self.shape_editing = false;
        self.shape_preset_selecting = false;
        self.shape_selected_hour = self.shape_selected_hour.min(23);
        self.status = "Edit consumer load shape".to_string();
    }

    fn close_shape_editor(&mut self) {
        self.mode = Mode::Normal;
        self.shape_editing = false;
        self.shape_preset_selecting = false;
        self.sync_consumer_shape_field();
        self.status = "Shape editor closed".to_string();
        self.save_state();
    }

    fn sync_consumer_shape_field(&mut self) {
        self.consumer_fields[CONSUMER_SHAPE_FIELD_INDEX].set_value(&self.consumer_shape.label());
    }

    fn set_shape_preset(&mut self, shape_id: BuiltInLoadShapeId) {
        self.consumer_shape = ConsumerShapeState::BuiltIn { shape_id };
        self.simulation_result = None;
        self.shape_editing = false;
        self.shape_preset_selecting = false;
        self.sync_consumer_shape_field();
        self.status = format!("Using {} shape", built_in_shape_label(shape_id));
        self.save_state();
    }

    fn open_shape_preset_picker(&mut self) {
        self.shape_editing = false;
        self.shape_preset_selecting = true;
        self.shape_preset_selected = match self.consumer_shape {
            ConsumerShapeState::BuiltIn { shape_id } => BUILT_IN_LOAD_SHAPES
                .iter()
                .position(|candidate| *candidate == shape_id)
                .unwrap_or(0),
            ConsumerShapeState::HourlyWeights { .. } => 0,
        };
        self.status = "Select a load shape preset".to_string();
    }

    fn dismiss_shape_preset_picker(&mut self) {
        self.shape_preset_selecting = false;
        self.status = "Preset selection dismissed".to_string();
    }

    fn apply_shape_preset_picker(&mut self) {
        let shape_id = BUILT_IN_LOAD_SHAPES[self
            .shape_preset_selected
            .min(BUILT_IN_LOAD_SHAPES.len() - 1)];
        self.set_shape_preset(shape_id);
    }

    fn move_shape_preset_selection(&mut self, movement: i32) {
        let last = BUILT_IN_LOAD_SHAPES.len() - 1;
        if movement < 0 {
            self.shape_preset_selected = self.shape_preset_selected.saturating_sub(1);
        } else if movement > 0 {
            self.shape_preset_selected = (self.shape_preset_selected + 1).min(last);
        }
    }

    fn set_shape_custom(&mut self) {
        if !matches!(
            self.consumer_shape,
            ConsumerShapeState::HourlyWeights { .. }
        ) {
            self.consumer_shape = ConsumerShapeState::HourlyWeights {
                weights: self.consumer_shape.weights(),
            };
        }
        self.simulation_result = None;
        self.shape_editing = false;
        self.shape_preset_selecting = false;
        self.sync_consumer_shape_field();
        self.status = "Using custom hourly shape".to_string();
        self.save_state();
    }

    fn start_shape_cell_edit(&mut self) {
        if !matches!(
            self.consumer_shape,
            ConsumerShapeState::HourlyWeights { .. }
        ) {
            self.consumer_shape = ConsumerShapeState::HourlyWeights {
                weights: self.consumer_shape.weights(),
            };
            self.sync_consumer_shape_field();
        }
        self.shape_preset_selecting = false;
        let weights = self.consumer_shape.weights();
        let Some(value) = weights.get(self.shape_selected_hour) else {
            return;
        };
        self.shape_cell.set_value(&format_shape_weight(*value));
        self.shape_editing = true;
        self.status = "Editing hourly weight".to_string();
    }

    fn apply_shape_cell_edit(&mut self) {
        let value = match self.shape_cell.value.parse::<f64>() {
            Ok(value) if value.is_finite() && value >= 0.0 => value,
            _ => {
                self.status = "Shape weight must be finite and non-negative".to_string();
                return;
            }
        };
        let ConsumerShapeState::HourlyWeights { weights } = &mut self.consumer_shape else {
            self.shape_editing = false;
            return;
        };
        if self.shape_selected_hour >= weights.len() {
            self.status = "Shape must contain 24 hourly weights".to_string();
            return;
        }
        let mut next = weights.clone();
        next[self.shape_selected_hour] = value;
        if let Err(error) = validate_shape_weights(&next) {
            self.status = format!("{error:#}");
            return;
        }
        *weights = next;
        self.simulation_result = None;
        self.shape_editing = false;
        self.sync_consumer_shape_field();
        self.status = "Shape weight updated".to_string();
        self.save_state();
    }

    fn move_shape_cell_forward(&mut self) {
        self.shape_selected_hour = (self.shape_selected_hour + 1).min(23);
    }

    fn move_shape_cell_backward(&mut self) {
        self.shape_selected_hour = self.shape_selected_hour.saturating_sub(1);
    }

    fn current_arrays(&self) -> Vec<EstimateArray> {
        parse_arrays(&self.fields[ARRAY_FIELD_INDEX]).unwrap_or_else(|_| vec![default_array()])
    }

    fn clamp_array_selection(&mut self) {
        let arrays = self.current_arrays();
        if arrays.is_empty() {
            self.array_selected = 0;
        } else {
            self.array_selected = self.array_selected.min(arrays.len() - 1);
        }
        self.array_column = self.array_column.min(2);
    }

    fn set_arrays(&mut self, arrays: &[EstimateArray]) {
        self.fields[ARRAY_FIELD_INDEX].set_value(&arrays_to_field_value(arrays));
        self.clamp_array_selection();
    }

    fn add_array(&mut self, estimator: &mut SourceModelEstimator) {
        let mut arrays = self.current_arrays();
        arrays.push(default_array());
        self.array_selected = arrays.len() - 1;
        self.array_column = 0;
        self.set_arrays(&arrays);
        self.recompute(estimator);
    }

    fn remove_selected_array(&mut self, estimator: &mut SourceModelEstimator) {
        let mut arrays = self.current_arrays();
        if arrays.len() <= 1 {
            self.status = "At least one array is required".to_string();
            return;
        }
        arrays.remove(self.array_selected.min(arrays.len() - 1));
        self.array_selected = self.array_selected.saturating_sub(1).min(arrays.len() - 1);
        self.set_arrays(&arrays);
        self.recompute(estimator);
    }

    fn start_array_cell_edit(&mut self) {
        let arrays = self.current_arrays();
        let Some(array) = arrays.get(self.array_selected) else {
            return;
        };
        self.array_cell
            .set_value(&array_cell_value(array, self.array_column));
        self.array_editing = true;
        self.status = "Editing array value".to_string();
    }

    fn apply_array_cell_edit(&mut self, estimator: &mut SourceModelEstimator) {
        let value = match self.array_cell.value.parse::<f64>() {
            Ok(value) => value,
            Err(_) => {
                self.status = "Array value must be a number".to_string();
                return;
            }
        };
        let mut arrays = self.current_arrays();
        let Some(array) = arrays.get_mut(self.array_selected) else {
            return;
        };
        match self.array_column {
            0 => array.peak_power_kwp = value,
            1 => array.tilt_deg = value,
            2 => array.azimuth_deg = value,
            _ => {}
        }
        self.array_editing = false;
        self.set_arrays(&arrays);
        self.recompute(estimator);
    }

    fn move_array_cell_forward(&mut self) {
        if self.array_column < 2 {
            self.array_column += 1;
        } else {
            self.array_column = 0;
            let arrays = self.current_arrays();
            if !arrays.is_empty() {
                self.array_selected = (self.array_selected + 1).min(arrays.len() - 1);
            }
        }
    }

    fn move_array_cell_backward(&mut self) {
        if self.array_column > 0 {
            self.array_column -= 1;
        } else {
            self.array_column = 2;
            self.array_selected = self.array_selected.saturating_sub(1);
        }
    }
    fn scroll_estimate_down(&mut self, rows: usize) {
        self.estimate_scroll = self.estimate_scroll.saturating_add(rows);
    }

    fn scroll_estimate_up(&mut self, rows: usize) {
        self.estimate_scroll = self.estimate_scroll.saturating_sub(rows);
    }
}

fn tui_state_path() -> Option<PathBuf> {
    ProjectDirs::from("dev", "lelloman", "pv-estimator")
        .map(|dirs| dirs.config_dir().join("pv-tui-state.json"))
}

impl Field {
    fn new(label: &'static str, value: &str) -> Self {
        Self {
            label,
            value: value.to_string(),
            cursor: value.len(),
        }
    }

    fn set_value(&mut self, value: &str) {
        self.value = value.to_string();
        self.cursor = self.value.len();
    }

    fn insert(&mut self, character: char) {
        self.value.insert(self.cursor, character);
        self.cursor += character.len_utf8();
    }

    fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        if let Some((index, _)) = self.value[..self.cursor].char_indices().next_back() {
            self.value.drain(index..self.cursor);
            self.cursor = index;
        }
    }

    fn delete(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        if let Some(character) = self.value[self.cursor..].chars().next() {
            let end = self.cursor + character.len_utf8();
            self.value.drain(self.cursor..end);
        }
    }

    fn move_left(&mut self) {
        if self.cursor == 0 {
            return;
        }
        if let Some((index, _)) = self.value[..self.cursor].char_indices().next_back() {
            self.cursor = index;
        }
    }

    fn move_right(&mut self) {
        if self.cursor >= self.value.len() {
            return;
        }
        if let Some(character) = self.value[self.cursor..].chars().next() {
            self.cursor += character.len_utf8();
        }
    }
}

fn parse_optional_f64(field: &Field) -> Result<Option<f64>> {
    let value = field.value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    value
        .parse::<f64>()
        .map(Some)
        .with_context(|| format!("{} must be empty or a number", field.label))
}

fn parse_positive_f64(field: &Field) -> Result<f64> {
    parse_optional_positive_f64(field)?
        .ok_or_else(|| anyhow::anyhow!("{} must be positive", field.label))
}

fn parse_optional_positive_f64(field: &Field) -> Result<Option<f64>> {
    let Some(value) = parse_optional_f64(field)? else {
        return Ok(None);
    };
    if !value.is_finite() || value <= 0.0 {
        anyhow::bail!("{} must be empty or positive", field.label);
    }
    Ok(Some(value))
}

fn format_energy_field_value(value: f64) -> String {
    let rounded = (value * 100.0).round() / 100.0;
    if (rounded - rounded.round()).abs() < 1.0e-9 {
        format!("{rounded:.0}")
    } else {
        format!("{rounded:.2}")
    }
}

fn built_in_shape_label(shape_id: BuiltInLoadShapeId) -> &'static str {
    match shape_id {
        BuiltInLoadShapeId::ResidentialDefault => "residential_default",
        BuiltInLoadShapeId::Flat => "flat",
        BuiltInLoadShapeId::Daytime => "daytime",
        BuiltInLoadShapeId::Evening => "evening",
    }
}

fn built_in_shape_weights(shape_id: BuiltInLoadShapeId) -> &'static [f64; 24] {
    match shape_id {
        BuiltInLoadShapeId::ResidentialDefault => &RESIDENTIAL_DEFAULT_WEIGHTS,
        BuiltInLoadShapeId::Flat => &FLAT_WEIGHTS,
        BuiltInLoadShapeId::Daytime => &DAYTIME_WEIGHTS,
        BuiltInLoadShapeId::Evening => &EVENING_WEIGHTS,
    }
}

fn validate_shape_weights(weights: &[f64]) -> Result<()> {
    if weights.len() != 24 {
        anyhow::bail!("Shape must contain 24 hourly weights");
    }
    if weights
        .iter()
        .any(|value| !value.is_finite() || *value < 0.0)
    {
        anyhow::bail!("Shape weights must be finite and non-negative");
    }
    if weights.iter().sum::<f64>() <= 0.0 {
        anyhow::bail!("Shape weights must contain at least one positive value");
    }
    Ok(())
}

fn format_shape_weight(value: f64) -> String {
    let rounded = (value * 100.0).round() / 100.0;
    if (rounded - rounded.round()).abs() < 1.0e-9 {
        format!("{rounded:.0}")
    } else {
        format!("{rounded:.2}")
    }
}

fn parse_f64(field: &Field) -> Result<f64> {
    field
        .value
        .parse::<f64>()
        .with_context(|| format!("{} must be a number", field.label))
}

fn default_array() -> EstimateArray {
    EstimateArray {
        peak_power_kwp: 1.0,
        tilt_deg: 30.0,
        azimuth_deg: 0.0,
    }
}

fn arrays_to_field_value(arrays: &[EstimateArray]) -> String {
    arrays
        .iter()
        .map(|array| {
            format!(
                "{:.2},{:.1},{:.1}",
                array.peak_power_kwp, array.tilt_deg, array.azimuth_deg
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn array_cell_value(array: &EstimateArray, column: usize) -> String {
    match column {
        0 => format!("{:.2}", array.peak_power_kwp),
        1 => format!("{:.1}", array.tilt_deg),
        2 => format!("{:.1}", array.azimuth_deg),
        _ => String::new(),
    }
}

fn parse_arrays(field: &Field) -> Result<Vec<EstimateArray>> {
    let entries = field
        .value
        .split(';')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .collect::<Vec<_>>();
    if entries.is_empty() {
        anyhow::bail!("Arrays must contain at least one kWp,tilt,azimuth entry");
    }

    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| parse_array_entry(index + 1, entry))
        .collect()
}

fn parse_array_entry(index: usize, entry: &str) -> Result<EstimateArray> {
    let parts = entry.split(',').map(str::trim).collect::<Vec<_>>();
    if parts.len() != 3 {
        anyhow::bail!("array {index} must be kWp,tilt,azimuth");
    }
    Ok(EstimateArray {
        peak_power_kwp: parts[0]
            .parse::<f64>()
            .with_context(|| format!("array {index} kWp must be a number"))?,
        tilt_deg: parts[1]
            .parse::<f64>()
            .with_context(|| format!("array {index} tilt must be a number"))?,
        azimuth_deg: parts[2]
            .parse::<f64>()
            .with_context(|| format!("array {index} azimuth must be a number"))?,
    })
}

fn handle_key(key: KeyEvent, app: &mut App, estimator: &mut SourceModelEstimator) -> Result<bool> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return Ok(true);
    }

    match app.mode {
        Mode::Normal => handle_normal_key(key, app, estimator),
        Mode::Edit => handle_edit_key(key, app, estimator),
        Mode::Location => handle_location_key(key, app, estimator),
        Mode::Arrays => handle_arrays_key(key, app, estimator),
        Mode::Shape => handle_shape_key(key, app),
        Mode::SimulationRun => handle_simulation_run_key(key, app),
    }
}

fn handle_normal_key(
    key: KeyEvent,
    app: &mut App,
    estimator: &mut SourceModelEstimator,
) -> Result<bool> {
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
        KeyCode::Char('1') if key.modifiers.is_empty() => app.toggle_panel(Panel::System),
        KeyCode::Char('2') if key.modifiers.is_empty() => app.toggle_panel(Panel::Consumer),
        KeyCode::Char('3') if key.modifiers.is_empty() => app.toggle_panel(Panel::Simulation),
        KeyCode::Char('4') if key.modifiers.is_empty() => app.toggle_panel(Panel::Estimate),
        KeyCode::Left | KeyCode::BackTab => app.focus_next_panel(-1),
        KeyCode::Right | KeyCode::Tab => app.focus_next_panel(1),
        KeyCode::Up if app.focused_panel == Panel::System => {
            app.selected = app.selected.saturating_sub(1)
        }
        KeyCode::Down if app.focused_panel == Panel::System => {
            app.selected = (app.selected + 1).min(app.fields.len() - 1)
        }
        KeyCode::Up if app.focused_panel == Panel::Consumer => {
            app.consumer_selected = app.consumer_selected.saturating_sub(1)
        }
        KeyCode::Down if app.focused_panel == Panel::Consumer => {
            app.consumer_selected = (app.consumer_selected + 1).min(app.consumer_fields.len() - 1)
        }
        KeyCode::Up if app.focused_panel == Panel::Simulation => {
            app.simulation_selected = app.simulation_selected.saturating_sub(1)
        }
        KeyCode::Down if app.focused_panel == Panel::Simulation => {
            app.simulation_selected = (app.simulation_selected + 1).min(SIMULATION_RUN_ROW_INDEX)
        }
        KeyCode::Home if app.focused_panel == Panel::System => app.selected = 0,
        KeyCode::End if app.focused_panel == Panel::System => app.selected = app.fields.len() - 1,
        KeyCode::Home if app.focused_panel == Panel::Consumer => app.consumer_selected = 0,
        KeyCode::End if app.focused_panel == Panel::Consumer => {
            app.consumer_selected = app.consumer_fields.len() - 1
        }
        KeyCode::Home if app.focused_panel == Panel::Simulation => app.simulation_selected = 0,
        KeyCode::End if app.focused_panel == Panel::Simulation => {
            app.simulation_selected = SIMULATION_RUN_ROW_INDEX
        }
        KeyCode::Enter
            if app.focused_panel == Panel::System && app.fields[app.selected].label == "Name" =>
        {
            app.open_location_search()
        }
        KeyCode::Enter
            if app.focused_panel == Panel::System && app.fields[app.selected].label == "Arrays" =>
        {
            app.open_array_editor()
        }
        KeyCode::Enter
            if app.focused_panel == Panel::Consumer
                && app.consumer_selected == CONSUMER_SHAPE_FIELD_INDEX =>
        {
            app.open_shape_editor()
        }
        KeyCode::Enter if app.focused_panel == Panel::System => app.mode = Mode::Edit,
        KeyCode::Enter if app.focused_panel == Panel::Consumer => app.mode = Mode::Edit,
        KeyCode::Enter
            if app.focused_panel == Panel::Simulation
                && app.simulation_selected == SIMULATION_RUN_ROW_INDEX =>
        {
            app.start_simulation_run(estimator)
        }
        KeyCode::Enter if app.focused_panel == Panel::Simulation => app.mode = Mode::Edit,
        KeyCode::Char('l') if app.focused_panel == Panel::System => app.open_location_search(),
        KeyCode::Char('e') => app.recompute(estimator),
        KeyCode::Char('r') if key.modifiers.is_empty() => app.start_simulation_run(estimator),
        KeyCode::PageDown if app.focused_panel == Panel::Estimate => {
            app.scroll_estimate_down(ESTIMATE_SCROLL_PAGE_ROWS)
        }
        KeyCode::PageUp if app.focused_panel == Panel::Estimate => {
            app.scroll_estimate_up(ESTIMATE_SCROLL_PAGE_ROWS)
        }
        _ => {}
    }
    Ok(false)
}

fn handle_edit_key(
    key: KeyEvent,
    app: &mut App,
    estimator: &mut SourceModelEstimator,
) -> Result<bool> {
    match key.code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Enter => app.apply_active_edit(estimator, 0),
        KeyCode::Tab => app.apply_active_edit(estimator, 1),
        KeyCode::BackTab => app.apply_active_edit(estimator, -1),
        KeyCode::Backspace => {
            if let Some(field) = app.active_field_mut() {
                field.backspace();
            }
            app.mark_custom_location_if_needed();
        }
        KeyCode::Delete => {
            if let Some(field) = app.active_field_mut() {
                field.delete();
            }
            app.mark_custom_location_if_needed();
        }
        KeyCode::Left => {
            if let Some(field) = app.active_field_mut() {
                field.move_left();
            }
        }
        KeyCode::Right => {
            if let Some(field) = app.active_field_mut() {
                field.move_right();
            }
        }
        KeyCode::Home => {
            if let Some(field) = app.active_field_mut() {
                field.cursor = 0;
            }
        }
        KeyCode::End => {
            if let Some(field) = app.active_field_mut() {
                field.cursor = field.value.len();
            }
        }
        KeyCode::Char(character)
            if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
        {
            if let Some(field) = app.active_field_mut() {
                field.insert(character);
            }
            app.mark_custom_location_if_needed();
        }
        _ => {}
    }
    Ok(false)
}

fn handle_mouse(
    mouse: MouseEvent,
    app: &mut App,
    estimator: &mut SourceModelEstimator,
) -> Result<()> {
    let (width, height) = size()?;
    let area = Rect::new(0, 0, width, height);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(10), Constraint::Length(2)])
        .split(area);

    if matches!(
        mouse.kind,
        MouseEventKind::ScrollDown | MouseEventKind::ScrollUp
    ) && app.mode != Mode::Location
        && app.mode != Mode::Arrays
        && app.mode != Mode::Shape
        && app.mode != Mode::SimulationRun
    {
        if let Some((Panel::Estimate, _)) = panel_at(vertical[0], app, mouse.column, mouse.row) {
            app.focus_panel(Panel::Estimate);
            match mouse.kind {
                MouseEventKind::ScrollDown => app.scroll_estimate_down(1),
                MouseEventKind::ScrollUp => app.scroll_estimate_up(1),
                _ => {}
            }
        }
        return Ok(());
    }

    if mouse.kind != MouseEventKind::Down(event::MouseButton::Left) {
        return Ok(());
    }

    if app.mode == Mode::Location {
        if location_cancel_hit(vertical[1], mouse.column, mouse.row) {
            app.cancel_location_search();
            return Ok(());
        }
        if let Some(index) = location_result_index_at(vertical[0], mouse.column, mouse.row)
            && index < app.location_results.len()
        {
            app.location_selected = index;
            app.apply_selected_location(estimator);
        }
        return Ok(());
    }

    if app.mode == Mode::Arrays {
        match array_footer_hit(vertical[1], mouse.column, mouse.row) {
            Some(ArrayFooterAction::Done) => app.close_array_editor(),
            Some(ArrayFooterAction::Add) => app.add_array(estimator),
            Some(ArrayFooterAction::Remove) => app.remove_selected_array(estimator),
            None => {
                let arrays = app.current_arrays();
                let inner = array_editor_inner(vertical[0]);
                let visible_start = array_visible_start(
                    app.array_selected,
                    arrays.len(),
                    array_visible_row_count(inner),
                );
                if let Some((array_index, array_column)) =
                    array_cell_at(vertical[0], mouse.column, mouse.row, visible_start)
                    && array_index < arrays.len()
                {
                    app.array_selected = array_index;
                    app.array_column = array_column;
                    app.start_array_cell_edit();
                }
            }
        }
        return Ok(());
    }

    if app.mode == Mode::Shape {
        match shape_footer_hit(vertical[1], mouse.column, mouse.row) {
            Some(ShapeFooterAction::Done) => app.close_shape_editor(),
            Some(ShapeFooterAction::Preset) => app.open_shape_preset_picker(),
            Some(ShapeFooterAction::Custom) => app.set_shape_custom(),
            None if app.shape_preset_selecting => {
                if let Some(index) = shape_preset_option_at(vertical[0], mouse.column, mouse.row) {
                    app.shape_preset_selected = index;
                    app.apply_shape_preset_picker();
                } else {
                    app.dismiss_shape_preset_picker();
                }
            }
            None => {
                if let Some(hour) = shape_cell_at(vertical[0], mouse.column, mouse.row) {
                    app.shape_selected_hour = hour;
                    app.start_shape_cell_edit();
                }
            }
        }
        return Ok(());
    }

    if app.mode == Mode::SimulationRun {
        app.cancel_or_close_simulation_run();
        return Ok(());
    }

    let Some((panel, panel_area)) = panel_at(vertical[0], app, mouse.column, mouse.row) else {
        return Ok(());
    };
    app.focus_panel(panel);
    if panel == Panel::System {
        let fields_inner = Block::default().borders(Borders::ALL).inner(panel_area);
        if mouse.column >= fields_inner.x
            && mouse.column < fields_inner.x.saturating_add(fields_inner.width)
            && mouse.row >= fields_inner.y
        {
            let row = mouse.row.saturating_sub(fields_inner.y) as usize;
            if row < app.fields.len() {
                app.selected = row;
                match app.fields[row].label {
                    "Name" => app.open_location_search(),
                    "Arrays" => app.open_array_editor(),
                    _ => {}
                }
            }
        }
    } else if panel == Panel::Consumer {
        let fields_inner = Block::default().borders(Borders::ALL).inner(panel_area);
        if mouse.column >= fields_inner.x
            && mouse.column < fields_inner.x.saturating_add(fields_inner.width)
            && mouse.row >= fields_inner.y
        {
            let row = mouse.row.saturating_sub(fields_inner.y) as usize;
            if row < app.consumer_fields.len() {
                app.consumer_selected = row;
                if row == CONSUMER_SHAPE_FIELD_INDEX {
                    app.open_shape_editor();
                } else {
                    app.mode = Mode::Edit;
                }
            }
        }
    } else if panel == Panel::Simulation {
        let fields_inner = Block::default().borders(Borders::ALL).inner(panel_area);
        if mouse.column >= fields_inner.x
            && mouse.column < fields_inner.x.saturating_add(fields_inner.width)
            && mouse.row >= fields_inner.y
        {
            let row = mouse.row.saturating_sub(fields_inner.y) as usize;
            if row <= SIMULATION_RUN_ROW_INDEX {
                app.simulation_selected = row;
                if row == SIMULATION_RUN_ROW_INDEX {
                    app.start_simulation_run(estimator);
                } else {
                    app.mode = Mode::Edit;
                }
            }
        }
    }
    Ok(())
}

fn handle_location_key(
    key: KeyEvent,
    app: &mut App,
    estimator: &mut SourceModelEstimator,
) -> Result<bool> {
    match key.code {
        KeyCode::Esc => app.cancel_location_search(),
        KeyCode::Enter => app.apply_selected_location(estimator),
        KeyCode::Up => app.location_selected = app.location_selected.saturating_sub(1),
        KeyCode::Down | KeyCode::Tab => {
            if !app.location_results.is_empty() {
                app.location_selected =
                    (app.location_selected + 1).min(app.location_results.len() - 1);
            }
        }
        KeyCode::Backspace => {
            app.location_query.backspace();
            app.location_selected = 0;
            app.refresh_location_results();
        }
        KeyCode::Delete => {
            app.location_query.delete();
            app.location_selected = 0;
            app.refresh_location_results();
        }
        KeyCode::Left => app.location_query.move_left(),
        KeyCode::Right => app.location_query.move_right(),
        KeyCode::Home => app.location_query.cursor = 0,
        KeyCode::End => app.location_query.cursor = app.location_query.value.len(),
        KeyCode::Char(character)
            if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
        {
            app.location_query.insert(character);
            app.location_selected = 0;
            app.refresh_location_results();
        }
        _ => {}
    }
    app.clamp_location_selection();
    Ok(false)
}

fn handle_arrays_key(
    key: KeyEvent,
    app: &mut App,
    estimator: &mut SourceModelEstimator,
) -> Result<bool> {
    if app.array_editing {
        match key.code {
            KeyCode::Esc => {
                app.array_editing = false;
                app.status = "Array edit cancelled".to_string();
            }
            KeyCode::Enter => app.apply_array_cell_edit(estimator),
            KeyCode::Tab => {
                app.apply_array_cell_edit(estimator);
                if !app.array_editing {
                    app.move_array_cell_forward();
                }
            }
            KeyCode::BackTab => {
                app.apply_array_cell_edit(estimator);
                if !app.array_editing {
                    app.move_array_cell_backward();
                }
            }
            KeyCode::Backspace => app.array_cell.backspace(),
            KeyCode::Delete => app.array_cell.delete(),
            KeyCode::Left => app.array_cell.move_left(),
            KeyCode::Right => app.array_cell.move_right(),
            KeyCode::Home => app.array_cell.cursor = 0,
            KeyCode::End => app.array_cell.cursor = app.array_cell.value.len(),
            KeyCode::Char(character)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                app.array_cell.insert(character);
            }
            _ => {}
        }
        return Ok(false);
    }

    match key.code {
        KeyCode::Esc => app.close_array_editor(),
        KeyCode::Enter => app.start_array_cell_edit(),
        KeyCode::Up => app.array_selected = app.array_selected.saturating_sub(1),
        KeyCode::Down => {
            let arrays = app.current_arrays();
            if !arrays.is_empty() {
                app.array_selected = (app.array_selected + 1).min(arrays.len() - 1);
            }
        }
        KeyCode::Left | KeyCode::BackTab => app.move_array_cell_backward(),
        KeyCode::Right | KeyCode::Tab => app.move_array_cell_forward(),
        KeyCode::Char('a') => app.add_array(estimator),
        KeyCode::Char('d') | KeyCode::Delete => app.remove_selected_array(estimator),
        _ => {}
    }
    app.clamp_array_selection();
    Ok(false)
}

fn handle_shape_key(key: KeyEvent, app: &mut App) -> Result<bool> {
    if app.shape_preset_selecting {
        match key.code {
            KeyCode::Esc => app.dismiss_shape_preset_picker(),
            KeyCode::Enter => app.apply_shape_preset_picker(),
            KeyCode::Up | KeyCode::BackTab => app.move_shape_preset_selection(-1),
            KeyCode::Down | KeyCode::Tab => app.move_shape_preset_selection(1),
            KeyCode::Char('p') if key.modifiers.is_empty() => app.dismiss_shape_preset_picker(),
            _ => {}
        }
        return Ok(false);
    }

    if app.shape_editing {
        match key.code {
            KeyCode::Esc => {
                app.shape_editing = false;
                app.status = "Shape edit cancelled".to_string();
            }
            KeyCode::Enter => app.apply_shape_cell_edit(),
            KeyCode::Tab => {
                app.apply_shape_cell_edit();
                if !app.shape_editing {
                    app.move_shape_cell_forward();
                }
            }
            KeyCode::BackTab => {
                app.apply_shape_cell_edit();
                if !app.shape_editing {
                    app.move_shape_cell_backward();
                }
            }
            KeyCode::Backspace => app.shape_cell.backspace(),
            KeyCode::Delete => app.shape_cell.delete(),
            KeyCode::Left => app.shape_cell.move_left(),
            KeyCode::Right => app.shape_cell.move_right(),
            KeyCode::Home => app.shape_cell.cursor = 0,
            KeyCode::End => app.shape_cell.cursor = app.shape_cell.value.len(),
            KeyCode::Char(character)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                app.shape_cell.insert(character);
            }
            _ => {}
        }
        return Ok(false);
    }

    match key.code {
        KeyCode::Esc => app.close_shape_editor(),
        KeyCode::Enter => app.start_shape_cell_edit(),
        KeyCode::Up => {
            app.shape_selected_hour = app.shape_selected_hour.saturating_sub(SHAPE_EDITOR_COLUMNS)
        }
        KeyCode::Down => {
            app.shape_selected_hour = (app.shape_selected_hour + SHAPE_EDITOR_COLUMNS).min(23)
        }
        KeyCode::Left | KeyCode::BackTab => app.move_shape_cell_backward(),
        KeyCode::Right | KeyCode::Tab => app.move_shape_cell_forward(),
        KeyCode::Char('p') if key.modifiers.is_empty() => app.open_shape_preset_picker(),
        KeyCode::Char('c') if key.modifiers.is_empty() => app.set_shape_custom(),
        _ => {}
    }
    Ok(false)
}

fn handle_simulation_run_key(key: KeyEvent, app: &mut App) -> Result<bool> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('c') => app.cancel_or_close_simulation_run(),
        KeyCode::Enter
            if app
                .simulation_run
                .as_ref()
                .map(|run| run.finished)
                .unwrap_or(false) =>
        {
            app.cancel_or_close_simulation_run()
        }
        _ => {}
    }
    Ok(false)
}

fn render(frame: &mut ratatui::Frame<'_>, app: &App) {
    let area = frame.area();
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(10), Constraint::Length(2)])
        .split(area);

    if app.mode == Mode::Location {
        render_location_search(frame, vertical[0], app);
        render_footer(frame, vertical[1], app);
        return;
    }
    if app.mode == Mode::Arrays {
        render_array_editor(frame, vertical[0], app);
        render_footer(frame, vertical[1], app);
        return;
    }
    if app.mode == Mode::Shape {
        render_shape_editor(frame, vertical[0], app);
        render_footer(frame, vertical[1], app);
        return;
    }
    if app.mode == Mode::SimulationRun {
        render_simulation_run(frame, vertical[0], app);
        render_footer(frame, vertical[1], app);
        return;
    }

    for (panel, panel_area) in panel_layout(vertical[0], app) {
        render_panel(frame, panel_area, app, panel);
    }
    render_footer(frame, vertical[1], app);
}

fn panel_layout(area: Rect, app: &App) -> Vec<(Panel, Rect)> {
    let left_panels = [Panel::System, Panel::Consumer]
        .into_iter()
        .filter(|panel| app.panel_visibility.is_visible(*panel))
        .collect::<Vec<_>>();
    let right_panels = [Panel::Simulation, Panel::Estimate]
        .into_iter()
        .filter(|panel| app.panel_visibility.is_visible(*panel))
        .collect::<Vec<_>>();

    match (left_panels.is_empty(), right_panels.is_empty()) {
        (true, true) => vec![(Panel::System, area)],
        (false, true) => stack_panel_column(area, &left_panels),
        (true, false) => stack_panel_column(area, &right_panels),
        (false, false) => {
            let columns = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(area);
            let mut output = stack_panel_column(columns[0], &left_panels);
            output.extend(stack_panel_column(columns[1], &right_panels));
            output
        }
    }
}

fn stack_panel_column(area: Rect, panels: &[Panel]) -> Vec<(Panel, Rect)> {
    match panels {
        [] => Vec::new(),
        [panel] => vec![(*panel, area)],
        [top, bottom, ..] => {
            let rows = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(area);
            vec![(*top, rows[0]), (*bottom, rows[1])]
        }
    }
}

fn panel_at(area: Rect, app: &App, column: u16, row: u16) -> Option<(Panel, Rect)> {
    panel_layout(area, app)
        .into_iter()
        .find(|(_, panel_area)| rect_contains(*panel_area, column, row))
}

fn render_panel(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App, panel: Panel) {
    match panel {
        Panel::System => render_fields(frame, area, app, app.focused_panel == panel),
        Panel::Consumer => render_consumer(frame, area, app, app.focused_panel == panel),
        Panel::Simulation => render_simulation(frame, area, app, app.focused_panel == panel),
        Panel::Estimate => render_estimate(frame, area, app, app.focused_panel == panel),
    }
}

fn rect_contains(area: Rect, column: u16, row: u16) -> bool {
    column >= area.x
        && column < area.x.saturating_add(area.width)
        && row >= area.y
        && row < area.y.saturating_add(area.height)
}

fn panel_block(title: &'static str, toggle_key: char, focused: bool) -> Block<'static> {
    let title = format!("[{toggle_key}] {title}");
    let style = if focused {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(style)
}

fn estimate_metric_line(label: &'static str, value: String, value_style: Style) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{label:<ESTIMATE_LABEL_WIDTH$}"),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(value, value_style),
    ])
}

fn annual_energy_line(document: Option<&SourceEnsembleEstimateDocument>) -> Line<'static> {
    let value = document
        .map(|document| {
            let estimate = &document.ensemble_estimate;
            let mean = estimate.annual_energy.mean.as_kilowatt_hours().round();
            estimate
                .uncertainty
                .annual_energy
                .map(|band| {
                    format!(
                        "{mean:.0} - {:.0}..{:.0}",
                        band.low.as_kilowatt_hours().round(),
                        band.high.as_kilowatt_hours().round()
                    )
                })
                .unwrap_or_else(|| format!("{mean:.0} - -..-"))
        })
        .unwrap_or_else(|| "-".to_string());

    estimate_metric_line(
        "Annual kWh",
        value,
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    )
}

fn annual_revenue_line(document: &SourceEnsembleEstimateDocument, price: f64) -> Line<'static> {
    let estimate = &document.ensemble_estimate;
    let mean = estimate.annual_energy.mean.as_kilowatt_hours() * price;
    let value = estimate
        .uncertainty
        .annual_energy
        .map(|band| {
            format!(
                "{:.0} - {:.0}..{:.0}",
                mean.round(),
                (band.low.as_kilowatt_hours() * price).round(),
                (band.high.as_kilowatt_hours() * price).round()
            )
        })
        .unwrap_or_else(|| format!("{:.0} - -..-", mean.round()));

    estimate_metric_line(
        "Revenue €",
        value,
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    )
}

fn render_fields(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App, focused: bool) {
    let block = panel_block("System", Panel::System.toggle_key(), focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let array_extra_lines = array_extra_line_count(app);
    let mut lines = Vec::with_capacity(
        app.fields.len() + array_extra_lines as usize + app.location_results.len().min(6) + 3,
    );
    for (index, field) in app.fields.iter().enumerate() {
        let selected = focused && index == app.selected;
        let style = match (selected, app.mode) {
            (true, Mode::Edit) => Style::default().fg(Color::Black).bg(Color::Yellow),
            (true, Mode::Normal) => Style::default().fg(Color::Black).bg(Color::Cyan),
            _ => Style::default(),
        };
        let value_width = field_value_width(inner);
        let value_view = if field.label == "Arrays" {
            arrays_field_summary(field)
        } else {
            field_value_view(field, value_width, selected).value
        };
        let label_style = if selected {
            style
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let spans = vec![
            Span::styled(
                format!(
                    "{:<width$}",
                    field.label,
                    width = FIELD_LABEL_WIDTH as usize
                ),
                label_style,
            ),
            Span::styled(
                format!("{:<width$}", value_view, width = value_width),
                style,
            ),
        ];
        lines.push(Line::from(spans));
        if field.label == "Arrays"
            && let Ok(arrays) = parse_arrays(field)
        {
            lines.extend(array_summary_lines(&arrays));
        }
    }
    frame.render_widget(Paragraph::new(lines), inner);

    if focused && app.mode == Mode::Edit {
        let field = &app.fields[app.selected];
        let value_view = field_value_view(field, field_value_width(inner), true);
        let y = inner.y.saturating_add(app.selected as u16);
        let x = inner
            .x
            .saturating_add(FIELD_LABEL_WIDTH)
            .saturating_add(value_view.cursor_col.min(u16::MAX as usize) as u16);
        frame.set_cursor_position(Position::new(x, y));
    }
}

fn arrays_field_summary(field: &Field) -> String {
    parse_arrays(field)
        .map(|arrays| {
            let count = arrays.len();
            let noun = if count == 1 { "array" } else { "arrays" };
            format!("[Edit]  {count} {noun}")
        })
        .unwrap_or_else(|_| "[Edit]  invalid arrays".to_string())
}

fn consumer_shape_summary(shape: &ConsumerShapeState, max_width: usize) -> String {
    let full = shape.label();
    if full.chars().count() <= max_width {
        return full;
    }
    match shape {
        ConsumerShapeState::BuiltIn { .. } => "[Edit]  preset".to_string(),
        ConsumerShapeState::HourlyWeights { .. } => truncate(&full, max_width),
    }
}

fn array_summary_lines(arrays: &[EstimateArray]) -> Vec<Line<'static>> {
    let label_style = Style::default().fg(Color::DarkGray);
    let total_style = Style::default().fg(Color::Green);
    let value_style = Style::default();
    let mut lines = vec![
        Line::from(vec![
            Span::styled("  total ", label_style),
            Span::styled(format!("{:.2} kWp", total_array_kwp(arrays)), total_style),
        ]),
        Line::from(vec![Span::styled(
            "  ID | kWp  | Tilt | Az     | Dir",
            label_style,
        )]),
    ];
    for (array_index, array) in arrays.iter().enumerate() {
        let direction = azimuth_direction_label(&array.azimuth_deg.to_string()).unwrap_or("");
        lines.push(Line::from(vec![
            Span::styled(format!("  A{:<2}| ", array_index + 1), label_style),
            Span::styled(format!("{:<5.2}", array.peak_power_kwp), value_style),
            Span::styled("| ", label_style),
            Span::styled(format!("{:<5.1}", array.tilt_deg), value_style),
            Span::styled("| ", label_style),
            Span::styled(format!("{:<7.1}", array.azimuth_deg), value_style),
            Span::styled("| ", label_style),
            Span::styled(direction.to_string(), value_style),
        ]));
    }
    lines
}

fn render_array_editor(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title("Arrays");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let arrays = app.current_arrays();
    let total_kwp = total_array_kwp(&arrays);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(1)])
        .split(inner);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled("Total ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{total_kwp:.2} kWp"),
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(""),
        ]),
        chunks[0],
    );

    let visible_count = array_visible_row_count(inner);
    let visible_start = array_visible_start(app.array_selected, arrays.len(), visible_count);
    let rows = arrays
        .iter()
        .enumerate()
        .skip(visible_start)
        .take(visible_count)
        .map(|(index, array)| array_editor_row(app, index, array));
    let header = Row::new(vec![
        Cell::from("ID"),
        Cell::from("|"),
        Cell::from("kWp"),
        Cell::from("|"),
        Cell::from("Tilt"),
        Cell::from("|"),
        Cell::from("Azimuth"),
        Cell::from("|"),
        Cell::from("Direction"),
    ])
    .style(Style::default().fg(Color::DarkGray));
    let widths = ARRAY_TABLE_WIDTHS.map(Constraint::Length);
    let table = Table::new(rows, widths).header(header).column_spacing(1);
    frame.render_widget(table, chunks[1]);

    if app.array_editing {
        let visible_row = app.array_selected.saturating_sub(visible_start);
        let cursor = array_editor_cursor(inner, &app.array_cell, visible_row, app.array_column);
        frame.set_cursor_position(cursor);
    }
}

fn array_editor_row(app: &App, index: usize, array: &EstimateArray) -> Row<'static> {
    let selected = app.array_selected == index;
    let cell_style = |column: usize| {
        if selected && app.array_column == column {
            if app.array_editing {
                Style::default().fg(Color::Black).bg(Color::Yellow)
            } else {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            }
        } else {
            Style::default()
        }
    };
    let value_for = |column: usize| {
        if selected && app.array_column == column && app.array_editing {
            field_value_view(&app.array_cell, ARRAY_CELL_WIDTHS[column], true).value
        } else {
            array_cell_value(array, column)
        }
    };
    let direction = azimuth_direction_label(&array.azimuth_deg.to_string()).unwrap_or("");
    let separator = Cell::from("|").style(Style::default().fg(Color::DarkGray));
    Row::new(vec![
        Cell::from(format!("A{}", index + 1)).style(Style::default().fg(Color::DarkGray)),
        separator.clone(),
        Cell::from(value_for(0)).style(cell_style(0)),
        separator.clone(),
        Cell::from(value_for(1)).style(cell_style(1)),
        separator.clone(),
        Cell::from(value_for(2)).style(cell_style(2)),
        separator,
        Cell::from(direction.to_string()).style(Style::default().fg(Color::DarkGray)),
    ])
}

fn array_editor_cursor(inner: Rect, field: &Field, row: usize, column: usize) -> Position {
    let value_view = field_value_view(field, ARRAY_CELL_WIDTHS[column], true);
    Position::new(
        inner
            .x
            .saturating_add(ARRAY_CELL_STARTS[column])
            .saturating_add(value_view.cursor_col.min(u16::MAX as usize) as u16),
        inner
            .y
            .saturating_add(ARRAY_EDITOR_HEADER_ROWS)
            .saturating_add(row.min(u16::MAX as usize) as u16),
    )
}

fn array_editor_inner(area: Rect) -> Rect {
    Block::default().borders(Borders::ALL).inner(area)
}

fn array_cell_at(
    area: Rect,
    column: u16,
    row: u16,
    visible_start: usize,
) -> Option<(usize, usize)> {
    let inner = array_editor_inner(area);
    if column < inner.x || column >= inner.x.saturating_add(inner.width) {
        return None;
    }
    let first_row = inner.y.saturating_add(ARRAY_EDITOR_HEADER_ROWS);
    if row < first_row {
        return None;
    }
    let visible_row = row.saturating_sub(first_row) as usize;
    if visible_row >= array_visible_row_count(inner) {
        return None;
    }
    let rel_col = column.saturating_sub(inner.x);
    let cell_column = ARRAY_CELL_STARTS
        .iter()
        .enumerate()
        .find_map(|(index, start)| {
            let end = start.saturating_add(ARRAY_CELL_WIDTHS[index] as u16);
            (rel_col >= *start && rel_col < end).then_some(index)
        })?;
    Some((visible_start + visible_row, cell_column))
}

fn array_visible_row_count(inner: Rect) -> usize {
    inner.height.saturating_sub(ARRAY_EDITOR_HEADER_ROWS).max(1) as usize
}

fn array_visible_start(selected: usize, total: usize, visible_count: usize) -> usize {
    if total <= visible_count {
        return 0;
    }
    selected
        .saturating_add(1)
        .saturating_sub(visible_count)
        .min(total.saturating_sub(visible_count))
}

fn render_shape_editor(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title("Load Shape");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let weights = app.consumer_shape.weights();
    let sum = weights.iter().sum::<f64>();
    let mut lines = vec![
        Line::from(vec![
            Span::styled("Mode         ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                app.consumer_shape.mode_label(),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::styled("Shape        ", Style::default().fg(Color::DarkGray)),
            Span::raw(match &app.consumer_shape {
                ConsumerShapeState::BuiltIn { shape_id } => {
                    built_in_shape_label(*shape_id).to_string()
                }
                ConsumerShapeState::HourlyWeights { .. } => {
                    format!("custom hourly  total {sum:.2}")
                }
            }),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Hourly weights",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    for row in 0..SHAPE_EDITOR_ROWS {
        let mut spans = Vec::new();
        for column in 0..SHAPE_EDITOR_COLUMNS {
            let hour = row * SHAPE_EDITOR_COLUMNS + column;
            let selected = hour == app.shape_selected_hour;
            let style = match (selected, app.shape_editing) {
                (true, true) => Style::default().fg(Color::Black).bg(Color::Yellow),
                (true, false) => Style::default().fg(Color::Black).bg(Color::Cyan),
                _ => Style::default(),
            };
            let value = if selected && app.shape_editing {
                field_value_view(&app.shape_cell, SHAPE_VALUE_WIDTH, true).value
            } else {
                format_shape_weight(weights[hour])
            };
            spans.push(Span::styled(
                format!("{hour:02}: {:<width$}", value, width = SHAPE_VALUE_WIDTH),
                style,
            ));
            if column + 1 < SHAPE_EDITOR_COLUMNS {
                spans.push(Span::raw("     "));
            }
        }
        lines.push(Line::from(spans));
    }

    frame.render_widget(Paragraph::new(lines), inner);
    if app.shape_preset_selecting {
        render_shape_preset_picker(frame, area, app);
    }
    if app.shape_editing {
        frame.set_cursor_position(shape_editor_cursor(inner, app));
    }
}

fn render_shape_preset_picker(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let picker = shape_preset_picker_area(area);
    frame.render_widget(Clear, picker);
    let block = Block::default().borders(Borders::ALL).title("Preset");
    let inner = block.inner(picker);
    frame.render_widget(block, picker);

    let lines = BUILT_IN_LOAD_SHAPES
        .iter()
        .enumerate()
        .map(|(index, shape_id)| {
            let selected = index == app.shape_preset_selected;
            let active = matches!(
                app.consumer_shape,
                ConsumerShapeState::BuiltIn { shape_id: active } if active == *shape_id
            );
            let style = if selected {
                Style::default().fg(Color::Black).bg(Color::Cyan)
            } else {
                Style::default()
            };
            let marker = if active { "*" } else { " " };
            Line::from(Span::styled(
                format!("{marker} {}", built_in_shape_label(*shape_id)),
                style,
            ))
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn shape_preset_picker_area(area: Rect) -> Rect {
    let inner = shape_editor_inner(area);
    Rect::new(
        inner.x.saturating_add(13),
        inner.y.saturating_add(1),
        SHAPE_PRESET_PICKER_WIDTH.min(inner.width.saturating_sub(13).max(1)),
        SHAPE_PRESET_PICKER_HEIGHT.min(inner.height.max(1)),
    )
}

fn shape_preset_option_at(area: Rect, column: u16, row: u16) -> Option<usize> {
    let picker = shape_preset_picker_area(area);
    let inner = Block::default().borders(Borders::ALL).inner(picker);
    if column < inner.x || column >= inner.x.saturating_add(inner.width) {
        return None;
    }
    if row < inner.y || row >= inner.y.saturating_add(inner.height) {
        return None;
    }
    let index = row.saturating_sub(inner.y) as usize;
    (index < BUILT_IN_LOAD_SHAPES.len()).then_some(index)
}

fn shape_editor_inner(area: Rect) -> Rect {
    Block::default().borders(Borders::ALL).inner(area)
}

fn shape_editor_cursor(inner: Rect, app: &App) -> Position {
    let row = app.shape_selected_hour / SHAPE_EDITOR_COLUMNS;
    let column = app.shape_selected_hour % SHAPE_EDITOR_COLUMNS;
    let value_view = field_value_view(&app.shape_cell, SHAPE_VALUE_WIDTH, true);
    Position::new(
        inner
            .x
            .saturating_add(SHAPE_CELL_STARTS[column])
            .saturating_add(SHAPE_VALUE_OFFSET)
            .saturating_add(value_view.cursor_col.min(u16::MAX as usize) as u16),
        inner
            .y
            .saturating_add(SHAPE_EDITOR_HEADER_ROWS)
            .saturating_add(row.min(u16::MAX as usize) as u16),
    )
}

fn shape_cell_at(area: Rect, column: u16, row: u16) -> Option<usize> {
    let inner = shape_editor_inner(area);
    if column < inner.x || column >= inner.x.saturating_add(inner.width) {
        return None;
    }
    let first_row = inner.y.saturating_add(SHAPE_EDITOR_HEADER_ROWS);
    if row < first_row {
        return None;
    }
    let rel_row = row.saturating_sub(first_row) as usize;
    if rel_row >= SHAPE_EDITOR_ROWS {
        return None;
    }
    let rel_col = column.saturating_sub(inner.x);
    let cell_column = SHAPE_CELL_STARTS
        .iter()
        .enumerate()
        .find_map(|(index, start)| {
            let end = start.saturating_add(SHAPE_CELL_WIDTH as u16);
            (rel_col >= *start && rel_col < end).then_some(index)
        })?;
    Some(rel_row * SHAPE_EDITOR_COLUMNS + cell_column)
}

fn render_location_search(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Location Search");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let search_view = field_value_view(
        &app.location_query,
        inner.width.saturating_sub(SEARCH_LABEL_WIDTH).max(1) as usize,
        true,
    );
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{:<width$}", "Search", width = SEARCH_LABEL_WIDTH as usize),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                search_view.value,
                Style::default().fg(Color::Black).bg(Color::Yellow),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "City               CC       Lat       Lon",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let visible_results = location_visible_result_count(inner);
    for (row, location) in app
        .location_results
        .iter()
        .take(visible_results)
        .enumerate()
    {
        let selected = row == app.location_selected;
        let style = if selected {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![Span::styled(city_label(location), style)]));
    }
    if app.location_results.is_empty() {
        let message = if app.location_query.value.is_empty() {
            "Type at least 2 characters to search"
        } else {
            "No matching locations"
        };
        lines.push(Line::from(Span::styled(
            message,
            Style::default().fg(Color::DarkGray),
        )));
    }

    frame.render_widget(Paragraph::new(lines), inner);
    frame.set_cursor_position(location_search_cursor(inner, &app.location_query));
}

fn location_visible_result_count(inner: Rect) -> usize {
    inner
        .height
        .saturating_sub(LOCATION_RESULT_HEADER_ROWS)
        .max(1) as usize
}

fn location_result_index_at(area: Rect, column: u16, row: u16) -> Option<usize> {
    let inner = location_search_inner(area);
    if column < inner.x || column >= inner.x.saturating_add(inner.width) {
        return None;
    }
    let first_result_row = inner.y.saturating_add(LOCATION_RESULT_HEADER_ROWS);
    if row < first_result_row {
        return None;
    }
    let index = row.saturating_sub(first_result_row) as usize;
    (index < location_visible_result_count(inner)).then_some(index)
}

fn location_search_inner(area: Rect) -> Rect {
    Block::default().borders(Borders::ALL).inner(area)
}

fn location_search_cursor(inner: Rect, field: &Field) -> Position {
    let search_view = field_value_view(
        field,
        inner.width.saturating_sub(SEARCH_LABEL_WIDTH).max(1) as usize,
        true,
    );
    Position::new(
        inner
            .x
            .saturating_add(SEARCH_LABEL_WIDTH)
            .saturating_add(search_view.cursor_col.min(u16::MAX as usize) as u16),
        inner.y,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FieldValueView {
    value: String,
    cursor_col: usize,
}

fn field_value_width(area: Rect) -> usize {
    area.width.saturating_sub(FIELD_LABEL_WIDTH).max(1) as usize
}

fn field_value_view(field: &Field, max_width: usize, keep_cursor_visible: bool) -> FieldValueView {
    if max_width == 0 {
        return FieldValueView {
            value: String::new(),
            cursor_col: 0,
        };
    }

    let chars = field.value.chars().collect::<Vec<_>>();
    let cursor_char = field.value[..field.cursor].chars().count().min(chars.len());
    if chars.len() <= max_width {
        return FieldValueView {
            value: field.value.clone(),
            cursor_col: cursor_char.min(max_width.saturating_sub(1)),
        };
    }

    let mut start = if keep_cursor_visible && cursor_char >= max_width {
        cursor_char + 1 - max_width
    } else {
        0
    };
    start = start.min(chars.len().saturating_sub(max_width));
    let end = (start + max_width).min(chars.len());
    let mut visible = chars[start..end].iter().copied().collect::<Vec<_>>();
    if start > 0 && !visible.is_empty() {
        visible[0] = '<';
    }
    if end < chars.len() && !visible.is_empty() {
        let last = visible.len() - 1;
        visible[last] = '>';
    }

    FieldValueView {
        value: visible.into_iter().collect(),
        cursor_col: cursor_char
            .saturating_sub(start)
            .min(max_width.saturating_sub(1)),
    }
}

fn render_consumer(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App, focused: bool) {
    let block = panel_block("Consumer", Panel::Consumer.toggle_key(), focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let value_width = field_value_width(inner);
    let lines = app
        .consumer_fields
        .iter()
        .enumerate()
        .map(|(index, field)| {
            let selected = focused && index == app.consumer_selected;
            let style = match (selected, app.mode) {
                (true, Mode::Edit) => Style::default().fg(Color::Black).bg(Color::Yellow),
                (true, Mode::Normal) => Style::default().fg(Color::Black).bg(Color::Cyan),
                _ => Style::default(),
            };
            let label_style = if selected {
                style
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let value = if index == CONSUMER_SHAPE_FIELD_INDEX {
                consumer_shape_summary(&app.consumer_shape, value_width)
            } else {
                field_value_view(field, value_width, selected).value
            };
            Line::from(vec![
                Span::styled(
                    format!(
                        "{:<width$}",
                        field.label,
                        width = FIELD_LABEL_WIDTH as usize
                    ),
                    label_style,
                ),
                Span::styled(format!("{:<width$}", value, width = value_width), style),
            ])
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), inner);

    if focused && app.mode == Mode::Edit && app.consumer_selected != CONSUMER_SHAPE_FIELD_INDEX {
        let field = &app.consumer_fields[app.consumer_selected];
        let value_view = field_value_view(field, value_width, true);
        let y = inner.y.saturating_add(app.consumer_selected as u16);
        let x = inner
            .x
            .saturating_add(FIELD_LABEL_WIDTH)
            .saturating_add(value_view.cursor_col.min(u16::MAX as usize) as u16);
        frame.set_cursor_position(Position::new(x, y));
    }
}

fn render_simulation(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App, focused: bool) {
    let block = panel_block("Simulation", Panel::Simulation.toggle_key(), focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let value_width = field_value_width(inner);
    let mut lines = app
        .simulation_fields
        .iter()
        .enumerate()
        .map(|(index, field)| simulation_field_line(app, focused, index, field, value_width))
        .collect::<Vec<_>>();
    lines.push(simulation_run_line(app, focused, value_width));
    lines.push(Line::from(""));
    lines.extend(
        app.simulation_result
            .as_ref()
            .map(simulation_result_lines)
            .unwrap_or_else(simulation_empty_lines),
    );
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);

    if focused && app.mode == Mode::Edit && app.simulation_selected < app.simulation_fields.len() {
        let field = &app.simulation_fields[app.simulation_selected];
        let value_view = field_value_view(field, value_width, true);
        let y = inner.y.saturating_add(app.simulation_selected as u16);
        let x = inner
            .x
            .saturating_add(FIELD_LABEL_WIDTH)
            .saturating_add(value_view.cursor_col.min(u16::MAX as usize) as u16);
        frame.set_cursor_position(Position::new(x, y));
    }
}

fn simulation_field_line(
    app: &App,
    focused: bool,
    index: usize,
    field: &Field,
    value_width: usize,
) -> Line<'static> {
    let selected = focused && app.simulation_selected == index;
    let style = match (selected, app.mode) {
        (true, Mode::Edit) => Style::default().fg(Color::Black).bg(Color::Yellow),
        (true, Mode::Normal) => Style::default().fg(Color::Black).bg(Color::Cyan),
        _ => Style::default(),
    };
    let label_style = if selected {
        style
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let value = if index == SIMULATION_SEED_FIELD_INDEX && field.value.trim().is_empty() {
        "default".to_string()
    } else {
        field_value_view(field, value_width, selected).value
    };
    Line::from(vec![
        Span::styled(
            format!(
                "{:<width$}",
                field.label,
                width = FIELD_LABEL_WIDTH as usize
            ),
            label_style,
        ),
        Span::styled(format!("{:<width$}", value, width = value_width), style),
    ])
}

fn simulation_run_line(app: &App, focused: bool, value_width: usize) -> Line<'static> {
    let selected = focused && app.simulation_selected == SIMULATION_RUN_ROW_INDEX;
    let style = if selected {
        Style::default().fg(Color::Black).bg(Color::Cyan)
    } else {
        Style::default()
    };
    let label_style = if selected {
        style
    } else {
        Style::default().fg(Color::DarkGray)
    };
    Line::from(vec![
        Span::styled(
            format!("{:<width$}", "Run", width = FIELD_LABEL_WIDTH as usize),
            label_style,
        ),
        Span::styled(format!("{:<width$}", "[Run]", width = value_width), style),
    ])
}

fn simulation_empty_lines() -> Vec<Line<'static>> {
    vec![
        estimate_metric_line(
            "Status",
            "Select Run or press r".to_string(),
            Style::default(),
        ),
        estimate_metric_line("Import", "-".to_string(), Style::default()),
        estimate_metric_line("Export", "-".to_string(), Style::default()),
        estimate_metric_line("Self use", "-".to_string(), Style::default()),
    ]
}

fn simulation_result_lines(result: &SimulationResult) -> Vec<Line<'static>> {
    let summaries = &result.summaries;
    vec![
        estimate_metric_line(
            "Import",
            format_kwh_summary(summaries.grid_import_kwh),
            Style::default().fg(Color::Green),
        ),
        estimate_metric_line(
            "Export",
            format_kwh_summary(summaries.grid_export_kwh),
            Style::default(),
        ),
        estimate_metric_line(
            "Self use",
            format_kwh_summary(summaries.self_consumed_kwh),
            Style::default(),
        ),
        estimate_metric_line(
            "Self suff",
            format_ratio_summary(summaries.self_sufficiency_ratio),
            Style::default().fg(Color::Green),
        ),
        estimate_metric_line(
            "Self cons",
            format_ratio_summary(summaries.self_consumption_ratio),
            Style::default(),
        ),
        estimate_metric_line(
            "Losses",
            format_kwh_summary(summaries.battery_losses_kwh),
            Style::default(),
        ),
    ]
}

fn format_kwh_summary(summary: MetricSummary) -> String {
    format!(
        "{:.0} ({:.0}..{:.0}) kWh",
        summary.mean, summary.p10, summary.p90
    )
}

fn format_ratio_summary(summary: MetricSummary) -> String {
    format!(
        "{:.0}% ({:.0}..{:.0})",
        summary.mean * 100.0,
        summary.p10 * 100.0,
        summary.p90 * 100.0
    )
}

fn simulation_run_elapsed(run: &SimulationRunState) -> Duration {
    run.finished_at
        .map(|finished_at| finished_at.saturating_duration_since(run.started_at))
        .unwrap_or_else(|| run.started_at.elapsed())
}

fn render_simulation_run(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title("Simulation Run");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(run) = &app.simulation_run else {
        frame.render_widget(Paragraph::new("No simulation is running"), inner);
        return;
    };

    let completed = run.completed_runs.load(Ordering::Relaxed);
    let progress = if run.requested_runs == 0 {
        0.0
    } else {
        completed as f64 / run.requested_runs as f64
    };
    let elapsed = simulation_run_elapsed(run);
    let eta = simulation_eta(elapsed, completed, run.requested_runs, run.finished);
    let state = if let Some(error) = &run.error {
        format!("Failed: {error}")
    } else if run.finished {
        app.simulation_result
            .as_ref()
            .map(|result| {
                if result.cancelled {
                    "Cancelled"
                } else {
                    "Completed"
                }
            })
            .unwrap_or("Finished")
            .to_string()
    } else if run.cancelling {
        "Cancelling".to_string()
    } else {
        "Running".to_string()
    };

    let mut lines = vec![
        estimate_metric_line("Status", state, Style::default().fg(Color::Green)),
        estimate_metric_line(
            "Runs",
            format!("{completed}/{}", run.requested_runs),
            Style::default(),
        ),
        estimate_metric_line("Progress", progress_bar(progress, 24), Style::default()),
        estimate_metric_line("Elapsed", format_duration(elapsed), Style::default()),
        estimate_metric_line(
            "ETA",
            eta.unwrap_or_else(|| "-".to_string()),
            Style::default(),
        ),
        Line::from(""),
    ];

    if run.finished {
        if let Some(result) = &app.simulation_result {
            lines.extend(simulation_result_lines(result));
        }
        lines.push(Line::from(""));
        lines.push(Line::from("Enter or Esc returns to the main screen"));
    } else {
        lines.push(Line::from("Esc or c cancels the run"));
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

fn progress_bar(progress: f64, width: usize) -> String {
    let progress = progress.clamp(0.0, 1.0);
    let filled = (progress * width as f64).round() as usize;
    format!(
        "[{}{}] {:>3}%",
        "#".repeat(filled),
        ".".repeat(width.saturating_sub(filled)),
        (progress * 100.0).round() as usize
    )
}

fn simulation_eta(
    elapsed: Duration,
    completed: usize,
    requested: usize,
    finished: bool,
) -> Option<String> {
    if finished {
        return Some("0s".to_string());
    }
    if completed == 0 || completed >= requested {
        return None;
    }
    let seconds_per_run = elapsed.as_secs_f64() / completed as f64;
    let remaining = seconds_per_run * (requested - completed) as f64;
    Some(format_duration(Duration::from_secs_f64(remaining.max(0.0))))
}

fn format_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    }
}

fn render_estimate(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App, focused: bool) {
    let block = panel_block("Estimate", Panel::Estimate.toggle_key(), focused);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(document) = &app.estimate else {
        frame.render_widget(
            Paragraph::new(vec![annual_energy_line(None), Line::from("No estimate")]),
            inner,
        );
        return;
    };

    let estimate = &document.ensemble_estimate;
    let sources = document
        .coverage
        .applicable_sources
        .iter()
        .map(|source| source.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let mut header_lines = vec![annual_energy_line(Some(document))];
    if let Some(price) = app.energy_price_eur_per_kwh().ok().flatten() {
        header_lines.push(annual_revenue_line(document, price));
    }
    header_lines.push(estimate_metric_line(
        "POA",
        format!(
            "{:.2} kWh/m2",
            estimate
                .annual_in_plane_irradiation
                .mean
                .as_kilowatt_hours_per_square_meter()
        ),
        Style::default(),
    ));
    header_lines.push(estimate_metric_line("Sources", sources, Style::default()));
    let header_height = header_lines.len().min(u16::MAX as usize) as u16;
    let header = Paragraph::new(header_lines);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(header_height), Constraint::Min(5)])
        .split(inner);
    frame.render_widget(header.wrap(Wrap { trim: true }), chunks[0]);

    let mut rows = Vec::new();
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
                    format!("{low:.0}"),
                    format!("{high:.0}"),
                    format!("{:.1}", low / days),
                    format!("{:.1}", high / days),
                )
            })
            .unwrap_or_else(|| {
                (
                    "-".to_string(),
                    "-".to_string(),
                    "-".to_string(),
                    "-".to_string(),
                )
            });
        rows.push([
            month_name.to_string(),
            format!("{total_kwh:.1}"),
            total_min,
            total_max,
            format!("{:.1}", total_kwh / days),
            daily_min,
            daily_max,
        ]);
    }

    frame.render_widget(
        Paragraph::new(monthly_table_lines(
            &rows,
            app.estimate_scroll,
            chunks[1].height,
        )),
        chunks[1],
    );
}

fn monthly_table_minimums(rows: &[[String; 7]]) -> Option<(f64, f64)> {
    if rows.is_empty() {
        return None;
    }
    Some((min_table_column(rows, 2)?, min_table_column(rows, 5)?))
}

fn min_table_column(rows: &[[String; 7]], index: usize) -> Option<f64> {
    rows.iter()
        .filter_map(|row| row[index].parse::<f64>().ok())
        .min_by(f64::total_cmp)
}

fn is_table_minimum(value: &str, minimum: f64) -> bool {
    value
        .parse::<f64>()
        .map(|parsed| parsed.total_cmp(&minimum).is_eq())
        .unwrap_or(false)
}

fn monthly_table_column_widths(rows: &[[String; 7]]) -> [usize; 7] {
    let mut column_widths = [0usize; 7];
    for (index, header) in MONTHLY_TABLE_HEADERS.iter().enumerate() {
        column_widths[index] = header.len();
    }
    for row in rows {
        for (index, value) in row.iter().enumerate() {
            column_widths[index] = column_widths[index].max(value.len());
        }
    }
    column_widths
}

#[cfg(test)]
fn monthly_table_text_lines(rows: &[[String; 7]]) -> Vec<String> {
    let column_widths = monthly_table_column_widths(rows);
    let monthly_width = column_widths[1] + column_widths[2] + column_widths[3] + 2;
    let daily_width = column_widths[4] + column_widths[5] + column_widths[6] + 2;

    let mut lines = vec![
        String::new(),
        format!(
            "{:<month_width$} | {:<monthly_width$} | {:<daily_width$}",
            "",
            "Monthly kWh",
            "Daily kWh",
            month_width = column_widths[0],
        ),
        monthly_table_text_row(&MONTHLY_TABLE_HEADERS.map(str::to_string), column_widths),
    ];
    for row in rows {
        lines.push(monthly_table_text_row(row, column_widths));
    }
    lines
}

#[cfg(test)]
fn monthly_table_text_row(row: &[String; 7], column_widths: [usize; 7]) -> String {
    format!(
        "{:<month_width$} | {:<monthly_mean_width$} {:<monthly_min_width$} {:<monthly_max_width$} | {:<daily_mean_width$} {:<daily_min_width$} {:<daily_max_width$}",
        row[0],
        row[1],
        row[2],
        row[3],
        row[4],
        row[5],
        row[6],
        month_width = column_widths[0],
        monthly_mean_width = column_widths[1],
        monthly_min_width = column_widths[2],
        monthly_max_width = column_widths[3],
        daily_mean_width = column_widths[4],
        daily_min_width = column_widths[5],
        daily_max_width = column_widths[6],
    )
}

fn monthly_table_lines(rows: &[[String; 7]], scroll: usize, height: u16) -> Vec<Line<'static>> {
    let column_widths = monthly_table_column_widths(rows);
    let monthly_width = column_widths[1] + column_widths[2] + column_widths[3] + 2;
    let daily_width = column_widths[4] + column_widths[5] + column_widths[6] + 2;
    let header_style = Style::default().fg(Color::DarkGray);
    let visible_rows = monthly_table_visible_row_count(height);
    let scroll = monthly_table_scroll_start(scroll, rows.len(), visible_rows);

    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!(
                "{:<month_width$} | {:<monthly_width$} | {:<daily_width$}",
                "",
                "Monthly kWh",
                "Daily kWh",
                month_width = column_widths[0],
            ),
            header_style,
        )),
        monthly_table_line(
            &MONTHLY_TABLE_HEADERS.map(str::to_string),
            column_widths,
            true,
            None,
        ),
    ];
    let minimums = monthly_table_minimums(rows);
    for row in rows.iter().skip(scroll).take(visible_rows) {
        lines.push(monthly_table_line(row, column_widths, false, minimums));
    }
    lines
}

fn monthly_table_visible_row_count(height: u16) -> usize {
    (height as usize)
        .saturating_sub(MONTHLY_TABLE_HEADER_ROWS)
        .max(1)
}

fn monthly_table_scroll_start(scroll: usize, row_count: usize, visible_rows: usize) -> usize {
    scroll.min(row_count.saturating_sub(visible_rows))
}

fn monthly_table_line(
    row: &[String; 7],
    column_widths: [usize; 7],
    is_header: bool,
    minimums: Option<(f64, f64)>,
) -> Line<'static> {
    let label_style = Style::default().fg(Color::DarkGray);
    let value_style = Style::default();
    let base_style = if is_header { label_style } else { value_style };
    let mean_style = Style::default().fg(Color::Green);
    let minimum_style = Style::default().fg(Color::Red);
    let monthly_min_style = minimums
        .filter(|(monthly_min, _)| is_table_minimum(&row[2], *monthly_min))
        .map(|_| minimum_style)
        .unwrap_or(base_style);
    let daily_min_style = minimums
        .filter(|(_, daily_min)| is_table_minimum(&row[5], *daily_min))
        .map(|_| minimum_style)
        .unwrap_or(base_style);
    Line::from(vec![
        Span::styled(
            format!("{:<width$}", row[0], width = column_widths[0]),
            label_style,
        ),
        Span::styled(" | ", base_style),
        Span::styled(
            format!("{:<width$}", row[1], width = column_widths[1]),
            mean_style,
        ),
        Span::styled(" ", base_style),
        Span::styled(
            format!("{:<width$}", row[2], width = column_widths[2]),
            monthly_min_style,
        ),
        Span::styled(" ", base_style),
        Span::styled(
            format!("{:<width$}", row[3], width = column_widths[3]),
            base_style,
        ),
        Span::styled(" | ", base_style),
        Span::styled(
            format!("{:<width$}", row[4], width = column_widths[4]),
            mean_style,
        ),
        Span::styled(" ", base_style),
        Span::styled(
            format!("{:<width$}", row[5], width = column_widths[5]),
            daily_min_style,
        ),
        Span::styled(" ", base_style),
        Span::styled(
            format!("{:<width$}", row[6], width = column_widths[6]),
            base_style,
        ),
    ])
}

fn array_extra_line_count(app: &App) -> u16 {
    app.fields
        .iter()
        .find(|field| field.label == "Arrays")
        .map(|field| {
            let parsed_lines = parse_arrays(field)
                .ok()
                .map(|arrays| arrays.len().saturating_add(1).min(u16::MAX as usize) as u16)
                .unwrap_or(0);
            1 + parsed_lines
        })
        .unwrap_or(0)
}

fn total_array_kwp(arrays: &[EstimateArray]) -> f64 {
    arrays.iter().map(|array| array.peak_power_kwp).sum()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArrayFooterAction {
    Done,
    Add,
    Remove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShapeFooterAction {
    Done,
    Preset,
    Custom,
}

fn array_footer_hit(area: Rect, column: u16, row: u16) -> Option<ArrayFooterAction> {
    if row != area.y.saturating_add(1) {
        return None;
    }
    let hits = [
        (
            "ARRAYS  ".len() as u16,
            "[Done]".len() as u16,
            ArrayFooterAction::Done,
        ),
        (
            "ARRAYS  [Done]  ".len() as u16,
            "[Add]".len() as u16,
            ArrayFooterAction::Add,
        ),
        (
            "ARRAYS  [Done]  [Add]  ".len() as u16,
            "[Remove]".len() as u16,
            ArrayFooterAction::Remove,
        ),
    ];
    hits.into_iter().find_map(|(start, width, action)| {
        let start = area.x.saturating_add(start);
        let end = start.saturating_add(width);
        (column >= start && column < end).then_some(action)
    })
}

fn shape_footer_hit(area: Rect, column: u16, row: u16) -> Option<ShapeFooterAction> {
    if row != area.y.saturating_add(1) {
        return None;
    }
    let hits = [
        (
            "SHAPE  ".len() as u16,
            "[Done]".len() as u16,
            ShapeFooterAction::Done,
        ),
        (
            "SHAPE  [Done]  ".len() as u16,
            "[Preset]".len() as u16,
            ShapeFooterAction::Preset,
        ),
        (
            "SHAPE  [Done]  [Preset]  ".len() as u16,
            "[Custom]".len() as u16,
            ShapeFooterAction::Custom,
        ),
    ];
    hits.into_iter().find_map(|(start, width, action)| {
        let start = area.x.saturating_add(start);
        let end = start.saturating_add(width);
        (column >= start && column < end).then_some(action)
    })
}

fn render_footer(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let (mode, help) = match app.mode {
        Mode::Normal => (
            "NORMAL",
            "1-4 panels  tab focus  enter edit/run  e estimate  r simulate  q/esc",
        ),
        Mode::Edit if app.fields[app.selected].label == "Arrays" => {
            ("EDIT", "enter/tab apply estimate  esc close edit")
        }
        Mode::Edit => ("EDIT", "type value  enter apply  esc cancel edit  tab next"),
        Mode::Location => (
            "LOCATION",
            "type filter  arrows select  enter apply  esc cancel",
        ),
        Mode::Arrays if app.array_editing => (
            "ARRAYS",
            "type value  enter apply  tab next  esc cancel edit",
        ),
        Mode::Arrays => (
            "ARRAYS",
            "arrows select  enter edit  a add  d remove  esc done",
        ),
        Mode::Shape if app.shape_preset_selecting => ("SHAPE", "arrows enter apply  esc dismiss"),
        Mode::Shape if app.shape_editing => (
            "SHAPE",
            "type value  enter apply  tab next  esc cancel edit",
        ),
        Mode::Shape => (
            "SHAPE",
            "arrows select  enter edit  p presets  c custom  esc",
        ),
        Mode::SimulationRun
            if app
                .simulation_run
                .as_ref()
                .map(|run| run.finished)
                .unwrap_or(false) =>
        {
            ("SIM", "enter/esc return")
        }
        Mode::SimulationRun => ("SIM", "progress live  esc/c cancel"),
    };
    let status = Line::from(vec![
        Span::styled("Status ", Style::default().fg(Color::DarkGray)),
        Span::raw(app.status.as_str()),
    ]);
    let help = if app.mode == Mode::Location {
        Line::from(vec![
            Span::styled(mode, Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(
                "[Cancel]",
                Style::default().fg(Color::Black).bg(Color::Yellow),
            ),
            Span::raw("  "),
            Span::raw(help),
        ])
    } else if app.mode == Mode::Arrays {
        Line::from(vec![
            Span::styled(mode, Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(
                "[Done]",
                Style::default().fg(Color::Black).bg(Color::Yellow),
            ),
            Span::raw("  "),
            Span::styled("[Add]", Style::default().fg(Color::Black).bg(Color::Yellow)),
            Span::raw("  "),
            Span::styled(
                "[Remove]",
                Style::default().fg(Color::Black).bg(Color::Yellow),
            ),
            Span::raw("  "),
            Span::raw(help),
        ])
    } else if app.mode == Mode::Shape {
        Line::from(vec![
            Span::styled(mode, Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(
                "[Done]",
                Style::default().fg(Color::Black).bg(Color::Yellow),
            ),
            Span::raw("  "),
            Span::styled(
                "[Preset]",
                Style::default().fg(Color::Black).bg(Color::Yellow),
            ),
            Span::raw("  "),
            Span::styled(
                "[Custom]",
                Style::default().fg(Color::Black).bg(Color::Yellow),
            ),
            Span::raw("  "),
            Span::raw(help),
        ])
    } else {
        Line::from(vec![
            Span::styled(mode, Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::raw(help),
        ])
    };
    frame.render_widget(Paragraph::new(vec![status, help]), area);
}

fn location_cancel_hit(area: Rect, column: u16, row: u16) -> bool {
    let cancel_start = "LOCATION  ".len() as u16;
    let cancel_end = cancel_start + "[Cancel]".len() as u16;
    row == area.y.saturating_add(1)
        && column >= area.x.saturating_add(cancel_start)
        && column < area.x.saturating_add(cancel_end)
}

fn azimuth_direction_label(value: &str) -> Option<&'static str> {
    let degrees = value.parse::<f64>().ok()?;
    let compass_degrees = (180.0 + degrees).rem_euclid(360.0);
    let index = ((compass_degrees + 22.5) / 45.0).floor() as usize % 8;
    Some(["N", "NE", "E", "SE", "S", "SW", "W", "NW"][index])
}

fn city_label(location: &CitySearchResult) -> String {
    format!(
        "  {:<16} {:>2} {:>8.3} {:>9.3}",
        truncate(&location.display_name, 16),
        location.country_code,
        location.latitude_degrees,
        location.longitude_degrees,
    )
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        truncated
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use pv_core::prelude::{
        AnnualPvEnsembleEstimate, Energy, EstimateCoverage, EstimateLocation, EstimateSystem,
        Irradiation, MonthOfYear, SourceAnnualPvEstimate, SourceMonthlyPvEstimate, WeatherSourceId,
    };
    use ratatui::backend::TestBackend;

    const SNAPSHOT_SIZE: (u16, u16) = (80, 24);

    fn render_snapshot(app: &App) -> String {
        let backend = TestBackend::new(SNAPSHOT_SIZE.0, SNAPSHOT_SIZE.1);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal.draw(|frame| render(frame, app)).expect("draw TUI");
        terminal.backend().to_string()
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn populated_estimate_document() -> SourceEnsembleEstimateDocument {
        let source_a = source_estimate("fixture-a", 0.0);
        let source_b = source_estimate("fixture-b", 12.0);
        let ensemble = AnnualPvEnsembleEstimate::from_source_estimates(vec![source_a, source_b])
            .expect("fixture has source estimates");
        SourceEnsembleEstimateDocument {
            schema_version: 1,
            location: EstimateLocation {
                location_id: "fixture".to_string(),
                name: "Fixture".to_string(),
                region: "IT".to_string(),
                latitude: 45.0,
                longitude: 9.0,
            },
            system: EstimateSystem {
                peak_power_kwp: 3.0,
                loss_pct: 14.0,
                tilt_deg: 30.0,
                aspect_deg: 0.0,
                storage_usable_kwh: None,
            },
            coverage: EstimateCoverage {
                pvgis_sarah3_applicable: true,
                applicable_sources: vec![
                    WeatherSourceId::new("fixture-a").expect("valid source id"),
                    WeatherSourceId::new("fixture-b").expect("valid source id"),
                ],
            },
            ensemble_estimate: ensemble,
            references: serde_json::json!({}),
        }
    }

    fn populated_simulation_result() -> SimulationResult {
        let summary = |mean, p10, p90| MetricSummary {
            mean,
            p10,
            p90,
            p50: mean,
        };
        SimulationResult {
            requested_runs: 10_000,
            completed_runs: 10_000,
            cancelled: false,
            summaries: SimulationMetricSummaries {
                production_kwh: summary(1700.0, 1500.0, 1900.0),
                load_kwh: summary(4200.0, 4200.0, 4200.0),
                self_consumed_kwh: summary(1100.0, 1000.0, 1200.0),
                grid_import_kwh: summary(3100.0, 3000.0, 3200.0),
                grid_export_kwh: summary(600.0, 500.0, 700.0),
                battery_losses_kwh: summary(0.0, 0.0, 0.0),
                ending_soc_kwh: summary(0.0, 0.0, 0.0),
                self_consumption_ratio: summary(0.65, 0.60, 0.70),
                self_sufficiency_ratio: summary(0.26, 0.24, 0.29),
            },
        }
    }

    fn source_estimate(source_id: &str, offset: f64) -> SourceAnnualPvEstimate {
        let monthly = (1..=12)
            .map(|month| {
                let energy = 70.0 + month as f64 * 10.0 + offset;
                let poa = 90.0 + month as f64 * 8.0 + offset;
                SourceMonthlyPvEstimate {
                    month: MonthOfYear::new(month).expect("valid month"),
                    energy: Energy::from_kilowatt_hours(energy),
                    in_plane_irradiation: Irradiation::from_kilowatt_hours_per_square_meter(poa),
                    global_horizontal_irradiation:
                        Irradiation::from_kilowatt_hours_per_square_meter(poa - 15.0),
                }
            })
            .collect::<Vec<_>>();
        SourceAnnualPvEstimate {
            weather_source_id: WeatherSourceId::new(source_id).expect("valid source id"),
            annual_energy: Energy::from_kilowatt_hours(
                monthly
                    .iter()
                    .map(|month| month.energy.as_kilowatt_hours())
                    .sum(),
            ),
            annual_in_plane_irradiation: Irradiation::from_kilowatt_hours_per_square_meter(
                monthly
                    .iter()
                    .map(|month| {
                        month
                            .in_plane_irradiation
                            .as_kilowatt_hours_per_square_meter()
                    })
                    .sum(),
            ),
            annual_global_horizontal_irradiation: Irradiation::from_kilowatt_hours_per_square_meter(
                monthly
                    .iter()
                    .map(|month| {
                        month
                            .global_horizontal_irradiation
                            .as_kilowatt_hours_per_square_meter()
                    })
                    .sum(),
            ),
            monthly_estimates: monthly,
        }
    }

    fn assert_snapshot(name: &str, actual: &str) {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src/snapshots")
            .join(format!("{name}.snap"));
        if std::env::var_os("PV_TUI_UPDATE_SNAPSHOTS").is_some() {
            std::fs::write(&path, actual).expect("write snapshot");
            return;
        }
        let expected = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("reading snapshot {}: {error}", path.display()));
        assert_eq!(actual, expected, "snapshot {} changed", path.display());
    }

    #[test]
    fn parses_consumer_load_profile_from_annual_energy() {
        let app = App::new();
        let profile = app.consumer_load_profile().expect("valid default consumer");

        match profile {
            LoadProfile::AnnualKwh { annual_kwh, shape } => {
                assert_eq!(annual_kwh, 4200.0);
                assert!(matches!(
                    shape,
                    LoadShape::BuiltIn {
                        shape_id: BuiltInLoadShapeId::ResidentialDefault
                    }
                ));
            }
            _ => panic!("expected annual load profile"),
        }
    }

    #[test]
    fn consumer_load_profile_prefers_annual_when_both_energy_fields_are_set() {
        let mut app = App::new();
        app.consumer_fields[CONSUMER_DAILY_FIELD_INDEX].set_value("12");

        assert!(matches!(
            app.consumer_load_profile().expect("annual profile"),
            LoadProfile::AnnualKwh {
                annual_kwh: 4200.0,
                ..
            }
        ));
    }

    #[test]
    fn consumer_load_profile_accepts_daily_energy_when_annual_empty() {
        let mut app = App::new();
        app.consumer_fields[CONSUMER_ANNUAL_FIELD_INDEX].set_value("");
        app.consumer_fields[CONSUMER_DAILY_FIELD_INDEX].set_value("12");

        assert!(matches!(
            app.consumer_load_profile().expect("daily profile"),
            LoadProfile::DailyKwh {
                daily_kwh: 12.0,
                ..
            }
        ));
    }

    #[test]
    fn consumer_load_profile_uses_custom_hourly_shape() {
        let mut app = App::new();
        let mut weights = vec![0.0; 24];
        weights[18] = 1.0;
        app.consumer_shape = ConsumerShapeState::HourlyWeights {
            weights: weights.clone(),
        };

        let profile = app.consumer_load_profile().expect("custom profile");

        match profile {
            LoadProfile::AnnualKwh { shape, .. } => {
                assert_eq!(shape, LoadShape::HourlyWeights { weights });
            }
            _ => panic!("expected annual load profile"),
        }
    }

    #[test]
    fn custom_shape_weights_require_24_finite_non_negative_values_with_positive_sum() {
        assert!(validate_shape_weights(&[1.0; 24]).is_ok());
        assert!(validate_shape_weights(&[0.0; 24]).is_err());
        assert!(validate_shape_weights(&[1.0; 23]).is_err());

        let mut weights = [1.0; 24];
        weights[3] = -1.0;
        assert!(validate_shape_weights(&weights).is_err());

        let mut weights = [1.0; 24];
        weights[3] = f64::NAN;
        assert!(validate_shape_weights(&weights).is_err());
    }

    #[test]
    fn shape_editor_opens_from_consumer_shape_row() {
        let mut app = App::new();
        app.panel_visibility.set_visible(Panel::Consumer, true);
        app.focused_panel = Panel::Consumer;
        app.consumer_selected = CONSUMER_SHAPE_FIELD_INDEX;
        let mut estimator = SourceModelEstimator::load_embedded().expect("embedded estimator");

        let quit = handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
            &mut app,
            &mut estimator,
        )
        .expect("key handled");

        assert!(!quit);
        assert_eq!(app.mode, Mode::Shape);
    }

    #[test]
    fn editing_preset_shape_copies_it_to_custom_first() {
        let mut app = App::new();
        app.shape_selected_hour = 18;

        app.start_shape_cell_edit();

        assert!(app.shape_editing);
        assert_eq!(app.shape_cell.value, "1.55");
        assert_eq!(
            app.consumer_shape,
            ConsumerShapeState::HourlyWeights {
                weights: RESIDENTIAL_DEFAULT_WEIGHTS.to_vec(),
            }
        );
    }

    #[test]
    fn switching_to_custom_shape_copies_preset_weights() {
        let mut app = App::new();

        app.set_shape_custom();

        assert_eq!(
            app.consumer_shape,
            ConsumerShapeState::HourlyWeights {
                weights: RESIDENTIAL_DEFAULT_WEIGHTS.to_vec(),
            }
        );
    }

    #[test]
    fn preset_picker_does_not_apply_until_confirmed() {
        let mut app = App::new();
        app.set_shape_custom();
        let custom = app.consumer_shape.clone();

        app.open_shape_preset_picker();
        app.move_shape_preset_selection(1);

        assert_eq!(app.consumer_shape, custom);
        assert!(app.shape_preset_selecting);

        app.apply_shape_preset_picker();
        assert_eq!(
            app.consumer_shape,
            ConsumerShapeState::BuiltIn {
                shape_id: BuiltInLoadShapeId::Flat,
            }
        );
        assert!(!app.shape_preset_selecting);
    }

    #[test]
    fn preset_picker_can_be_dismissed_without_replacing_custom_weights() {
        let mut app = App::new();
        app.set_shape_custom();
        let custom = app.consumer_shape.clone();

        app.open_shape_preset_picker();
        app.move_shape_preset_selection(1);
        app.dismiss_shape_preset_picker();

        assert_eq!(app.consumer_shape, custom);
        assert!(!app.shape_preset_selecting);
    }

    #[test]
    fn built_in_shape_weights_support_all_presets() {
        for shape_id in BUILT_IN_LOAD_SHAPES {
            let weights = built_in_shape_weights(shape_id);
            assert_eq!(weights.len(), 24);
            assert!(validate_shape_weights(weights).is_ok());
        }
    }

    #[test]
    fn applying_custom_shape_weight_updates_selected_hour() {
        let mut app = App::new();
        app.set_shape_custom();
        app.shape_selected_hour = 18;
        app.shape_cell.set_value("2.5");
        app.shape_editing = true;

        app.apply_shape_cell_edit();

        let ConsumerShapeState::HourlyWeights { weights } = app.consumer_shape else {
            panic!("expected custom shape");
        };
        assert_eq!(weights[18], 2.5);
        assert!(!app.shape_editing);
    }

    #[test]
    fn consumer_annual_edit_updates_daily_energy() {
        let mut app = App::new();
        app.focused_panel = Panel::Consumer;
        app.consumer_selected = CONSUMER_ANNUAL_FIELD_INDEX;
        app.mode = Mode::Edit;
        app.consumer_fields[CONSUMER_ANNUAL_FIELD_INDEX].set_value("3650");
        let mut estimator = SourceModelEstimator::load_embedded().expect("embedded estimator");

        app.apply_active_edit(&mut estimator, 0);

        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.consumer_fields[CONSUMER_DAILY_FIELD_INDEX].value, "10");
    }

    #[test]
    fn consumer_daily_edit_updates_annual_energy() {
        let mut app = App::new();
        app.focused_panel = Panel::Consumer;
        app.consumer_selected = CONSUMER_DAILY_FIELD_INDEX;
        app.mode = Mode::Edit;
        app.consumer_fields[CONSUMER_DAILY_FIELD_INDEX].set_value("12");
        let mut estimator = SourceModelEstimator::load_embedded().expect("embedded estimator");

        app.apply_active_edit(&mut estimator, 0);

        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(
            app.consumer_fields[CONSUMER_ANNUAL_FIELD_INDEX].value,
            "4380"
        );
    }

    #[test]
    fn consumer_edit_validation_keeps_invalid_field_in_edit_mode() {
        let mut app = App::new();
        app.focused_panel = Panel::Consumer;
        app.consumer_selected = CONSUMER_ANNUAL_FIELD_INDEX;
        app.mode = Mode::Edit;
        app.consumer_fields[CONSUMER_ANNUAL_FIELD_INDEX].set_value("0");
        let mut estimator = SourceModelEstimator::load_embedded().expect("embedded estimator");

        app.apply_active_edit(&mut estimator, 0);

        assert_eq!(app.mode, Mode::Edit);
        assert!(app.status.contains("Annual kWh"));
    }

    #[test]
    fn parses_simulation_options_from_fields() {
        let mut app = App::new();
        app.simulation_fields[SIMULATION_RUNS_FIELD_INDEX].set_value("250");
        app.simulation_fields[SIMULATION_SEED_FIELD_INDEX].set_value("42");

        let options = app.simulation_options().expect("valid simulation options");

        assert_eq!(options.runs, 250);
        assert_eq!(options.seed, Some(42));
    }

    #[test]
    fn simulation_options_reject_invalid_runs_and_seed() {
        let mut app = App::new();
        app.simulation_fields[SIMULATION_RUNS_FIELD_INDEX].set_value("0");
        assert!(app.simulation_options().is_err());

        app.simulation_fields[SIMULATION_RUNS_FIELD_INDEX].set_value("10");
        app.simulation_fields[SIMULATION_SEED_FIELD_INDEX].set_value("abc");
        assert!(app.simulation_options().is_err());
    }

    #[test]
    fn simulation_panel_enter_edits_options_and_uses_run_row() {
        let mut app = App::new();
        app.panel_visibility.set_visible(Panel::Simulation, true);
        app.focused_panel = Panel::Simulation;
        app.simulation_selected = SIMULATION_RUNS_FIELD_INDEX;
        let mut estimator = SourceModelEstimator::load_embedded().expect("embedded estimator");

        let quit = handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
            &mut app,
            &mut estimator,
        )
        .expect("key handled");

        assert!(!quit);
        assert_eq!(app.mode, Mode::Edit);

        app.mode = Mode::Normal;
        app.simulation_selected = SIMULATION_RUN_ROW_INDEX;
        app.simulation_fields[SIMULATION_RUNS_FIELD_INDEX].set_value("0");

        let quit = handle_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
            &mut app,
            &mut estimator,
        )
        .expect("key handled");

        assert!(!quit);
        assert_eq!(app.mode, Mode::Normal);
        assert!(app.status.contains("Runs must be a positive integer"));
    }

    #[test]
    fn simulation_run_key_cancels_without_enter_while_running() {
        let mut app = App::new();
        let (sender, receiver) = mpsc::channel();
        drop(sender);
        let cancel = Arc::new(AtomicBool::new(false));
        app.mode = Mode::SimulationRun;
        app.simulation_run = Some(SimulationRunState {
            requested_runs: 100,
            completed_runs: Arc::new(AtomicUsize::new(25)),
            cancel: Arc::clone(&cancel),
            started_at: Instant::now(),
            finished_at: None,
            receiver,
            finished: false,
            cancelling: false,
            error: None,
        });

        handle_simulation_run_key(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()),
            &mut app,
        )
        .expect("enter handled");
        assert!(!cancel.load(Ordering::Relaxed));
        assert_eq!(app.mode, Mode::SimulationRun);

        handle_simulation_run_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()), &mut app)
            .expect("esc handled");
        assert!(cancel.load(Ordering::Relaxed));
        assert_eq!(app.status, "Cancelling simulation");
    }

    #[test]
    fn simulation_run_screen_shows_progress_and_cancel_hint() {
        let mut app = App::new();
        let (sender, receiver) = mpsc::channel();
        drop(sender);
        app.mode = Mode::SimulationRun;
        app.simulation_run = Some(SimulationRunState {
            requested_runs: 100,
            completed_runs: Arc::new(AtomicUsize::new(25)),
            cancel: Arc::new(AtomicBool::new(false)),
            started_at: Instant::now(),
            finished_at: None,
            receiver,
            finished: false,
            cancelling: false,
            error: None,
        });

        let snapshot = render_snapshot(&app);

        assert!(snapshot.contains("Simulation Run"));
        assert!(snapshot.contains("25/100"));
        assert!(snapshot.contains("Esc or c cancels the run"));
    }

    #[test]
    fn simulation_progress_helpers_are_stable() {
        assert_eq!(progress_bar(0.25, 8), "[##......]  25%");
        assert_eq!(format_duration(Duration::from_secs(65)), "1m 5s");
        assert_eq!(
            simulation_eta(Duration::from_secs(10), 25, 100, false),
            Some("30s".to_string())
        );
        assert_eq!(
            simulation_eta(Duration::from_secs(10), 100, 100, true),
            Some("0s".to_string())
        );

        let (sender, receiver) = mpsc::channel();
        drop(sender);
        let started_at = Instant::now();
        let run = SimulationRunState {
            requested_runs: 100,
            completed_runs: Arc::new(AtomicUsize::new(100)),
            cancel: Arc::new(AtomicBool::new(false)),
            started_at,
            finished_at: Some(started_at + Duration::from_secs(12)),
            receiver,
            finished: true,
            cancelling: false,
            error: None,
        };
        assert_eq!(simulation_run_elapsed(&run), Duration::from_secs(12));
    }

    #[test]
    fn panel_visibility_defaults_to_system_and_estimate() {
        let app = App::new();

        assert_eq!(app.visible_panels(), vec![Panel::System, Panel::Estimate]);
        assert_eq!(app.focused_panel, Panel::System);
    }

    #[test]
    fn panel_toggles_keep_one_panel_visible_and_focus_new_panel() {
        let mut app = App::new();
        app.toggle_panel(Panel::Consumer);

        assert!(app.panel_visibility.is_visible(Panel::Consumer));
        assert_eq!(app.focused_panel, Panel::Consumer);

        app.toggle_panel(Panel::System);
        app.toggle_panel(Panel::Estimate);
        app.toggle_panel(Panel::Consumer);

        assert!(app.panel_visibility.is_visible(Panel::Consumer));
        assert_eq!(app.status, "At least one panel must stay visible");
    }

    #[test]
    fn panel_layout_adapts_to_visible_panel_count() {
        let area = Rect::new(0, 0, 80, 22);
        let mut app = App::new();

        assert_eq!(panel_layout(area, &app).len(), 2);
        app.toggle_panel(Panel::Consumer);
        assert_eq!(panel_layout(area, &app).len(), 3);
        app.toggle_panel(Panel::Simulation);
        let layout = panel_layout(area, &app);
        assert_eq!(layout.len(), 4);
        assert_eq!(layout[0].0, Panel::System);
        assert_eq!(layout[1].0, Panel::Consumer);
        assert_eq!(layout[2].0, Panel::Simulation);
        assert_eq!(layout[3].0, Panel::Estimate);
        assert_eq!(layout[0].1.x, 0);
        assert_eq!(layout[1].1.x, 0);
        assert!(layout[1].1.y > layout[0].1.y);
        assert!(layout[2].1.x > layout[0].1.x);
        assert_eq!(layout[2].1.y, 0);
        assert_eq!(layout[3].1.x, layout[2].1.x);
        assert!(layout[3].1.y > layout[2].1.y);
        app.toggle_panel(Panel::System);
        app.toggle_panel(Panel::Estimate);
        app.toggle_panel(Panel::Consumer);
        assert_eq!(panel_layout(area, &app).len(), 1);
    }

    #[test]
    fn panel_focus_cycles_with_visible_panel_order() {
        let mut app = App::new();
        app.toggle_panel(Panel::Consumer);
        app.toggle_panel(Panel::Simulation);

        app.focused_panel = Panel::System;
        app.focus_next_panel(1);
        assert_eq!(app.focused_panel, Panel::Consumer);
        app.focus_next_panel(1);
        assert_eq!(app.focused_panel, Panel::Simulation);
        app.focus_next_panel(-1);
        assert_eq!(app.focused_panel, Panel::Consumer);
    }

    #[test]
    fn normal_mode_escape_quits() {
        let mut app = App::new();
        let mut estimator = SourceModelEstimator::load_embedded().expect("embedded estimator");

        let quit = handle_key(
            KeyEvent::new(KeyCode::Esc, KeyModifiers::empty()),
            &mut app,
            &mut estimator,
        )
        .expect("key handled");

        assert!(quit);
    }

    #[test]
    fn default_layout_snapshot() {
        let app = App::new();

        assert_snapshot("default_layout", &render_snapshot(&app));
    }

    #[test]
    fn empty_price_field_selected_snapshot() {
        let mut app = App::new();
        app.selected = PRICE_FIELD_INDEX;

        assert_snapshot("empty_price_selected", &render_snapshot(&app));
    }

    #[test]
    fn populated_estimate_snapshot() {
        let mut app = App::new();
        app.fields[PRICE_FIELD_INDEX].set_value("0.22");
        app.estimate = Some(populated_estimate_document());

        assert_snapshot("populated_estimate", &render_snapshot(&app));
    }

    #[test]
    fn long_arrays_edit_snapshot() {
        let mut app = App::new();
        app.selected = ARRAY_FIELD_INDEX;
        app.mode = Mode::Edit;
        app.fields[ARRAY_FIELD_INDEX].set_value("1.50,30,0; 2.25,20,-90; 3.75,15,90; 4.50,10,45");

        assert_snapshot("long_arrays_edit", &render_snapshot(&app));
    }

    #[test]
    fn four_panel_layout_snapshot() {
        let mut app = App::new();
        app.toggle_panel(Panel::Consumer);
        app.toggle_panel(Panel::Simulation);

        assert_snapshot("four_panel_layout", &render_snapshot(&app));
    }

    #[test]
    fn simulation_result_snapshot() {
        let mut app = App::new();
        app.toggle_panel(Panel::Simulation);
        app.simulation_result = Some(populated_simulation_result());

        assert_snapshot("simulation_result", &render_snapshot(&app));
    }

    #[test]
    fn location_search_snapshot() {
        let mut app = App::new();
        app.mode = Mode::Location;
        app.location_query.set_value("Milan");
        app.refresh_location_results();

        assert_snapshot("location_search", &render_snapshot(&app));
    }

    #[test]
    fn arrays_editor_snapshot() {
        let mut app = App::new();
        app.mode = Mode::Arrays;
        app.selected = ARRAY_FIELD_INDEX;
        app.array_selected = 1;
        app.array_column = 2;
        app.fields[ARRAY_FIELD_INDEX].set_value("1.50,30,0; 2.25,20,-90");

        assert_snapshot("arrays_editor", &render_snapshot(&app));
    }

    #[test]
    fn shape_editor_snapshot() {
        let mut app = App::new();
        app.mode = Mode::Shape;
        app.shape_selected_hour = 18;
        app.consumer_shape = ConsumerShapeState::HourlyWeights {
            weights: RESIDENTIAL_DEFAULT_WEIGHTS.to_vec(),
        };
        app.sync_consumer_shape_field();

        assert_snapshot("shape_editor", &render_snapshot(&app));
    }

    #[test]
    fn shape_preset_picker_snapshot() {
        let mut app = App::new();
        app.mode = Mode::Shape;
        app.consumer_shape = ConsumerShapeState::HourlyWeights {
            weights: RESIDENTIAL_DEFAULT_WEIGHTS.to_vec(),
        };
        app.open_shape_preset_picker();
        app.shape_preset_selected = 2;

        assert_snapshot("shape_preset_picker", &render_snapshot(&app));
    }

    #[test]
    fn arrays_editor_hit_tests_match_layout() {
        let area = Rect::new(0, 0, 80, 22);
        assert_eq!(array_cell_at(area, 8, 4, 0), Some((0, 0)));
        assert_eq!(array_cell_at(area, 19, 5, 0), Some((1, 1)));
        assert_eq!(array_cell_at(area, 30, 5, 3), Some((4, 2)));
        assert_eq!(array_cell_at(area, 1, 3, 0), None);
        assert_eq!(array_visible_start(20, 30, 10), 11);

        let footer_area = Rect::new(0, 22, 80, 2);
        assert_eq!(
            array_footer_hit(footer_area, 9, 23),
            Some(ArrayFooterAction::Done)
        );
        assert_eq!(
            array_footer_hit(footer_area, 17, 23),
            Some(ArrayFooterAction::Add)
        );
        assert_eq!(
            array_footer_hit(footer_area, 25, 23),
            Some(ArrayFooterAction::Remove)
        );
        assert_eq!(array_footer_hit(footer_area, 33, 23), None);
    }

    #[test]
    fn shape_editor_hit_tests_match_layout() {
        let area = Rect::new(0, 0, 80, 22);
        assert_eq!(shape_cell_at(area, 1, 5), Some(0));
        assert_eq!(shape_cell_at(area, 19, 5), Some(1));
        assert_eq!(shape_cell_at(area, 37, 9), Some(18));
        assert_eq!(shape_cell_at(area, 1, 4), None);
        assert_eq!(shape_cell_at(area, 79, 5), None);

        let footer_area = Rect::new(0, 22, 80, 2);
        assert_eq!(
            shape_footer_hit(footer_area, 8, 23),
            Some(ShapeFooterAction::Done)
        );
        assert_eq!(
            shape_footer_hit(footer_area, 17, 23),
            Some(ShapeFooterAction::Preset)
        );
        assert_eq!(
            shape_footer_hit(footer_area, 28, 23),
            Some(ShapeFooterAction::Custom)
        );
        assert_eq!(shape_footer_hit(footer_area, 36, 23), None);
    }

    #[test]
    fn shape_preset_picker_hit_tests_match_layout() {
        let area = Rect::new(0, 0, 80, 22);
        assert_eq!(shape_preset_option_at(area, 15, 3), Some(0));
        assert_eq!(shape_preset_option_at(area, 15, 6), Some(3));
        assert_eq!(shape_preset_option_at(area, 15, 7), None);
        assert_eq!(shape_preset_option_at(area, 1, 3), None);
    }

    #[test]
    fn arrays_to_field_value_preserves_structured_rows() {
        let arrays = vec![
            EstimateArray {
                peak_power_kwp: 1.5,
                tilt_deg: 30.0,
                azimuth_deg: 0.0,
            },
            EstimateArray {
                peak_power_kwp: 2.25,
                tilt_deg: 20.0,
                azimuth_deg: -90.0,
            },
        ];

        assert_eq!(
            arrays_to_field_value(&arrays),
            "1.50,30.0,0.0; 2.25,20.0,-90.0"
        );
    }

    #[test]
    fn location_search_hit_tests_match_layout() {
        let search_area = Rect::new(0, 0, 80, 22);
        assert_eq!(location_result_index_at(search_area, 1, 4), Some(0));
        assert_eq!(location_result_index_at(search_area, 1, 5), Some(1));
        assert_eq!(location_result_index_at(search_area, 1, 3), None);
        assert_eq!(location_result_index_at(search_area, 79, 4), None);

        let footer_area = Rect::new(0, 22, 80, 2);
        assert!(location_cancel_hit(footer_area, 10, 23));
        assert!(!location_cancel_hit(footer_area, 18, 23));
        assert!(!location_cancel_hit(footer_area, 10, 22));
    }

    #[test]
    fn monthly_table_uses_widest_left_aligned_columns() {
        let rows = vec![
            [
                "Jan".to_string(),
                "999.9".to_string(),
                "1000".to_string(),
                "1200".to_string(),
                "32.3".to_string(),
                "33.3".to_string(),
                "38.7".to_string(),
            ],
            [
                "Feb".to_string(),
                "1000.0".to_string(),
                "10000".to_string(),
                "120000".to_string(),
                "35.7".to_string(),
                "357.1".to_string(),
                "4285.7".to_string(),
            ],
        ];

        let lines = monthly_table_text_lines(&rows);

        assert_eq!(lines[0], "");
        assert_eq!(lines[2], "Month | mean   min   max    | mean min   max   ");
        assert_eq!(lines[3], "Jan   | 999.9  1000  1200   | 32.3 33.3  38.7  ");
        assert_eq!(lines[4], "Feb   | 1000.0 10000 120000 | 35.7 357.1 4285.7");
    }

    #[test]
    fn monthly_table_scrolls_month_rows_below_headers() {
        let rows = vec![
            [
                "Jan".to_string(),
                "100.0".to_string(),
                "90".to_string(),
                "110".to_string(),
                "3.2".to_string(),
                "2.9".to_string(),
                "3.5".to_string(),
            ],
            [
                "Feb".to_string(),
                "120.0".to_string(),
                "100".to_string(),
                "140".to_string(),
                "4.3".to_string(),
                "3.6".to_string(),
                "5.0".to_string(),
            ],
            [
                "Mar".to_string(),
                "140.0".to_string(),
                "120".to_string(),
                "160".to_string(),
                "4.5".to_string(),
                "3.9".to_string(),
                "5.2".to_string(),
            ],
        ];

        let lines = monthly_table_lines(&rows, 1, 5);

        assert_eq!(lines.len(), 5);
        assert!(line_text(&lines[3]).starts_with("Feb"));
        assert!(line_text(&lines[4]).starts_with("Mar"));
    }

    #[test]
    fn simulation_summary_formatting_uses_mean_and_p10_p90() {
        let kwh = MetricSummary {
            mean: 1234.4,
            p10: 1000.0,
            p50: 1200.0,
            p90: 1500.0,
        };
        let ratio = MetricSummary {
            mean: 0.26,
            p10: 0.24,
            p50: 0.25,
            p90: 0.29,
        };

        assert_eq!(format_kwh_summary(kwh), "1234 (1000..1500) kWh");
        assert_eq!(format_ratio_summary(ratio), "26% (24..29)");
    }

    #[test]
    fn monthly_table_marks_mean_columns_green_and_min_columns_red() {
        let rows = vec![
            [
                "Jan".to_string(),
                "999.9".to_string(),
                "1000".to_string(),
                "1200".to_string(),
                "32.3".to_string(),
                "33.3".to_string(),
                "38.7".to_string(),
            ],
            [
                "Feb".to_string(),
                "1000.0".to_string(),
                "10000".to_string(),
                "120000".to_string(),
                "35.7".to_string(),
                "357.1".to_string(),
                "4285.7".to_string(),
            ],
        ];

        let lines = monthly_table_lines(&rows, 0, 20);

        assert_eq!(lines[2].spans[0].style.fg, Some(Color::DarkGray));
        assert_eq!(lines[2].spans[2].style.fg, Some(Color::Green));
        assert_eq!(lines[2].spans[8].style.fg, Some(Color::Green));
        assert_eq!(lines[3].spans[0].style.fg, Some(Color::DarkGray));
        assert_eq!(lines[3].spans[2].style.fg, Some(Color::Green));
        assert_eq!(lines[3].spans[4].style.fg, Some(Color::Red));
        assert_eq!(lines[3].spans[8].style.fg, Some(Color::Green));
        assert_eq!(lines[3].spans[10].style.fg, Some(Color::Red));
        assert_eq!(lines[4].spans[2].style.fg, Some(Color::Green));
        assert_eq!(lines[4].spans[4].style.fg, None);
        assert_eq!(lines[4].spans[8].style.fg, Some(Color::Green));
        assert_eq!(lines[4].spans[10].style.fg, None);
    }

    #[test]
    fn azimuth_label_matches_pvgis_convention() {
        assert_eq!(azimuth_direction_label("0"), Some("S"));
        assert_eq!(azimuth_direction_label("-90"), Some("E"));
        assert_eq!(azimuth_direction_label("90"), Some("W"));
        assert_eq!(azimuth_direction_label("180"), Some("N"));
        assert_eq!(azimuth_direction_label("-180"), Some("N"));
        assert_eq!(azimuth_direction_label("45"), Some("SW"));
        assert_eq!(azimuth_direction_label("-45"), Some("SE"));
        assert_eq!(azimuth_direction_label("not-a-number"), None);
    }

    #[test]
    fn parses_optional_positive_storage() {
        assert_eq!(
            parse_optional_positive_f64(&Field::new("Storage kWh", "")).unwrap(),
            None
        );
        assert_eq!(
            parse_optional_positive_f64(&Field::new("Storage kWh", "5.5")).unwrap(),
            Some(5.5)
        );
        assert!(parse_optional_positive_f64(&Field::new("Storage kWh", "0")).is_err());
        assert!(parse_optional_positive_f64(&Field::new("Storage kWh", "-1")).is_err());
    }

    #[test]
    fn old_state_without_storage_keeps_default_empty_storage_field() {
        let mut app = App::new();
        let state = TuiState {
            schema_version: TUI_STATE_SCHEMA_VERSION,
            selected_location_id: "custom".to_string(),
            location_query: String::new(),
            fields: vec![
                TuiFieldState {
                    label: "Name".to_string(),
                    value: "Saved".to_string(),
                },
                TuiFieldState {
                    label: "Region".to_string(),
                    value: "IT".to_string(),
                },
                TuiFieldState {
                    label: "Latitude".to_string(),
                    value: "45.0".to_string(),
                },
                TuiFieldState {
                    label: "Longitude".to_string(),
                    value: "9.0".to_string(),
                },
                TuiFieldState {
                    label: "Loss %".to_string(),
                    value: "12.0".to_string(),
                },
                TuiFieldState {
                    label: "EUR/kWh".to_string(),
                    value: "0.22".to_string(),
                },
                TuiFieldState {
                    label: "Arrays".to_string(),
                    value: "2.0,30,0".to_string(),
                },
            ],
            consumer_fields: Vec::new(),
            consumer_shape: ConsumerShapeState::default(),
            simulation_fields: Vec::new(),
            panel_visibility: PanelVisibility::default(),
            focused_panel: Panel::System,
        };

        for field in &mut app.fields {
            if let Some(saved) = state.fields.iter().find(|saved| saved.label == field.label) {
                field.set_value(&saved.value);
            }
        }

        assert_eq!(app.fields[STORAGE_FIELD_INDEX].value, "");
        assert_eq!(app.fields[ARRAY_FIELD_INDEX].value, "2.0,30,0");
    }

    #[test]
    fn parses_optional_energy_price() {
        assert_eq!(
            parse_optional_f64(&Field::new("EUR/kWh", "")).unwrap(),
            None
        );
        assert_eq!(
            parse_optional_f64(&Field::new("EUR/kWh", "0.22")).unwrap(),
            Some(0.22)
        );
        assert!(parse_optional_f64(&Field::new("EUR/kWh", "abc")).is_err());
    }

    #[test]
    fn parses_multiple_array_entries() {
        let field = Field::new("Arrays", "1.5,30,0; 2.25,20,-90");
        let arrays = parse_arrays(&field).expect("valid arrays");

        assert_eq!(arrays.len(), 2);
        assert_eq!(arrays[0].peak_power_kwp, 1.5);
        assert_eq!(arrays[0].tilt_deg, 30.0);
        assert_eq!(arrays[0].azimuth_deg, 0.0);
        assert_eq!(arrays[1].peak_power_kwp, 2.25);
        assert_eq!(arrays[1].tilt_deg, 20.0);
        assert_eq!(arrays[1].azimuth_deg, -90.0);
    }

    #[test]
    fn rejects_malformed_array_entries() {
        let field = Field::new("Arrays", "1.5,30; 2.0,20,0");
        let error = parse_arrays(&field).expect_err("entry is missing azimuth");

        assert!(
            error
                .to_string()
                .contains("array 1 must be kWp,tilt,azimuth")
        );
    }

    #[test]
    fn totals_array_kwp() {
        let arrays =
            parse_arrays(&Field::new("Arrays", "1.5,30,0; 2.25,20,-90")).expect("valid arrays");

        assert_eq!(total_array_kwp(&arrays), 3.75);
    }

    #[test]
    fn field_value_view_tracks_cursor_in_long_values() {
        let mut field = Field::new("Arrays", "1,2,3; 4,5,6; 7,8,9");
        field.cursor = 0;

        let start = field_value_view(&field, 10, true);
        assert_eq!(start.value.chars().count(), 10);
        assert!(start.value.ends_with('>'));
        assert_eq!(start.cursor_col, 0);

        field.cursor = field.value.len();
        let end = field_value_view(&field, 10, true);
        assert_eq!(end.value.chars().count(), 10);
        assert!(end.value.starts_with('<'));
        assert_eq!(end.cursor_col, 9);
    }

    #[test]
    fn field_editing_tracks_cursor_and_text() {
        let mut field = Field::new("Name", "Milan");
        field.move_left();
        field.move_left();
        field.insert('X');
        assert_eq!(field.value, "MilXan");
        assert_eq!(field.cursor, 4);

        field.backspace();
        assert_eq!(field.value, "Milan");
        assert_eq!(field.cursor, 3);

        field.delete();
        assert_eq!(field.value, "Miln");
        assert_eq!(field.cursor, 3);

        field.move_right();
        field.move_right();
        assert_eq!(field.cursor, field.value.len());
    }

    #[test]
    fn location_search_refreshes_results() {
        let mut app = App::new();
        app.location_query.set_value("Milan");
        app.refresh_location_results();

        let first = app.location_results.first().expect("Milan search result");
        assert_eq!(first.display_name, "Milan");
        assert_eq!(first.country_code, "IT");
        assert_eq!(app.location_selected, 0);
    }

    #[test]
    fn applying_selected_city_updates_location_fields() {
        let mut app = App::new();
        app.location_query.set_value("Milan");
        app.refresh_location_results();
        let milan = app
            .location_results
            .iter()
            .find(|result| result.display_name == "Milan" && result.country_code == "IT")
            .cloned()
            .expect("Milan search result");

        app.apply_location_fields(&milan);

        assert_eq!(app.fields[0].value, "Milan");
        assert_eq!(app.fields[1].value, "IT");
        assert_eq!(
            app.fields[2].value,
            format!("{:.4}", milan.latitude_degrees)
        );
        assert_eq!(
            app.fields[3].value,
            format!("{:.4}", milan.longitude_degrees)
        );
        assert_eq!(
            app.selected_location_id,
            format!("geonames-{}", milan.geoname_id)
        );
    }
}
