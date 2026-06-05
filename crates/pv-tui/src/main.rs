use std::fs;
use std::io;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use directories::ProjectDirs;
use pv_core::source_model::SourceEnsembleEstimateDocument;
use pv_data::CitySearchResult;
use pv_model::{EstimateRequest, SourceModelEstimator, days_in_month, short_month_name};
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
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
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
    execute!(io::stdout(), EnterAlternateScreen)?;
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
                Field::new("kWp", "1.0"),
                Field::new("Loss %", "14.0"),
                Field::new("Tilt deg", "30.0"),
                Field::new("Azimuth deg", "0.0"),
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
            .request()
            .and_then(|request| estimator.estimate(&request))
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

    fn request(&self) -> Result<EstimateRequest> {
        Ok(EstimateRequest {
            location_id: self.selected_location_id.clone(),
            name: self.fields[0].value.clone(),
            region: self.fields[1].value.clone(),
            latitude: parse_f64(&self.fields[2])?,
            longitude: parse_f64(&self.fields[3])?,
            peak_power_kwp: parse_f64(&self.fields[4])?,
            loss_pct: parse_f64(&self.fields[5])?,
            tilt_deg: parse_f64(&self.fields[6])?,
            azimuth_deg: parse_f64(&self.fields[7])?,
        })
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
        self.fields[0].set_value(&location.display_name);
        self.fields[1].set_value(&location.country_code);
        self.fields[2].set_value(&format!("{:.4}", location.latitude_degrees));
        self.fields[3].set_value(&format!("{:.4}", location.longitude_degrees));
        self.selected_location_id = format!("geonames-{}", location.geoname_id);
        self.status = format!(
            "Selected {}, {}",
            location.display_name, location.country_code
        );
        self.mode = Mode::Normal;
        self.recompute(estimator);
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
        KeyCode::Enter => app.mode = Mode::Edit,
        KeyCode::Char('l') => {
            app.mode = Mode::Location;
            app.location_selected = 0;
            app.refresh_location_results();
            app.status = "Select location by name".to_string();
        }
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
            app.selected = (app.selected + 1).min(app.fields.len() - 1);
        }
        KeyCode::BackTab => {
            app.mode = Mode::Normal;
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

fn handle_location_key(
    key: KeyEvent,
    app: &mut App,
    estimator: &mut SourceModelEstimator,
) -> Result<bool> {
    match key.code {
        KeyCode::Esc => app.mode = Mode::Normal,
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
        .constraints([
            Constraint::Length(2),
            Constraint::Min(10),
            Constraint::Length(1),
        ])
        .split(area);

    render_summary(frame, vertical[0], app);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(35), Constraint::Min(40)])
        .split(vertical[1]);
    render_fields(frame, body[0], app);
    render_estimate(frame, body[1], app);
    render_footer(frame, vertical[2], app);
}

fn render_summary(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
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

    let line = Line::from(vec![
        Span::styled("Annual ", Style::default().fg(Color::DarkGray)),
        Span::styled(annual, Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("   "),
        Span::styled("Band ", Style::default().fg(Color::DarkGray)),
        Span::raw(band),
    ]);
    let status = Line::from(vec![
        Span::styled("Status ", Style::default().fg(Color::DarkGray)),
        Span::raw(app.status.as_str()),
    ]);
    frame.render_widget(Paragraph::new(vec![line, status]), area);
}

fn render_fields(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title("System");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = Vec::with_capacity(app.fields.len() + app.location_results.len().min(6) + 3);
    for (index, field) in app.fields.iter().enumerate() {
        let selected = index == app.selected;
        let style = match (selected, app.mode) {
            (true, Mode::Edit) => Style::default().fg(Color::Black).bg(Color::Yellow),
            (true, Mode::Normal) => Style::default().fg(Color::Black).bg(Color::Cyan),
            _ => Style::default(),
        };
        let mut spans = vec![
            Span::styled(
                format!("{:<13}", field.label),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(field.value.as_str(), style),
        ];
        if field.label == "Azimuth deg"
            && let Some(label) = azimuth_direction_label(field.value.as_str())
        {
            spans.push(Span::styled(
                format!(" ({label})"),
                Style::default().fg(Color::DarkGray),
            ));
        }
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(""));
    let search_style = if app.mode == Mode::Location {
        Style::default().fg(Color::Black).bg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    lines.push(Line::from(vec![
        Span::styled("Location     ", Style::default().fg(Color::DarkGray)),
        Span::styled(app.location_query.value.as_str(), search_style),
    ]));
    for (row, location) in app.location_results.iter().take(6).enumerate() {
        let selected = app.mode == Mode::Location && row == app.location_selected;
        let style = if selected {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![Span::styled(city_label(location), style)]));
    }
    if app.location_results.is_empty() {
        let message = if app.location_query.value.is_empty() {
            "Type at least 2 characters"
        } else {
            "No matching locations"
        };
        lines.push(Line::from(Span::styled(
            message,
            Style::default().fg(Color::DarkGray),
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);

    if app.mode == Mode::Edit {
        let field = &app.fields[app.selected];
        let y = inner.y.saturating_add(app.selected as u16);
        let x = inner
            .x
            .saturating_add(13)
            .saturating_add(field.cursor.min(u16::MAX as usize) as u16);
        frame.set_cursor_position(Position::new(x, y));
    } else if app.mode == Mode::Location {
        let y = inner.y.saturating_add(app.fields.len() as u16 + 1);
        let x = inner
            .x
            .saturating_add(13)
            .saturating_add(app.location_query.cursor.min(u16::MAX as usize) as u16);
        frame.set_cursor_position(Position::new(x, y));
    }
}

fn render_estimate(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let block = Block::default().borders(Borders::ALL).title("Estimate");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(document) = &app.estimate else {
        frame.render_widget(Paragraph::new("No estimate"), inner);
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
    let header = Paragraph::new(vec![
        Line::from(format!(
            "POA {:.2} kWh/m2",
            estimate
                .annual_in_plane_irradiation
                .mean
                .as_kilowatt_hours_per_square_meter()
        )),
        Line::from(format!("Sources {sources}")),
    ]);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(5)])
        .split(inner);
    frame.render_widget(header.wrap(Wrap { trim: true }), chunks[0]);

    let mut lines = vec![
        Line::from(Span::styled(
            format!("{:<5} | {:^16} | {:^14}", "", "Monthly kWh", "Daily kWh"),
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            format!(
                "{:<5} | {:>5} {:>4} {:>4} | {:>4} {:>4} {:>4}",
                "Month", "mean", "min", "max", "mean", "min", "max"
            ),
            Style::default().fg(Color::DarkGray),
        )),
    ];
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
        lines.push(Line::from(format!(
            "{:<5} | {:>5.1} {:>4} {:>4} | {:>4.1} {:>4} {:>4}",
            month_name,
            total_kwh,
            total_min,
            total_max,
            total_kwh / days,
            daily_min,
            daily_max
        )));
    }
    frame.render_widget(Paragraph::new(lines), chunks[1]);
}

fn render_footer(frame: &mut ratatui::Frame<'_>, area: Rect, app: &App) {
    let (mode, help) = match app.mode {
        Mode::Normal => (
            "NORMAL",
            "arrows/tab select  enter edit  l locations  e estimate  q quit",
        ),
        Mode::Edit => ("EDIT", "type value  enter apply  esc cancel edit  tab next"),
        Mode::Location => (
            "LOCATION",
            "type filter  arrows select  enter apply  esc close",
        ),
    };
    let line = Line::from(vec![
        Span::styled(mode, Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::raw(help),
    ]);
    frame.render_widget(Paragraph::new(line), area);
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
}
