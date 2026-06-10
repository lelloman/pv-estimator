use std::fs;
use std::io;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

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
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
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
const ARRAY_FORMAT_HELP: &str = "format: kWp,tilt,az; repeat with ;";
const FIELD_LABEL_WIDTH: u16 = 13;
const ESTIMATE_LABEL_WIDTH: usize = 8;
const SEARCH_LABEL_WIDTH: u16 = 8;
const LOCATION_RESULT_HEADER_ROWS: u16 = 3;
const MONTHLY_TABLE_HEADERS: [&str; 7] = ["Month", "mean", "min", "max", "mean", "min", "max"];

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TuiState {
    schema_version: u32,
    selected_location_id: String,
    location_query: String,
    fields: Vec<TuiFieldState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TuiFieldState {
    label: String,
    value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    Edit,
    Location,
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
    selected: usize,
    mode: Mode,
    status: String,
    estimate: Option<SourceEnsembleEstimateDocument>,
    selected_location_id: String,
    location_query: Field,
    location_results: Vec<CitySearchResult>,
    location_selected: usize,
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
                Field::new("Arrays", "1.0,30.0,0.0"),
            ],
            selected: 2,
            mode: Mode::Normal,
            status: "Ready".to_string(),
            estimate: None,
            selected_location_id: "custom".to_string(),
            location_query: Field::new("Find", ""),
            location_results: Vec::new(),
            location_selected: 0,
        }
    }

    fn recompute(&mut self, estimator: &mut SourceModelEstimator) {
        match self
            .request_and_arrays()
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
        self.selected_location_id = state.selected_location_id;
        self.location_query.set_value(&state.location_query);
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

    fn request_and_arrays(&self) -> Result<(EstimateRequest, Vec<EstimateArray>)> {
        let arrays = parse_arrays(&self.fields[5])?;
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
            },
            arrays,
        ))
    }

    fn selected_field_mut(&mut self) -> &mut Field {
        &mut self.fields[self.selected]
    }

    fn mark_custom_location_if_needed(&mut self) {
        if self.selected <= 3 {
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

fn parse_f64(field: &Field) -> Result<f64> {
    field
        .value
        .parse::<f64>()
        .with_context(|| format!("{} must be a number", field.label))
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
    }
}

fn handle_normal_key(
    key: KeyEvent,
    app: &mut App,
    estimator: &mut SourceModelEstimator,
) -> Result<bool> {
    match key.code {
        KeyCode::Char('q') => return Ok(true),
        KeyCode::Up => app.selected = app.selected.saturating_sub(1),
        KeyCode::Down | KeyCode::Tab => app.selected = (app.selected + 1).min(app.fields.len() - 1),
        KeyCode::BackTab => app.selected = app.selected.saturating_sub(1),
        KeyCode::Home => app.selected = 0,
        KeyCode::End => app.selected = app.fields.len() - 1,
        KeyCode::Enter if app.fields[app.selected].label == "Name" => app.open_location_search(),
        KeyCode::Enter => app.mode = Mode::Edit,
        KeyCode::Char('l') => app.open_location_search(),
        KeyCode::Char('e') => app.recompute(estimator),
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
        KeyCode::Enter => {
            app.mode = Mode::Normal;
            app.recompute(estimator);
        }
        KeyCode::Tab => {
            app.mode = Mode::Normal;
            app.recompute(estimator);
            app.selected = (app.selected + 1).min(app.fields.len() - 1);
        }
        KeyCode::BackTab => {
            app.mode = Mode::Normal;
            app.recompute(estimator);
            app.selected = app.selected.saturating_sub(1);
        }
        KeyCode::Backspace => {
            app.selected_field_mut().backspace();
            app.mark_custom_location_if_needed();
        }
        KeyCode::Delete => {
            app.selected_field_mut().delete();
            app.mark_custom_location_if_needed();
        }
        KeyCode::Left => app.selected_field_mut().move_left(),
        KeyCode::Right => app.selected_field_mut().move_right(),
        KeyCode::Home => app.selected_field_mut().cursor = 0,
        KeyCode::End => {
            let field = app.selected_field_mut();
            field.cursor = field.value.len();
        }
        KeyCode::Char(character)
            if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
        {
            app.selected_field_mut().insert(character);
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
    if mouse.kind != MouseEventKind::Down(event::MouseButton::Left) {
        return Ok(());
    }
    let (width, height) = size()?;
    let area = Rect::new(0, 0, width, height);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(10), Constraint::Length(2)])
        .split(area);

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

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(45), Constraint::Min(40)])
        .split(vertical[0]);
    let fields_inner = Block::default().borders(Borders::ALL).inner(body[0]);
    if mouse.column >= fields_inner.x
        && mouse.column < fields_inner.x.saturating_add(fields_inner.width)
        && mouse.row == fields_inner.y
    {
        app.selected = 0;
        app.open_location_search();
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

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(45), Constraint::Min(40)])
        .split(vertical[0]);
    render_fields(frame, body[0], app);
    render_estimate(frame, body[1], app);
    render_footer(frame, vertical[1], app);
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

fn annual_band_lines(app: &App) -> [Line<'static>; 2] {
    let (annual, band) = app
        .estimate
        .as_ref()
        .map(|document| {
            let estimate = &document.ensemble_estimate;
            let annual = format!("{:.2} kWh", estimate.annual_energy.mean.as_kilowatt_hours());
            let band = estimate
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
            (annual, band)
        })
        .unwrap_or_else(|| ("-".to_string(), "-".to_string()));

    [
        estimate_metric_line(
            "Annual",
            annual,
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ),
        estimate_metric_line("Band", band, Style::default()),
    ]
}

fn render_fields(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title("System");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let array_extra_lines = array_extra_line_count(app);
    let mut lines = Vec::with_capacity(
        app.fields.len() + array_extra_lines as usize + app.location_results.len().min(6) + 3,
    );
    for (index, field) in app.fields.iter().enumerate() {
        let selected = index == app.selected;
        let style = match (selected, app.mode) {
            (true, Mode::Edit) => Style::default().fg(Color::Black).bg(Color::Yellow),
            (true, Mode::Normal) => Style::default().fg(Color::Black).bg(Color::Cyan),
            _ => Style::default(),
        };
        let value_view = field_value_view(field, field_value_width(inner), selected);
        let spans = vec![
            Span::styled(
                format!(
                    "{:<width$}",
                    field.label,
                    width = FIELD_LABEL_WIDTH as usize
                ),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(value_view.value, style),
        ];
        lines.push(Line::from(spans));
        if field.label == "Arrays" {
            lines.push(Line::from(vec![Span::styled(
                format!("  {ARRAY_FORMAT_HELP}"),
                Style::default().fg(Color::DarkGray),
            )]));
        }
        if field.label == "Arrays"
            && let Ok(arrays) = parse_arrays(field)
        {
            lines.push(Line::from(vec![Span::styled(
                format!("  total {:.2} kWp", total_array_kwp(&arrays)),
                Style::default().fg(Color::DarkGray),
            )]));
            for (array_index, array) in arrays.iter().enumerate() {
                let direction = azimuth_direction_label(&array.azimuth_deg.to_string())
                    .map(|label| format!(" {label}"))
                    .unwrap_or_default();
                lines.push(Line::from(vec![Span::styled(
                    format!(
                        "  A{} {:>5.2} kWp  tilt {:>4.1}  az {:>5.1}{}",
                        array_index + 1,
                        array.peak_power_kwp,
                        array.tilt_deg,
                        array.azimuth_deg,
                        direction
                    ),
                    Style::default().fg(Color::DarkGray),
                )]));
            }
        }
    }
    frame.render_widget(Paragraph::new(lines), inner);

    if app.mode == Mode::Edit {
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

fn render_estimate(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title("Estimate");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(document) = &app.estimate else {
        let [annual, band] = annual_band_lines(app);
        frame.render_widget(
            Paragraph::new(vec![annual, band, Line::from("No estimate")]),
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
    let [annual, band] = annual_band_lines(app);
    let header = Paragraph::new(vec![
        annual,
        band,
        estimate_metric_line(
            "POA",
            format!(
                "{:.2} kWh/m2",
                estimate
                    .annual_in_plane_irradiation
                    .mean
                    .as_kilowatt_hours_per_square_meter()
            ),
            Style::default(),
        ),
        estimate_metric_line("Sources", sources, Style::default()),
    ]);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(4), Constraint::Min(5)])
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

    frame.render_widget(Paragraph::new(monthly_table_lines(&rows)), chunks[1]);
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

fn monthly_table_lines(rows: &[[String; 7]]) -> Vec<Line<'static>> {
    let column_widths = monthly_table_column_widths(rows);
    let monthly_width = column_widths[1] + column_widths[2] + column_widths[3] + 2;
    let daily_width = column_widths[4] + column_widths[5] + column_widths[6] + 2;
    let header_style = Style::default().fg(Color::DarkGray);

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
    for row in rows {
        lines.push(monthly_table_line(row, column_widths, false, minimums));
    }
    lines
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

fn render_footer(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let (mode, help) = match app.mode {
        Mode::Normal => (
            "NORMAL",
            "arrows/tab select  enter edit  l locations  e estimate  q quit",
        ),
        Mode::Edit if app.fields[app.selected].label == "Arrays" => {
            ("EDIT", "enter/tab apply estimate  esc close edit")
        }
        Mode::Edit => ("EDIT", "type value  enter apply  esc cancel edit  tab next"),
        Mode::Location => (
            "LOCATION",
            "type filter  arrows select  enter apply  esc cancel",
        ),
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

    use ratatui::backend::TestBackend;

    const SNAPSHOT_SIZE: (u16, u16) = (80, 24);

    fn render_snapshot(app: &App) -> String {
        let backend = TestBackend::new(SNAPSHOT_SIZE.0, SNAPSHOT_SIZE.1);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        terminal.draw(|frame| render(frame, app)).expect("draw TUI");
        terminal.backend().to_string()
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
    fn default_layout_snapshot() {
        let app = App::new();

        assert_snapshot("default_layout", &render_snapshot(&app));
    }

    #[test]
    fn long_arrays_edit_snapshot() {
        let mut app = App::new();
        app.selected = 5;
        app.mode = Mode::Edit;
        app.fields[5].set_value("1.50,30,0; 2.25,20,-90; 3.75,15,90; 4.50,10,45");

        assert_snapshot("long_arrays_edit", &render_snapshot(&app));
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

        let lines = monthly_table_lines(&rows);

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
