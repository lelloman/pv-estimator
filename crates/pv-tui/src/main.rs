use std::io;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use pv_core::source_model::SourceEnsembleEstimateDocument;
use pv_core::weather::Location;
use pv_model::{EstimateRequest, SourceModelEstimator, days_in_month};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap};

#[derive(Debug, Parser)]
#[command(name = "pv-tui")]
#[command(about = "Interactive PV estimator terminal UI")]
struct Args {
    #[arg(long)]
    model_dir: PathBuf,
    #[arg(long, default_value = "source-model-artifacts.json")]
    manifest: String,
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
    locations: Vec<Location>,
    location_query: Field,
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
    let mut estimator = SourceModelEstimator::load(&args.model_dir, &args.manifest)
        .with_context(|| format!("loading model artifacts from {}", args.model_dir.display()))?;

    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)?;
    let _guard = TerminalGuard;

    let backend = CrosstermBackend::new(io::stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let mut app = App::new(pv_data::locations());
    app.recompute(&mut estimator);
    run_app(&mut terminal, &mut app, &mut estimator)?;
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
    fn new(locations: Vec<Location>) -> Self {
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
            locations,
            location_query: Field::new("Find", ""),
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
            }
            Err(error) => {
                self.status = format!("{error:#}");
            }
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

    fn filtered_location_indexes(&self) -> Vec<usize> {
        let query = self.location_query.value.to_lowercase();
        self.locations
            .iter()
            .enumerate()
            .filter_map(|(index, location)| {
                if query.is_empty() || location_matches(location, &query) {
                    Some(index)
                } else {
                    None
                }
            })
            .collect()
    }

    fn clamp_location_selection(&mut self) {
        let matches = self.filtered_location_indexes();
        if matches.is_empty() {
            self.location_selected = 0;
        } else {
            self.location_selected = self.location_selected.min(matches.len() - 1);
        }
    }

    fn apply_selected_location(&mut self, estimator: &mut SourceModelEstimator) {
        let matches = self.filtered_location_indexes();
        let Some(location_index) = matches.get(self.location_selected).copied() else {
            self.status = "No matching location".to_string();
            return;
        };
        let location = self.locations[location_index].clone();
        self.fields[0].set_value(&location.display_name);
        self.fields[1].set_value(location.region.as_deref().unwrap_or_default());
        self.fields[2].set_value(&format!("{:.4}", location.latitude.as_degrees()));
        self.fields[3].set_value(&format!("{:.4}", location.longitude.as_degrees()));
        self.selected_location_id = location.location_id.as_str().to_string();
        self.status = format!("Selected {}", location.display_name);
        self.mode = Mode::Normal;
        self.recompute(estimator);
    }
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
            let matches = app.filtered_location_indexes();
            if !matches.is_empty() {
                app.location_selected = (app.location_selected + 1).min(matches.len() - 1);
            }
        }
        KeyCode::Backspace => {
            app.location_query.backspace();
            app.location_selected = 0;
        }
        KeyCode::Delete => {
            app.location_query.delete();
            app.location_selected = 0;
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

    let matches = app.filtered_location_indexes();
    let mut lines = Vec::with_capacity(app.fields.len() + matches.len().min(6) + 3);
    for (index, field) in app.fields.iter().enumerate() {
        let selected = index == app.selected;
        let style = match (selected, app.mode) {
            (true, Mode::Edit) => Style::default().fg(Color::Black).bg(Color::Yellow),
            (true, Mode::Normal) => Style::default().fg(Color::Black).bg(Color::Cyan),
            _ => Style::default(),
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{:<11}", field.label),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(field.value.as_str(), style),
        ]));
    }
    lines.push(Line::from(""));
    let search_style = if app.mode == Mode::Location {
        Style::default().fg(Color::Black).bg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    lines.push(Line::from(vec![
        Span::styled("Location   ", Style::default().fg(Color::DarkGray)),
        Span::styled(app.location_query.value.as_str(), search_style),
    ]));
    for (row, location_index) in matches.iter().take(6).enumerate() {
        let location = &app.locations[*location_index];
        let selected = app.mode == Mode::Location && row == app.location_selected;
        let style = if selected {
            Style::default().fg(Color::Black).bg(Color::Cyan)
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![Span::styled(
            location_label(location),
            style,
        )]));
    }
    if matches.is_empty() {
        lines.push(Line::from(Span::styled(
            "No matching locations",
            Style::default().fg(Color::DarkGray),
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);

    if app.mode == Mode::Edit {
        let field = &app.fields[app.selected];
        let y = inner.y.saturating_add(app.selected as u16);
        let x = inner
            .x
            .saturating_add(11)
            .saturating_add(field.cursor.min(u16::MAX as usize) as u16);
        frame.set_cursor_position(Position::new(x, y));
    } else if app.mode == Mode::Location {
        let y = inner.y.saturating_add(app.fields.len() as u16 + 1);
        let x = inner
            .x
            .saturating_add(11)
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

    let rows = estimate.monthly_estimates.iter().map(|monthly| {
        let month = monthly.month.value();
        let days = days_in_month(month).expect("valid month has a day count");
        let total_kwh = monthly.energy.mean.as_kilowatt_hours();
        let total_band = monthly
            .uncertainty
            .annual_energy
            .map(|band| {
                format!(
                    "{:.0}-{:.0}",
                    band.low.as_kilowatt_hours(),
                    band.high.as_kilowatt_hours()
                )
            })
            .unwrap_or_else(|| "-".to_string());
        let daily_band = monthly
            .uncertainty
            .annual_energy
            .map(|band| {
                format!(
                    "{:.1}-{:.1}",
                    band.low.as_kilowatt_hours() / days,
                    band.high.as_kilowatt_hours() / days
                )
            })
            .unwrap_or_else(|| "-".to_string());
        Row::new(vec![
            Cell::from(month.to_string()),
            Cell::from(format!("{:.1}", total_kwh)),
            Cell::from(total_band),
            Cell::from(format!("{:.1}", total_kwh / days)),
            Cell::from(daily_band),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(3),
            Constraint::Length(7),
            Constraint::Length(10),
            Constraint::Length(7),
            Constraint::Min(9),
        ],
    )
    .header(
        Row::new(vec!["M", "kWh", "band", "kWh/d", "d band"])
            .style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(table, chunks[1]);
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

fn location_matches(location: &Location, query: &str) -> bool {
    location.display_name.to_lowercase().contains(query)
        || location.country_code.to_lowercase().contains(query)
        || location
            .region
            .as_deref()
            .unwrap_or_default()
            .to_lowercase()
            .contains(query)
        || location
            .province
            .as_deref()
            .unwrap_or_default()
            .to_lowercase()
            .contains(query)
}

fn location_label(location: &Location) -> String {
    format!(
        "  {:<10} {:>6.2} {:>7.2}",
        truncate(&location.display_name, 10),
        location.latitude.as_degrees(),
        location.longitude.as_degrees(),
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
