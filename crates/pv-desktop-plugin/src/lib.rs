#![allow(deprecated)]

use std::cell::RefCell;
use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use directories::ProjectDirs;
use gtk::glib::translate::IntoGlibPtr;
use gtk::prelude::*;
use gtk::{
    Align, Box as GtkBox, Button, CheckButton, DrawingArea, DropDown, Entry, FileChooserAction,
    FileChooserDialog, FileFilter, Grid, Image, Label, ListBox, Orientation, PolicyType, Popover,
    ProgressBar, ResponseType, ScrolledWindow, SelectionMode, Separator, Switch, Window,
};
use maruzzella_sdk::{
    CommandSpec, HostApi, MzHostEvent, MzStatusCode, MzSurfaceFocusEvent, MzViewPlacement, Plugin,
    PluginDependency, PluginDescriptor, SurfaceContributionSpec, Version, ViewFactorySpec,
    button_css_class, decode_json_payload, export_plugin, input_css_class, mark_clickable,
    text_css_class,
};
use pv_core::simulation::{
    BuiltInLoadShapeId, LoadProfile, LoadShape, MetricSummary, ProductionProfile,
    SimulationRequest, SimulationResult, SimulationRunMetrics, StorageConfig,
    deterministic_hourly_load_kwh, simulate_with_progress,
};
use pv_core::source_model::SourceEnsembleEstimateDocument;
use pv_data::{CitySearchResult, search_cities};
use pv_desktop_core::{
    PROJECT_EXTENSION, PvProjectDocument, SimulationRunMetadata, load_project, save_project,
};
use pv_model::{
    EstimateArray, EstimateRequest, SourceModelEstimator, days_in_month, short_month_name,
};
use serde::{Deserialize, Serialize};

pub struct PvDesktopPlugin;

const PLUGIN_ID: &str = "com.lelloman.pv_estimator.desktop";
const VIEW_LAUNCHER: &str = "com.lelloman.pv_estimator.launcher";
const VIEW_SYSTEM: &str = "com.lelloman.pv_estimator.system";
const VIEW_ESTIMATE: &str = "com.lelloman.pv_estimator.estimate";
const VIEW_SIMULATION: &str = "com.lelloman.pv_estimator.simulation";
const VIEW_DETAILS: &str = "com.lelloman.pv_estimator.details";
const VIEW_SETTINGS: &str = "com.lelloman.pv_estimator.settings";
const DESKTOP_SESSION_SCHEMA_VERSION: u32 = 1;

const CMD_NEW: &str = "pv.project.new";
const CMD_OPEN: &str = "pv.project.open";
const CMD_CLOSE: &str = "pv.project.close";
const CMD_SAVE: &str = "pv.project.save";
const CMD_SAVE_AS: &str = "pv.project.save_as";
const CMD_SET_SIMULATION_RUNS: &str = "pv.project.set_simulation_runs";
const CMD_EXIT: &str = "pv.app.exit";
const SAVE_ACTION_IDS: &[&str] = &["pv-project-save", "file-save", "save"];
const DETAILS_PANEL_MIN_WIDTH: i32 = 320;
const COMPUTATION_DEBOUNCE: Duration = Duration::from_millis(750);
const AUTO_SAVE_DEBOUNCE: Duration = Duration::from_millis(900);
const SIMULATION_RUN_OPTIONS: [usize; 7] = [
    1_000,
    10_000,
    100_000,
    1_000_000,
    10_000_000,
    100_000_000,
    1_000_000_000,
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorPalettePreference {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ApplicationSettings {
    #[serde(default = "default_true")]
    automatic_simulation_updates: bool,
    #[serde(default)]
    color_palette: ColorPalettePreference,
    #[serde(default)]
    automatic_project_save: bool,
}

impl Default for ApplicationSettings {
    fn default() -> Self {
        Self {
            automatic_simulation_updates: true,
            color_palette: ColorPalettePreference::System,
            automatic_project_save: false,
        }
    }
}

const fn default_true() -> bool {
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComputationImpact {
    Simulation,
    EstimateAndSimulation,
}

impl ComputationImpact {
    fn merge(self, other: Self) -> Self {
        if self == Self::EstimateAndSimulation || other == Self::EstimateAndSimulation {
            Self::EstimateAndSimulation
        } else {
            Self::Simulation
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComputationTrigger {
    Debounced,
    Immediate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComputationStage {
    Estimate,
    Simulation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ComputationPhase {
    Idle,
    Debouncing,
    Estimating,
    WaitingForSimulation,
}

#[derive(Clone, Debug)]
struct DesktopState {
    project: Option<PvProjectDocument>,
    path: Option<PathBuf>,
    dirty: bool,
    status: String,
    log: Vec<String>,
    session_loaded: bool,
    simulation_generation: u64,
    computation_phase: ComputationPhase,
    retry_stage: Option<ComputationStage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DesktopSession {
    schema_version: u32,
    last_project_path: Option<PathBuf>,
    #[serde(default)]
    settings: ApplicationSettings,
}

impl Default for DesktopState {
    fn default() -> Self {
        Self {
            project: None,
            path: None,
            dirty: false,
            status: "No project open".to_string(),
            log: vec!["No project open".to_string()],
            session_loaded: false,
            simulation_generation: 0,
            computation_phase: ComputationPhase::Idle,
            retry_stage: None,
        }
    }
}

#[derive(Default)]
struct AutomaticComputationState {
    generation: u64,
    pending: Option<ComputationImpact>,
    debounce: Option<gtk::glib::SourceId>,
}

struct ShellModeHandlers {
    show_workspace: Box<dyn Fn()>,
    show_launcher: Box<dyn Fn()>,
}

struct AppearanceHandlers {
    apply_color_palette: Box<dyn Fn(ColorPalettePreference)>,
}

struct SimulationRunState {
    requested_runs: usize,
    completed_runs: usize,
    cancel: Arc<AtomicBool>,
    cancelling: bool,
    receiver: mpsc::Receiver<SimulationRunMessage>,
    generation: u64,
    started_at: Instant,
    started_wall_time: SystemTime,
}

#[derive(Debug)]
enum SimulationRunMessage {
    Progress(usize),
    Finished(Box<Result<SimulationResult, String>>),
}

#[derive(Debug, Clone, Copy)]
struct SimulationRunSnapshot {
    requested_runs: usize,
    completed_runs: usize,
    cancelling: bool,
    started_at: Instant,
    started_wall_time: SystemTime,
}

#[derive(Clone, Copy)]
struct DailyProjectionDate {
    month: u8,
    day: u8,
}

struct SimulationProgressWidgets {
    heading: gtk::glib::WeakRef<Label>,
    count: gtk::glib::WeakRef<Label>,
    progress: gtk::glib::WeakRef<ProgressBar>,
    status: gtk::glib::WeakRef<Label>,
    cancel: gtk::glib::WeakRef<Button>,
    cancel_label: gtk::glib::WeakRef<Label>,
}

thread_local! {
    static STATE: RefCell<DesktopState> = RefCell::new(DesktopState::default());
    static ESTIMATOR: RefCell<Option<SourceModelEstimator>> = const { RefCell::new(None) };
    static SHELL_MODE_HANDLERS: RefCell<Option<ShellModeHandlers>> = const { RefCell::new(None) };
    static ACTIVE_FILE_CHOOSER: RefCell<Option<FileChooserDialog>> = const { RefCell::new(None) };
    static SIMULATION_RUN: RefCell<Option<SimulationRunState>> = const { RefCell::new(None) };
    static AUTOMATIC_COMPUTATION: RefCell<AutomaticComputationState> = RefCell::new(AutomaticComputationState::default());
    static APPLICATION_SETTINGS: RefCell<Option<ApplicationSettings>> = const { RefCell::new(None) };
    static APPEARANCE_HANDLERS: RefCell<Option<AppearanceHandlers>> = const { RefCell::new(None) };
    static AUTOMATIC_SAVE: RefCell<Option<gtk::glib::SourceId>> = const { RefCell::new(None) };
    static SIMULATION_GRAPH_DATE: RefCell<DailyProjectionDate> = const { RefCell::new(DailyProjectionDate { month: 6, day: 21 }) };
    static SIMULATION_PROGRESS_VIEWS: RefCell<Vec<SimulationProgressWidgets>> = const { RefCell::new(Vec::new()) };
    static SYSTEM_VIEWS: RefCell<Vec<gtk::glib::WeakRef<GtkBox>>> = const { RefCell::new(Vec::new()) };
    static ESTIMATE_VIEWS: RefCell<Vec<gtk::glib::WeakRef<GtkBox>>> = const { RefCell::new(Vec::new()) };
    static SIMULATION_VIEWS: RefCell<Vec<gtk::glib::WeakRef<GtkBox>>> = const { RefCell::new(Vec::new()) };
    static DETAIL_VIEWS: RefCell<Vec<gtk::glib::WeakRef<GtkBox>>> = const { RefCell::new(Vec::new()) };
    static ACTIVE_CONTEXT: RefCell<Option<MzSurfaceFocusEvent>> = const { RefCell::new(None) };
    static INITIAL_DETAILS_VIEW: RefCell<Option<&'static str>> = const { RefCell::new(None) };
}

pub fn install_shell_mode_handlers(
    show_workspace: impl Fn() + 'static,
    show_launcher: impl Fn() + 'static,
) {
    SHELL_MODE_HANDLERS.with(|handlers| {
        *handlers.borrow_mut() = Some(ShellModeHandlers {
            show_workspace: Box::new(show_workspace),
            show_launcher: Box::new(show_launcher),
        });
    });
}

pub fn install_color_palette_handler(
    apply_color_palette: impl Fn(ColorPalettePreference) + 'static,
) {
    APPEARANCE_HANDLERS.with(|handlers| {
        *handlers.borrow_mut() = Some(AppearanceHandlers {
            apply_color_palette: Box::new(apply_color_palette),
        });
    });
}

pub fn current_color_palette() -> ColorPalettePreference {
    application_settings().color_palette
}

fn automatic_simulation_updates_enabled() -> bool {
    application_settings().automatic_simulation_updates
}

fn set_automatic_simulation_updates(enabled: bool) {
    if automatic_simulation_updates_enabled() == enabled {
        return;
    }
    update_application_settings(|settings| settings.automatic_simulation_updates = enabled);
    if enabled {
        ensure_results_loaded();
    } else {
        clear_automatic_computation();
    }
    refresh_workbench_views();
}

fn set_color_palette(palette: ColorPalettePreference) {
    if current_color_palette() == palette {
        return;
    }
    update_application_settings(|settings| settings.color_palette = palette);
    APPEARANCE_HANDLERS.with(|handlers| {
        if let Some(handlers) = handlers.borrow().as_ref() {
            (handlers.apply_color_palette)(palette);
        }
    });
}

fn set_automatic_project_save(enabled: bool) {
    if application_settings().automatic_project_save == enabled {
        return;
    }
    update_application_settings(|settings| settings.automatic_project_save = enabled);
    if enabled {
        schedule_automatic_save();
    } else {
        clear_automatic_save();
    }
}

extern "C" fn observe_context_event(
    payload: maruzzella_sdk::ffi::MzBytes,
) -> maruzzella_sdk::ffi::MzStatus {
    let event = match decode_json_payload::<MzHostEvent>(payload) {
        Ok(Some(event)) => event,
        Ok(None) | Err(_) => {
            return maruzzella_sdk::ffi::MzStatus::new(MzStatusCode::InvalidArgument);
        }
    };
    let context = match MzSurfaceFocusEvent::from_bytes(&event.payload) {
        Ok(context) => context,
        Err(_) => return maruzzella_sdk::ffi::MzStatus::new(MzStatusCode::InvalidArgument),
    };
    ACTIVE_CONTEXT.with(|slot| {
        *slot.borrow_mut() = Some(context);
    });
    refresh_detail_views();
    maruzzella_sdk::ffi::MzStatus::OK
}

pub fn set_initial_details_view(plugin_view_id: &'static str) {
    INITIAL_DETAILS_VIEW.with(|slot| {
        *slot.borrow_mut() = Some(plugin_view_id);
    });
}

pub fn has_restorable_desktop_session() -> bool {
    load_desktop_session()
        .and_then(|session| session.last_project_path)
        .is_some_and(|path| path.exists())
}

impl Plugin for PvDesktopPlugin {
    fn descriptor() -> PluginDescriptor {
        static DEPENDENCIES: &[PluginDependency] = &[PluginDependency::required(
            "maruzzella.base",
            Version::new(1, 0, 0),
            Version::new(2, 0, 0),
        )];

        PluginDescriptor::new(PLUGIN_ID, "PV Estimator Desktop", Version::new(0, 1, 0))
            .with_description("Engineering-focused PV estimator desktop views")
            .with_dependencies(DEPENDENCIES)
    }

    fn register(host: &HostApi<'_>) -> Result<(), MzStatusCode> {
        host.register_command(
            CommandSpec::new(PLUGIN_ID, CMD_NEW, "New Project").with_handler(command_new_project),
        )?;
        host.register_command(
            CommandSpec::new(PLUGIN_ID, CMD_OPEN, "Open Project")
                .with_handler(command_open_project),
        )?;
        host.register_command(
            CommandSpec::new(PLUGIN_ID, CMD_CLOSE, "Close Project")
                .with_handler(command_close_project)
                .with_enabled(has_open_project_for_command),
        )?;
        host.register_command(
            CommandSpec::new(PLUGIN_ID, CMD_SAVE, "Save Project")
                .with_handler(command_save_project)
                .with_enabled(has_dirty_project_for_command),
        )?;
        host.register_command(
            CommandSpec::new(PLUGIN_ID, CMD_SAVE_AS, "Save Project As")
                .with_handler(command_save_project_as)
                .with_enabled(has_open_project_for_command),
        )?;
        host.register_command(
            CommandSpec::new(PLUGIN_ID, CMD_SET_SIMULATION_RUNS, "Set Simulation Runs")
                .with_handler(command_set_simulation_runs)
                .with_enabled(has_open_project_for_command),
        )?;
        host.register_command(
            CommandSpec::new(PLUGIN_ID, CMD_EXIT, "Exit").with_handler(command_exit_app),
        )?;

        host.register_surface_contribution(SurfaceContributionSpec::about_section(
            PLUGIN_ID,
            "pv-desktop-about",
            "PV Estimator Desktop",
            "Engineering workbench for photovoltaic production and consumption simulations.",
        ))?;

        host.register_host_event_subscriber(
            "maruzzella.context.active_changed",
            observe_context_event,
        )?;

        host.register_view_factory(ViewFactorySpec::new(
            PLUGIN_ID,
            VIEW_LAUNCHER,
            "PV Estimator",
            MzViewPlacement::Workbench,
            create_launcher_view,
        ))?;
        host.register_view_factory(ViewFactorySpec::new(
            PLUGIN_ID,
            VIEW_SYSTEM,
            "System",
            MzViewPlacement::SidePanel,
            create_system_view,
        ))?;
        host.register_view_factory(ViewFactorySpec::new(
            PLUGIN_ID,
            VIEW_ESTIMATE,
            "Estimate",
            MzViewPlacement::Workbench,
            create_estimate_view,
        ))?;
        host.register_view_factory(ViewFactorySpec::new(
            PLUGIN_ID,
            VIEW_SIMULATION,
            "Simulation",
            MzViewPlacement::Workbench,
            create_simulation_view,
        ))?;
        host.register_view_factory(ViewFactorySpec::new(
            PLUGIN_ID,
            VIEW_DETAILS,
            "Details",
            MzViewPlacement::SidePanel,
            create_details_view,
        ))?;
        host.register_view_factory(ViewFactorySpec::new(
            PLUGIN_ID,
            VIEW_SETTINGS,
            "Settings",
            MzViewPlacement::Workbench,
            create_settings_view,
        ))?;

        Ok(())
    }
}

extern "C" fn command_new_project(
    _payload: maruzzella_sdk::ffi::MzBytes,
) -> maruzzella_sdk::ffi::MzStatus {
    create_new_project();
    maruzzella_sdk::ffi::MzStatus::OK
}

fn create_new_project() {
    clear_automatic_computation();
    clear_automatic_save();
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        invalidate_running_simulation(&mut state);
        state.project = Some(PvProjectDocument::default());
        state.path = None;
        state.dirty = true;
        state.session_loaded = true;
        let status = "New project created".to_string();
        state.status = status.clone();
        state.log.push(status);
    });
    save_desktop_session(None);
    ensure_results_loaded();
    show_project_workspace();
    refresh_views();
}

extern "C" fn command_open_project(
    _payload: maruzzella_sdk::ffi::MzBytes,
) -> maruzzella_sdk::ffi::MzStatus {
    show_open_project_dialog();
    maruzzella_sdk::ffi::MzStatus::OK
}

extern "C" fn command_close_project(
    _payload: maruzzella_sdk::ffi::MzBytes,
) -> maruzzella_sdk::ffi::MzStatus {
    close_project();
    maruzzella_sdk::ffi::MzStatus::OK
}

extern "C" fn command_save_project(
    _payload: maruzzella_sdk::ffi::MzBytes,
) -> maruzzella_sdk::ffi::MzStatus {
    if !has_open_project() {
        append_log("Open or create a project first".to_string());
        refresh_views();
        return maruzzella_sdk::ffi::MzStatus::OK;
    }

    match save_current_project() {
        Ok(SaveDisposition::Saved) => maruzzella_sdk::ffi::MzStatus::OK,
        Ok(SaveDisposition::NeedsPath) => {
            show_save_project_dialog();
            maruzzella_sdk::ffi::MzStatus::OK
        }
        Err(message) => {
            append_log(message);
            refresh_views();
            maruzzella_sdk::ffi::MzStatus::new(MzStatusCode::InternalError)
        }
    }
}

extern "C" fn command_save_project_as(
    _payload: maruzzella_sdk::ffi::MzBytes,
) -> maruzzella_sdk::ffi::MzStatus {
    if has_open_project() {
        show_save_project_dialog();
    } else {
        append_log("Open or create a project first".to_string());
        refresh_views();
    }
    maruzzella_sdk::ffi::MzStatus::OK
}

extern "C" fn command_exit_app(
    _payload: maruzzella_sdk::ffi::MzBytes,
) -> maruzzella_sdk::ffi::MzStatus {
    if let Some(application) = gtk::gio::Application::default() {
        application.quit();
    }
    maruzzella_sdk::ffi::MzStatus::OK
}

extern "C" fn command_set_simulation_runs(
    payload: maruzzella_sdk::ffi::MzBytes,
) -> maruzzella_sdk::ffi::MzStatus {
    let Some(runs) = simulation_runs_from_payload(payload) else {
        append_log("Invalid simulation run count".to_string());
        refresh_views();
        return maruzzella_sdk::ffi::MzStatus::new(MzStatusCode::InvalidArgument);
    };

    set_simulation_runs(runs);
    maruzzella_sdk::ffi::MzStatus::OK
}

fn set_simulation_runs(runs: usize) {
    if current_simulation_runs() == runs {
        return;
    }
    update_simulation_state(|state| {
        let Some(project) = state.project.as_mut() else {
            return;
        };
        project.inputs.simulation_options.runs = runs;
        let message = format!("Simulation runs set to {}", format_runs(runs));
        state.status = message.clone();
        state.log.push(message);
    });
    sync_simulation_run_controls(runs);
}

fn sync_simulation_run_controls(runs: usize) {
    let Some(selected) = SIMULATION_RUN_OPTIONS
        .iter()
        .position(|candidate| *candidate == runs)
        .map(|index| index as u32)
    else {
        return;
    };
    let Some(window) = active_window() else {
        return;
    };
    sync_simulation_run_controls_in(&window.upcast(), selected);
}

fn sync_simulation_run_controls_in(widget: &gtk::Widget, selected: u32) {
    if let Some(dropdown) = widget.downcast_ref::<DropDown>()
        && (dropdown.has_css_class("simulation-runs-control")
            || dropdown.tooltip_text().as_deref() == Some("Simulation runs"))
        && dropdown.selected() != selected
    {
        dropdown.set_selected(selected);
    }

    let mut child = widget.first_child();
    while let Some(current) = child {
        sync_simulation_run_controls_in(&current, selected);
        child = current.next_sibling();
    }
}

pub fn current_simulation_runs() -> usize {
    ensure_session_loaded();
    STATE.with(|state| {
        state
            .borrow()
            .project
            .as_ref()
            .map(|project| project.inputs.simulation_options.runs)
            .unwrap_or(10_000)
    })
}

fn simulation_runs_from_payload(payload: maruzzella_sdk::ffi::MzBytes) -> Option<usize> {
    if payload.ptr.is_null() {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(payload.ptr, payload.len) };
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|runs| *runs > 0)
}

fn compute_estimate_for_project(
    project: &PvProjectDocument,
) -> Result<(SourceEnsembleEstimateDocument, ProductionProfile, f64), String> {
    let request = project.inputs.estimate_request.clone();
    let arrays = project.inputs.arrays.clone();
    ESTIMATOR.with(|estimator| {
        let mut estimator = estimator.borrow_mut();
        if estimator.is_none() {
            *estimator =
                Some(SourceModelEstimator::load_embedded().map_err(|error| {
                    format!("Failed to load embedded model artifacts: {error:#}")
                })?);
        }
        let estimator = estimator
            .as_mut()
            .expect("embedded estimator is initialized above");
        let finished = estimator
            .estimate_arrays_with_profile(&request, &arrays)
            .map_err(|error| format!("Estimate failed: {error:#}"))?;
        let estimate = finished.estimate;
        let production_profile = finished.production_profile;
        let annual_kwh = estimate
            .ensemble_estimate
            .annual_energy
            .mean
            .as_kilowatt_hours();
        Ok((estimate, production_profile, annual_kwh))
    })
}

fn store_estimate_result(
    estimate: SourceEnsembleEstimateDocument,
    production_profile: ProductionProfile,
    status: String,
    push_log: bool,
    mark_dirty: bool,
) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        {
            let Some(project) = state.project.as_mut() else {
                return;
            };
            project.results.estimate = Some(estimate);
            project.results.production_profile = Some(production_profile);
            project.results.simulation = None;
            project.results.simulation_metadata = None;
        }
        invalidate_running_simulation(&mut state);
        if mark_dirty {
            state.dirty = true;
        }
        state.status = status.clone();
        if push_log {
            state.log.push(status);
        }
    });
    if mark_dirty {
        schedule_automatic_save();
    }
}

fn recompute_current_estimate(
    status_prefix: &str,
    push_log: bool,
    mark_dirty: bool,
) -> Result<(), String> {
    let Some(project) = STATE.with(|state| state.borrow().project.clone()) else {
        return Ok(());
    };
    let (estimate, production_profile, annual_kwh) = compute_estimate_for_project(&project)?;
    store_estimate_result(
        estimate,
        production_profile,
        format!("{status_prefix}: {annual_kwh:.0} kWh/year"),
        push_log,
        mark_dirty,
    );
    Ok(())
}

fn ensure_results_loaded() {
    let work_in_flight = simulation_run_snapshot().is_some()
        || AUTOMATIC_COMPUTATION.with(|automatic| automatic.borrow().pending.is_some());
    if work_in_flight {
        return;
    }
    let impact = STATE.with(|state| {
        let mut state = state.borrow_mut();
        let project = state.project.as_mut()?;
        let impact = required_computation(
            project.results.estimate.is_some(),
            project.results.production_profile.is_some(),
            project
                .results
                .simulation
                .as_ref()
                .map(|result| (result.cancelled, result.requested_runs)),
            project.inputs.simulation_options.runs,
        );
        match impact {
            Some(ComputationImpact::EstimateAndSimulation) => {
                project.results.estimate = None;
                project.results.production_profile = None;
                project.results.simulation = None;
                project.results.simulation_metadata = None;
            }
            Some(ComputationImpact::Simulation) => {
                project.results.simulation = None;
                project.results.simulation_metadata = None;
            }
            None => {}
        }
        impact
    });
    if let Some(impact) = impact {
        schedule_automatic_computation(impact, ComputationTrigger::Immediate);
    }
}

fn required_computation(
    has_estimate: bool,
    has_production_profile: bool,
    simulation: Option<(bool, usize)>,
    requested_runs: usize,
) -> Option<ComputationImpact> {
    if !has_estimate || !has_production_profile {
        Some(ComputationImpact::EstimateAndSimulation)
    } else if simulation.is_none_or(|(cancelled, completed_request)| {
        cancelled || completed_request != requested_runs
    }) {
        Some(ComputationImpact::Simulation)
    } else {
        None
    }
}

fn schedule_automatic_computation(impact: ComputationImpact, trigger: ComputationTrigger) {
    let generation = AUTOMATIC_COMPUTATION.with(|automatic| {
        let mut automatic = automatic.borrow_mut();
        if let Some(source) = automatic.debounce.take() {
            source.remove();
        }
        automatic.generation = automatic.generation.wrapping_add(1);
        automatic.pending = Some(
            automatic
                .pending
                .map_or(impact, |pending| pending.merge(impact)),
        );
        automatic.generation
    });
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.computation_phase = match trigger {
            ComputationTrigger::Debounced => ComputationPhase::Debouncing,
            ComputationTrigger::Immediate => ComputationPhase::Idle,
        };
        state.retry_stage = None;
    });

    match trigger {
        ComputationTrigger::Debounced => {
            let source = gtk::glib::timeout_add_local_once(COMPUTATION_DEBOUNCE, move || {
                AUTOMATIC_COMPUTATION.with(|automatic| {
                    let mut automatic = automatic.borrow_mut();
                    if automatic.generation == generation {
                        automatic.debounce = None;
                    }
                });
                execute_pending_computation(generation);
            });
            AUTOMATIC_COMPUTATION.with(|automatic| {
                let mut automatic = automatic.borrow_mut();
                if automatic.generation == generation {
                    automatic.debounce = Some(source);
                } else {
                    source.remove();
                }
            });
        }
        ComputationTrigger::Immediate => {
            gtk::glib::idle_add_local_once(move || execute_pending_computation(generation));
        }
    }
    refresh_workbench_views();
}

fn flush_pending_computation() {
    let generation = AUTOMATIC_COMPUTATION.with(|automatic| {
        let mut automatic = automatic.borrow_mut();
        automatic.pending?;
        if let Some(source) = automatic.debounce.take() {
            source.remove();
        }
        Some(automatic.generation)
    });
    if let Some(generation) = generation {
        execute_pending_computation(generation);
    }
}

fn execute_pending_computation(generation: u64) {
    let impact = AUTOMATIC_COMPUTATION.with(|automatic| {
        let mut automatic = automatic.borrow_mut();
        if automatic.generation != generation {
            return None;
        }
        if simulation_run_snapshot().is_some() {
            STATE.with(|state| {
                state.borrow_mut().computation_phase = ComputationPhase::WaitingForSimulation;
            });
            return None;
        }
        automatic.pending.take()
    });
    let Some(mut impact) = impact else {
        return;
    };

    let missing_estimate = STATE.with(|state| {
        state.borrow().project.as_ref().is_some_and(|project| {
            project.results.estimate.is_none() || project.results.production_profile.is_none()
        })
    });
    if missing_estimate {
        impact = ComputationImpact::EstimateAndSimulation;
    }

    if impact == ComputationImpact::EstimateAndSimulation {
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.computation_phase = ComputationPhase::Estimating;
            state.status = "Updating estimate".to_string();
        });
        refresh_workbench_views();
        if let Err(message) = recompute_current_estimate("Estimate updated", false, true) {
            computation_failed(ComputationStage::Estimate, message);
            return;
        }
    }

    STATE.with(|state| state.borrow_mut().computation_phase = ComputationPhase::Idle);
    match run_simulation() {
        Ok(()) => {}
        Err(RunSimulationError::NeedsProject) => clear_automatic_computation(),
        Err(RunSimulationError::NeedsEstimate) => computation_failed(
            ComputationStage::Estimate,
            "Estimate data is unavailable".to_string(),
        ),
    }
}

fn computation_failed(stage: ComputationStage, message: String) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.computation_phase = ComputationPhase::Idle;
        state.retry_stage = Some(stage);
        state.status = message.clone();
        state.log.push(message);
    });
    refresh_workbench_views();
}

fn retry_automatic_computation(stage: ComputationStage) {
    let impact = match stage {
        ComputationStage::Estimate => ComputationImpact::EstimateAndSimulation,
        ComputationStage::Simulation => ComputationImpact::Simulation,
    };
    schedule_automatic_computation(impact, ComputationTrigger::Immediate);
}

fn clear_automatic_computation() {
    AUTOMATIC_COMPUTATION.with(|automatic| {
        let mut automatic = automatic.borrow_mut();
        if let Some(source) = automatic.debounce.take() {
            source.remove();
        }
        automatic.generation = automatic.generation.wrapping_add(1);
        automatic.pending = None;
    });
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.computation_phase = ComputationPhase::Idle;
        state.retry_stage = None;
    });
}

fn resume_pending_computation() {
    let generation = AUTOMATIC_COMPUTATION.with(|automatic| {
        let automatic = automatic.borrow();
        (automatic.pending.is_some() && automatic.debounce.is_none())
            .then_some(automatic.generation)
    });
    if let Some(generation) = generation {
        gtk::glib::idle_add_local_once(move || execute_pending_computation(generation));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RunSimulationError {
    NeedsProject,
    NeedsEstimate,
}

fn run_simulation() -> Result<(), RunSimulationError> {
    if simulation_run_snapshot().is_some() {
        append_log("Simulation already running".to_string());
        refresh_workbench_views();
        return Ok(());
    }

    let Some((production, load, storage, options, generation)) = STATE.with(|state| {
        let state = state.borrow();
        state.project.as_ref().map(|project| {
            (
                project.results.production_profile.clone(),
                project.inputs.load_profile.clone(),
                project.inputs.estimate_request.storage_usable_kwh,
                project.inputs.simulation_options,
                state.simulation_generation,
            )
        })
    }) else {
        return Err(RunSimulationError::NeedsProject);
    };
    let Some(production) = production else {
        return Err(RunSimulationError::NeedsEstimate);
    };

    let request = SimulationRequest {
        production,
        load,
        storage: storage.map(|usable_capacity_kwh| StorageConfig {
            usable_capacity_kwh,
        }),
        options,
    };
    let requested_runs = request.options.runs;
    let started_at = Instant::now();
    let started_wall_time = SystemTime::now();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    let (sender, receiver) = mpsc::channel();

    thread::spawn(move || {
        let progress_sender = sender.clone();
        let progress_step = (requested_runs / 200).max(1);
        let mut last_reported = 0usize;
        let result = simulate_with_progress(
            &request,
            || worker_cancel.load(Ordering::Relaxed),
            |completed_runs| {
                let should_report = completed_runs == 1
                    || completed_runs == requested_runs
                    || completed_runs.saturating_sub(last_reported) >= progress_step;
                if should_report {
                    last_reported = completed_runs;
                    let _ = progress_sender.send(SimulationRunMessage::Progress(completed_runs));
                }
            },
        )
        .map_err(|error| format!("Simulation failed: {error}"));
        let _ = sender.send(SimulationRunMessage::Finished(Box::new(result)));
    });

    SIMULATION_RUN.with(|run| {
        *run.borrow_mut() = Some(SimulationRunState {
            requested_runs,
            completed_runs: 0,
            cancel,
            cancelling: false,
            receiver,
            generation,
            started_at,
            started_wall_time,
        });
    });
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.computation_phase = ComputationPhase::Idle;
        state.retry_stage = None;
        if let Some(project) = state.project.as_mut() {
            project.results.simulation = None;
            project.results.simulation_metadata = None;
            state.dirty = true;
        }
        let message = format!(
            "Running simulation with {} runs",
            format_runs(requested_runs)
        );
        state.status = message.clone();
        state.log.push(message);
    });
    schedule_automatic_save();
    schedule_simulation_poll();
    refresh_workbench_views();
    Ok(())
}

fn schedule_simulation_poll() {
    gtk::glib::timeout_add_local(Duration::from_millis(100), || {
        poll_simulation_run();
        if simulation_run_snapshot().is_some() {
            gtk::glib::ControlFlow::Continue
        } else {
            gtk::glib::ControlFlow::Break
        }
    });
}

fn simulation_run_snapshot() -> Option<SimulationRunSnapshot> {
    SIMULATION_RUN.with(|run| {
        run.borrow().as_ref().map(|run| SimulationRunSnapshot {
            requested_runs: run.requested_runs,
            completed_runs: run.completed_runs,
            cancelling: run.cancelling,
            started_at: run.started_at,
            started_wall_time: run.started_wall_time,
        })
    })
}

fn cancel_simulation_run() {
    let cancelled = STATE.with(|state| {
        let mut state = state.borrow_mut();
        if simulation_run_snapshot().is_none() {
            return false;
        }
        invalidate_running_simulation(&mut state);
        state.computation_phase = ComputationPhase::Idle;
        state.retry_stage = Some(ComputationStage::Simulation);
        state.status = "Cancelling simulation".to_string();
        true
    });
    if cancelled {
        AUTOMATIC_COMPUTATION.with(|automatic| {
            let mut automatic = automatic.borrow_mut();
            if let Some(source) = automatic.debounce.take() {
                source.remove();
            }
            automatic.generation = automatic.generation.wrapping_add(1);
            automatic.pending = None;
        });
        if let Some(run) = simulation_run_snapshot() {
            update_simulation_progress_views(run);
        }
        refresh_detail_views();
    }
}

fn request_simulation_cancel() -> bool {
    SIMULATION_RUN.with(|run| {
        let mut run = run.borrow_mut();
        let Some(run) = run.as_mut() else {
            return false;
        };
        run.cancelling = true;
        run.cancel.store(true, Ordering::Relaxed);
        true
    })
}

fn invalidate_running_simulation(state: &mut DesktopState) {
    state.simulation_generation = state.simulation_generation.wrapping_add(1);
    let _ = request_simulation_cancel();
}

fn poll_simulation_run() {
    let mut finished = None;
    let mut finished_metadata = None;
    let mut progress_changed = false;
    let mut progress_status = None;

    SIMULATION_RUN.with(|run| {
        let mut run = run.borrow_mut();
        let Some(run) = run.as_mut() else {
            return;
        };
        loop {
            match run.receiver.try_recv() {
                Ok(SimulationRunMessage::Progress(completed_runs)) => {
                    if completed_runs > run.completed_runs {
                        run.completed_runs = completed_runs.min(run.requested_runs);
                        progress_changed = true;
                    }
                }
                Ok(SimulationRunMessage::Finished(result)) => {
                    finished_metadata = Some(build_simulation_run_metadata(
                        run.started_at,
                        run.started_wall_time,
                        SystemTime::now(),
                    ));
                    finished = Some((*result, run.generation));
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    finished_metadata = Some(build_simulation_run_metadata(
                        run.started_at,
                        run.started_wall_time,
                        SystemTime::now(),
                    ));
                    finished = Some((
                        Err("Simulation worker disconnected".to_string()),
                        run.generation,
                    ));
                    break;
                }
            }
        }
        if progress_changed {
            progress_status = Some(simulation_progress_status(
                run.completed_runs,
                run.requested_runs,
                run.cancelling,
            ));
        }
    });

    if let Some((result, generation)) = finished {
        SIMULATION_RUN.with(|run| {
            run.borrow_mut().take();
        });
        finish_simulation_run(result, generation, finished_metadata);
        resume_pending_computation();
        refresh_workbench_views();
    } else if let Some(status) = progress_status {
        STATE.with(|state| {
            state.borrow_mut().status = status;
        });
        if let Some(run) = simulation_run_snapshot() {
            update_simulation_progress_views(run);
        }
        refresh_detail_views();
    }
}

fn finish_simulation_run(
    result: Result<SimulationResult, String>,
    generation: u64,
    metadata: Option<SimulationRunMetadata>,
) {
    let stale = STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.simulation_generation == generation {
            return false;
        }
        if state.project.is_none() {
            return true;
        }
        if state.retry_stage == Some(ComputationStage::Simulation) {
            state.status = "Simulation cancelled".to_string();
        } else if state.computation_phase != ComputationPhase::WaitingForSimulation {
            let status = "Simulation result discarded because inputs changed".to_string();
            state.status = status.clone();
            state.log.push(status);
        }
        true
    });
    if stale {
        return;
    }

    match result {
        Ok(result) => {
            let self_sufficiency = result.summaries.self_sufficiency_ratio.p50;
            let status = if result.cancelled {
                format!(
                    "Simulation cancelled: {} / {} runs",
                    format_runs(result.completed_runs),
                    format_runs(result.requested_runs)
                )
            } else {
                format!(
                    "Simulation complete: {:.0}% self sufficiency",
                    self_sufficiency * 100.0
                )
            };
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                let Some(project) = state.project.as_mut() else {
                    return;
                };
                project.results.simulation = Some(result);
                project.results.simulation_metadata = metadata;
                state.computation_phase = ComputationPhase::Idle;
                state.retry_stage = None;
                state.dirty = true;
                state.status = status.clone();
                state.log.push(status);
            });
            schedule_automatic_save();
        }
        Err(message) => computation_failed(ComputationStage::Simulation, message),
    }
}

fn build_simulation_run_metadata(
    started_at: Instant,
    started_wall_time: SystemTime,
    completed_wall_time: SystemTime,
) -> SimulationRunMetadata {
    SimulationRunMetadata {
        started_at: format_system_time(started_wall_time),
        completed_at: format_system_time(completed_wall_time),
        elapsed_ms: started_at.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
    }
}

fn format_system_time(time: SystemTime) -> String {
    unix_seconds(time)
        .and_then(|seconds| {
            gtk::glib::DateTime::from_unix_local(seconds)
                .ok()
                .and_then(|datetime| datetime.format_iso8601().ok())
                .map(|value| value.to_string())
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn unix_seconds(time: SystemTime) -> Option<i64> {
    let seconds = time.duration_since(UNIX_EPOCH).ok()?.as_secs();
    i64::try_from(seconds).ok()
}

fn format_duration(duration: Duration) -> String {
    let total_ms = duration.as_millis();
    if total_ms < 1_000 {
        return format!("{} ms", total_ms);
    }
    let total_seconds = duration.as_secs();
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    if minutes == 0 {
        format!("{seconds}s")
    } else {
        format!("{minutes}m {seconds}s")
    }
}

fn simulation_progress_status(
    completed_runs: usize,
    requested_runs: usize,
    cancelling: bool,
) -> String {
    if cancelling {
        format!(
            "Cancelling simulation: {} / {} runs",
            format_runs(completed_runs),
            format_runs(requested_runs)
        )
    } else {
        format!(
            "Simulation running: {} / {} runs",
            format_runs(completed_runs),
            format_runs(requested_runs)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SaveDisposition {
    Saved,
    NeedsPath,
}

extern "C" fn has_open_project_for_command() -> bool {
    has_open_project()
}

extern "C" fn has_dirty_project_for_command() -> bool {
    has_dirty_project()
}

fn has_dirty_project() -> bool {
    ensure_session_loaded();
    STATE.with(|state| {
        let state = state.borrow();
        state.project.is_some() && state.dirty
    })
}

fn has_open_project() -> bool {
    ensure_session_loaded();
    STATE.with(|state| state.borrow().project.is_some())
}

fn clear_automatic_save() {
    AUTOMATIC_SAVE.with(|pending| {
        if let Some(source) = pending.borrow_mut().take() {
            source.remove();
        }
    });
}

fn schedule_automatic_save() {
    clear_automatic_save();
    if !application_settings().automatic_project_save {
        return;
    }
    let eligible = STATE.with(|state| {
        let state = state.borrow();
        state.project.is_some() && state.path.is_some() && state.dirty
    });
    if !eligible {
        return;
    }

    let source = gtk::glib::timeout_add_local_once(AUTO_SAVE_DEBOUNCE, || {
        AUTOMATIC_SAVE.with(|pending| {
            pending.borrow_mut().take();
        });
        if let Err(message) = save_current_project() {
            append_log(format!("Automatic save failed: {message}"));
            refresh_views();
        }
    });
    AUTOMATIC_SAVE.with(|pending| {
        *pending.borrow_mut() = Some(source);
    });
}

fn save_current_project() -> Result<SaveDisposition, String> {
    clear_automatic_save();
    let (path, project) = STATE.with(|state| {
        let state = state.borrow();
        (state.path.clone(), state.project.clone())
    });
    let Some(project) = project else {
        return Err("Open or create a project first".to_string());
    };
    let Some(path) = path else {
        return Ok(SaveDisposition::NeedsPath);
    };
    save_project(&path, &project).map_err(|error| format!("Save failed: {error:#}"))?;
    save_desktop_session(Some(&path));
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.dirty = false;
        let status = format!("Saved {}", path.display());
        state.status = status.clone();
        state.log.push(status);
    });
    refresh_views();
    Ok(SaveDisposition::Saved)
}

fn open_project(path: PathBuf) {
    clear_automatic_save();
    if load_project_into_state(&path, true) {
        ensure_results_loaded();
        show_project_workspace();
    }
    refresh_views();
}

fn close_project() {
    clear_automatic_save();
    clear_automatic_computation();
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        invalidate_running_simulation(&mut state);
        state.project = None;
        state.path = None;
        state.dirty = false;
        state.session_loaded = true;
        let status = "No project open".to_string();
        state.status = status.clone();
        state.log.push(status);
    });
    save_desktop_session(None);
    show_no_project_launcher();
    refresh_views();
}

fn show_project_workspace() {
    SHELL_MODE_HANDLERS.with(|handlers| {
        if let Some(handlers) = handlers.borrow().as_ref() {
            (handlers.show_workspace)();
        }
    });
}

fn show_no_project_launcher() {
    SHELL_MODE_HANDLERS.with(|handlers| {
        if let Some(handlers) = handlers.borrow().as_ref() {
            (handlers.show_launcher)();
        }
    });
}

fn load_project_into_state(path: &Path, remember: bool) -> bool {
    match load_project(path) {
        Ok(project) => {
            clear_automatic_computation();
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                invalidate_running_simulation(&mut state);
                state.project = Some(project);
                state.path = Some(path.to_path_buf());
                state.dirty = false;
                state.session_loaded = true;
                let status = format!("Opened {}", path.display());
                state.status = status.clone();
                state.log.push(status);
            });
            if remember {
                save_desktop_session(Some(path));
            }
            true
        }
        Err(error) => {
            append_log(format!("Open failed: {error:#}"));
            false
        }
    }
}

fn save_project_as(path: PathBuf) {
    clear_automatic_save();
    let path = ensure_project_extension(path);
    let Some(project) = STATE.with(|state| state.borrow().project.clone()) else {
        append_log("Open or create a project first".to_string());
        refresh_views();
        return;
    };
    match save_project(&path, &project) {
        Ok(()) => {
            save_desktop_session(Some(&path));
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.path = Some(path.clone());
                state.dirty = false;
                let status = format!("Saved {}", path.display());
                state.status = status.clone();
                state.log.push(status);
            });
        }
        Err(error) => append_log(format!("Save failed: {error:#}")),
    }
    refresh_views();
}

fn ensure_session_loaded() {
    let already_loaded = STATE.with(|state| state.borrow().session_loaded);
    if already_loaded {
        return;
    }

    let last_project_path = load_desktop_session().and_then(|session| session.last_project_path);
    match last_project_path {
        Some(path) if path.exists() => {
            if !load_project_into_state(&path, false) {
                STATE.with(|state| state.borrow_mut().session_loaded = true);
                save_desktop_session(None);
            }
        }
        Some(path) => {
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.session_loaded = true;
                let status = format!("Last project not found: {}", path.display());
                state.status = status.clone();
                state.log.push(status);
            });
            save_desktop_session(None);
        }
        None => STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.session_loaded = true;
            state.project = None;
            state.path = None;
            state.dirty = false;
            state.status = "No project open".to_string();
        }),
    }
}

fn desktop_session_path() -> Option<PathBuf> {
    ProjectDirs::from("dev", "lelloman", "pv-estimator")
        .map(|dirs| dirs.config_dir().join("pv-desktop-session.json"))
}

fn load_desktop_session() -> Option<DesktopSession> {
    let path = desktop_session_path()?;
    let bytes = fs::read(path).ok()?;
    let session: DesktopSession = serde_json::from_slice(&bytes).ok()?;
    (session.schema_version == DESKTOP_SESSION_SCHEMA_VERSION).then_some(session)
}

fn application_settings() -> ApplicationSettings {
    if let Some(settings) = APPLICATION_SETTINGS.with(|settings| settings.borrow().clone()) {
        return settings;
    }
    let settings = load_desktop_session()
        .map(|session| session.settings)
        .unwrap_or_default();
    APPLICATION_SETTINGS.with(|slot| {
        *slot.borrow_mut() = Some(settings.clone());
    });
    settings
}

fn persist_application_settings() {
    let state_project_path = STATE.with(|state| {
        let state = state.borrow();
        state.session_loaded.then(|| state.path.clone())
    });
    let last_project_path = state_project_path
        .unwrap_or_else(|| load_desktop_session().and_then(|session| session.last_project_path));
    save_desktop_session(last_project_path.as_deref());
}

fn update_application_settings(update: impl FnOnce(&mut ApplicationSettings)) {
    let mut settings = application_settings();
    update(&mut settings);
    APPLICATION_SETTINGS.with(|slot| {
        *slot.borrow_mut() = Some(settings);
    });
    persist_application_settings();
}

fn save_desktop_session(last_project_path: Option<&Path>) {
    let Some(path) = desktop_session_path() else {
        return;
    };
    let Some(parent) = path.parent() else {
        return;
    };
    if fs::create_dir_all(parent).is_err() {
        return;
    }
    let session = DesktopSession {
        schema_version: DESKTOP_SESSION_SCHEMA_VERSION,
        last_project_path: last_project_path.map(Path::to_path_buf),
        settings: application_settings(),
    };
    if let Ok(bytes) = serde_json::to_vec_pretty(&session) {
        let _ = fs::write(path, bytes);
    }
}

fn ensure_project_extension(path: PathBuf) -> PathBuf {
    if path.extension().and_then(|extension| extension.to_str()) == Some(PROJECT_EXTENSION) {
        path
    } else {
        path.with_extension(PROJECT_EXTENSION)
    }
}

fn pv_project_file_filter() -> FileFilter {
    let filter = FileFilter::new();
    filter.set_name(Some("PV projects (*.pvproj)"));
    filter.add_pattern("*.pvproj");
    filter
}

fn active_window() -> Option<Window> {
    gtk::gio::Application::default()
        .and_then(|application| application.downcast::<gtk::Application>().ok())
        .and_then(|application| application.active_window())
        .map(|window| window.upcast())
}

fn apply_text_role(label: &Label, role: &str) {
    label.add_css_class(&text_css_class(role));
}

fn apply_button_role(button: &Button, role: &str) {
    button.add_css_class(&button_css_class(role));
}

fn apply_input_role(entry: &Entry, role: &str) {
    entry.add_css_class(&input_css_class(role));
}

fn field_entry() -> Entry {
    let entry = Entry::new();
    apply_input_role(&entry, "field");
    entry
}

fn search_entry() -> Entry {
    let entry = Entry::new();
    apply_input_role(&entry, "search");
    entry
}

fn keep_file_chooser_alive(dialog: &FileChooserDialog) {
    ACTIVE_FILE_CHOOSER.with(|active_dialog| {
        *active_dialog.borrow_mut() = Some(dialog.clone());
    });
}

fn release_file_chooser(dialog: &FileChooserDialog) {
    ACTIVE_FILE_CHOOSER.with(|active_dialog| {
        let mut active_dialog = active_dialog.borrow_mut();
        if active_dialog
            .as_ref()
            .is_some_and(|active_dialog| active_dialog.as_ptr() == dialog.as_ptr())
        {
            active_dialog.take();
        }
    });
}

fn show_open_project_dialog() {
    if !gtk::is_initialized_main_thread() && gtk::init().is_err() {
        append_log("GTK is not initialized; cannot open file dialog".to_string());
        return;
    }
    let parent = active_window();
    let dialog = FileChooserDialog::new(
        Some("Open PV Project"),
        parent.as_ref(),
        FileChooserAction::Open,
        &[
            ("Cancel", ResponseType::Cancel),
            ("Open", ResponseType::Accept),
        ],
    );
    dialog.add_filter(&pv_project_file_filter());
    dialog.add_css_class("app-dialog");
    dialog.set_modal(true);
    keep_file_chooser_alive(&dialog);
    dialog.connect_response(|dialog, response| {
        if response == ResponseType::Accept
            && let Some(file) = dialog.file()
            && let Some(path) = file.path()
        {
            open_project(path);
        }
        dialog.destroy();
        release_file_chooser(dialog);
    });
    dialog.present();
}

fn show_save_project_dialog() {
    if !gtk::is_initialized_main_thread() && gtk::init().is_err() {
        append_log("GTK is not initialized; cannot open file dialog".to_string());
        return;
    }
    let parent = active_window();
    let dialog = FileChooserDialog::new(
        Some("Save PV Project"),
        parent.as_ref(),
        FileChooserAction::Save,
        &[
            ("Cancel", ResponseType::Cancel),
            ("Save", ResponseType::Accept),
        ],
    );
    dialog.add_filter(&pv_project_file_filter());
    dialog.add_css_class("app-dialog");
    dialog.set_current_name("untitled.pvproj");
    dialog.set_modal(true);
    keep_file_chooser_alive(&dialog);
    dialog.connect_response(|dialog, response| {
        if response == ResponseType::Accept
            && let Some(file) = dialog.file()
            && let Some(path) = file.path()
        {
            save_project_as(path);
        }
        dialog.destroy();
        release_file_chooser(dialog);
    });
    dialog.present();
}

extern "C" fn create_launcher_view(
    _host: *const maruzzella_sdk::ffi::MzHostApi,
    _request: *const maruzzella_sdk::ffi::MzViewRequest,
) -> *mut c_void {
    if !gtk::is_initialized_main_thread() && gtk::init().is_err() {
        return std::ptr::null_mut();
    }
    let root = GtkBox::new(Orientation::Vertical, 0);
    root.set_hexpand(true);
    root.set_vexpand(true);
    render_launcher_into(&root);
    widget_ptr(root)
}

extern "C" fn create_system_view(
    _host: *const maruzzella_sdk::ffi::MzHostApi,
    _request: *const maruzzella_sdk::ffi::MzViewRequest,
) -> *mut c_void {
    if !gtk::is_initialized_main_thread() && gtk::init().is_err() {
        return std::ptr::null_mut();
    }
    ensure_session_loaded();
    ensure_results_loaded();
    let root = GtkBox::new(Orientation::Vertical, 0);
    root.set_hexpand(true);
    root.set_vexpand(true);
    render_system_into(&root);
    remember_view(&SYSTEM_VIEWS, &root);
    widget_ptr(root)
}

extern "C" fn create_estimate_view(
    _host: *const maruzzella_sdk::ffi::MzHostApi,
    _request: *const maruzzella_sdk::ffi::MzViewRequest,
) -> *mut c_void {
    if !gtk::is_initialized_main_thread() && gtk::init().is_err() {
        return std::ptr::null_mut();
    }
    ensure_session_loaded();
    ensure_results_loaded();
    let root = GtkBox::new(Orientation::Vertical, 0);
    root.set_hexpand(true);
    root.set_vexpand(true);
    render_estimate_into(&root);
    remember_view(&ESTIMATE_VIEWS, &root);
    widget_ptr(root)
}

extern "C" fn create_simulation_view(
    _host: *const maruzzella_sdk::ffi::MzHostApi,
    _request: *const maruzzella_sdk::ffi::MzViewRequest,
) -> *mut c_void {
    if !gtk::is_initialized_main_thread() && gtk::init().is_err() {
        return std::ptr::null_mut();
    }
    ensure_session_loaded();
    ensure_results_loaded();
    let root = GtkBox::new(Orientation::Vertical, 0);
    root.set_hexpand(true);
    root.set_vexpand(true);
    render_simulation_into(&root);
    remember_view(&SIMULATION_VIEWS, &root);
    widget_ptr(root)
}

extern "C" fn create_details_view(
    _host: *const maruzzella_sdk::ffi::MzHostApi,
    _request: *const maruzzella_sdk::ffi::MzViewRequest,
) -> *mut c_void {
    if !gtk::is_initialized_main_thread() && gtk::init().is_err() {
        return std::ptr::null_mut();
    }
    ensure_session_loaded();
    ensure_results_loaded();
    let root = GtkBox::new(Orientation::Vertical, 0);
    root.set_hexpand(true);
    root.set_vexpand(true);
    root.set_size_request(DETAILS_PANEL_MIN_WIDTH, -1);
    render_details_into(&root);
    remember_view(&DETAIL_VIEWS, &root);
    widget_ptr(root)
}

extern "C" fn create_settings_view(
    _host: *const maruzzella_sdk::ffi::MzHostApi,
    _request: *const maruzzella_sdk::ffi::MzViewRequest,
) -> *mut c_void {
    if !gtk::is_initialized_main_thread() && gtk::init().is_err() {
        return std::ptr::null_mut();
    }

    let page = GtkBox::new(Orientation::Vertical, 0);
    page.add_css_class("settings-page");
    page.set_margin_top(0);
    page.set_margin_bottom(40);
    page.set_margin_start(32);
    page.set_margin_end(32);
    page.set_hexpand(true);
    let settings = application_settings();

    let run_labels = SIMULATION_RUN_OPTIONS.map(format_runs);
    let run_label_refs = run_labels.iter().map(String::as_str).collect::<Vec<_>>();
    let runs = DropDown::from_strings(&run_label_refs);
    runs.set_selected(
        SIMULATION_RUN_OPTIONS
            .iter()
            .position(|runs| *runs == current_simulation_runs())
            .unwrap_or(1) as u32,
    );
    runs.set_size_request(220, -1);
    runs.add_css_class("settings-control");
    runs.add_css_class("simulation-runs-control");
    runs.set_sensitive(has_open_project());
    mark_clickable(&runs);
    runs.connect_selected_notify(move |dropdown| {
        let selected = dropdown.selected();
        if selected == gtk::INVALID_LIST_POSITION {
            return;
        }
        if let Some(runs) = SIMULATION_RUN_OPTIONS.get(selected as usize) {
            set_simulation_runs(*runs);
        }
    });

    let automatic_updates = Switch::builder()
        .active(settings.automatic_simulation_updates)
        .valign(Align::Center)
        .build();
    automatic_updates.add_css_class("settings-switch");
    mark_clickable(&automatic_updates);
    automatic_updates.connect_active_notify(|toggle| {
        set_automatic_simulation_updates(toggle.is_active());
    });

    let simulation = settings_group();
    simulation.append(&settings_row(
        "Simulation runs",
        "Higher values produce more stable results but take longer to calculate.",
        &runs,
    ));
    simulation.append(&Separator::new(Orientation::Horizontal));
    simulation.append(&settings_row(
        "Update simulations automatically",
        "Recalculate results after system parameters change.",
        &automatic_updates,
    ));
    page.append(&simulation);
    page.append(&Separator::new(Orientation::Horizontal));

    let palette_group = GtkBox::new(Orientation::Vertical, 0);
    palette_group.add_css_class("settings-palette-group");
    let (system_palette, system_radio) = palette_option(
        "System",
        "Follow the desktop appearance",
        &["#f4f6f8", "#d9dee5", "#323842", "#171a1f"],
        None,
        ColorPalettePreference::System,
    );
    let (light_palette, light_radio) = palette_option(
        "Light",
        "Bright surfaces with dark text",
        &["#ffffff", "#edf1f5", "#cad2dc", "#2463a7"],
        Some(&system_radio),
        ColorPalettePreference::Light,
    );
    let (dark_palette, dark_radio) = palette_option(
        "Dark",
        "Low-glare surfaces with light text",
        &["#111419", "#1c222a", "#394451", "#4f91d8"],
        Some(&system_radio),
        ColorPalettePreference::Dark,
    );
    match settings.color_palette {
        ColorPalettePreference::System => system_radio.set_active(true),
        ColorPalettePreference::Light => light_radio.set_active(true),
        ColorPalettePreference::Dark => dark_radio.set_active(true),
    }
    palette_group.append(&system_palette);
    palette_group.append(&Separator::new(Orientation::Horizontal));
    palette_group.append(&light_palette);
    palette_group.append(&Separator::new(Orientation::Horizontal));
    palette_group.append(&dark_palette);

    let appearance = settings_group();
    appearance.append(&settings_label(
        "Color palette",
        "Choose the application-wide appearance.",
    ));
    appearance.append(&Separator::new(Orientation::Horizontal));
    appearance.append(&palette_group);
    page.append(&appearance);
    page.append(&Separator::new(Orientation::Horizontal));

    let automatic_save = Switch::builder()
        .active(settings.automatic_project_save)
        .valign(Align::Center)
        .build();
    automatic_save.add_css_class("settings-switch");
    mark_clickable(&automatic_save);
    automatic_save.connect_active_notify(|toggle| {
        set_automatic_project_save(toggle.is_active());
    });
    let projects = settings_group();
    projects.append(&settings_row(
        "Save changes automatically",
        "Save edits after a short delay once the project has a file location.",
        &automatic_save,
    ));
    page.append(&projects);

    let scroller = ScrolledWindow::builder()
        .hscrollbar_policy(PolicyType::Never)
        .vscrollbar_policy(PolicyType::Automatic)
        .child(&page)
        .build();
    scroller.set_hexpand(true);
    scroller.set_vexpand(true);
    widget_ptr(scroller)
}

fn settings_group() -> GtkBox {
    let group = GtkBox::new(Orientation::Vertical, 0);
    group.add_css_class("settings-group");
    group
}

fn settings_label(title: &str, description: &str) -> GtkBox {
    let label = GtkBox::new(Orientation::Vertical, 4);
    label.add_css_class("settings-row");
    let title_label = Label::new(Some(title));
    title_label.set_xalign(0.0);
    title_label.add_css_class("settings-row-title");
    label.append(&title_label);
    let description_label = Label::new(Some(description));
    description_label.set_xalign(0.0);
    description_label.set_wrap(true);
    description_label.add_css_class("settings-row-description");
    label.append(&description_label);
    label
}

fn settings_row<C: IsA<gtk::Widget>>(title: &str, description: &str, control: &C) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 24);
    row.add_css_class("settings-row");

    let copy = GtkBox::new(Orientation::Vertical, 4);
    copy.set_hexpand(true);
    let title_label = Label::new(Some(title));
    title_label.set_xalign(0.0);
    title_label.add_css_class("settings-row-title");
    copy.append(&title_label);
    let description_label = Label::new(Some(description));
    description_label.set_xalign(0.0);
    description_label.set_wrap(true);
    description_label.add_css_class("settings-row-description");
    copy.append(&description_label);
    row.append(&copy);

    control.set_halign(Align::End);
    control.set_valign(Align::Center);
    row.append(control);
    row
}

fn palette_option(
    title: &str,
    description: &str,
    colors: &[&str],
    group: Option<&CheckButton>,
    palette: ColorPalettePreference,
) -> (GtkBox, CheckButton) {
    let option = GtkBox::new(Orientation::Horizontal, 12);
    option.add_css_class("settings-palette-option");

    let radio = CheckButton::new();
    radio.set_group(group);
    radio.set_valign(Align::Center);
    mark_clickable(&radio);
    radio.connect_toggled(move |radio| {
        if radio.is_active() {
            set_color_palette(palette);
        }
    });
    option.append(&radio);

    let copy = GtkBox::new(Orientation::Vertical, 3);
    copy.set_hexpand(true);
    let title_label = Label::new(Some(title));
    title_label.set_xalign(0.0);
    title_label.add_css_class("settings-row-title");
    copy.append(&title_label);
    let description_label = Label::new(Some(description));
    description_label.set_xalign(0.0);
    description_label.add_css_class("settings-row-description");
    copy.append(&description_label);
    option.append(&copy);

    let preview = GtkBox::new(Orientation::Horizontal, 0);
    preview.add_css_class("settings-palette-preview");
    for color in colors {
        let swatch = DrawingArea::new();
        swatch.set_content_width(30);
        swatch.set_content_height(24);
        let rgb = parse_hex_color(color);
        swatch.set_draw_func(move |_, context, width, height| {
            context.set_source_rgb(rgb.0, rgb.1, rgb.2);
            context.rectangle(0.0, 0.0, f64::from(width), f64::from(height));
            let _ = context.fill();
        });
        preview.append(&swatch);
    }
    option.append(&preview);

    let click = gtk::GestureClick::new();
    let radio_for_click = radio.clone();
    click.connect_released(move |_, _, _, _| radio_for_click.set_active(true));
    option.add_controller(click);
    mark_clickable(&option);
    (option, radio)
}

fn parse_hex_color(color: &str) -> (f64, f64, f64) {
    let value = u32::from_str_radix(color.trim_start_matches('#'), 16).unwrap_or_default();
    (
        f64::from((value >> 16) as u8) / 255.0,
        f64::from((value >> 8) as u8) / 255.0,
        f64::from(value as u8) / 255.0,
    )
}

fn widget_ptr<W: IsA<gtk::Widget>>(widget: W) -> *mut c_void {
    unsafe {
        <gtk::Widget as IntoGlibPtr<*mut gtk::ffi::GtkWidget>>::into_glib_ptr(widget.upcast())
            as *mut c_void
    }
}

fn remember_view(
    storage: &'static std::thread::LocalKey<RefCell<Vec<gtk::glib::WeakRef<GtkBox>>>>,
    root: &GtkBox,
) {
    let weak = gtk::glib::WeakRef::new();
    weak.set(Some(root));
    storage.with(|views| views.borrow_mut().push(weak));
}

fn refresh_views() {
    refresh_view_group(&SYSTEM_VIEWS, render_system_into);
    refresh_workbench_views();
}

fn refresh_workbench_views() {
    refresh_view_group(&ESTIMATE_VIEWS, render_estimate_into);
    refresh_view_group(&SIMULATION_VIEWS, render_simulation_into);
    refresh_detail_views();
    refresh_save_action_enabled();
}

fn refresh_detail_views() {
    refresh_view_group(&DETAIL_VIEWS, render_details_into);
}

fn refresh_save_action_enabled() {
    let enabled = has_dirty_project();
    let Some(window) =
        active_window().and_then(|window| window.downcast::<gtk::ApplicationWindow>().ok())
    else {
        return;
    };
    for action_id in SAVE_ACTION_IDS {
        if let Some(action) = window
            .lookup_action(action_id)
            .and_then(|action| action.downcast::<gtk::gio::SimpleAction>().ok())
        {
            action.set_enabled(enabled);
        }
    }
}

fn refresh_view_group(
    storage: &'static std::thread::LocalKey<RefCell<Vec<gtk::glib::WeakRef<GtkBox>>>>,
    render: fn(&GtkBox),
) {
    storage.with(|views| {
        views.borrow_mut().retain(|weak| {
            if let Some(root) = weak.upgrade() {
                render(&root);
                true
            } else {
                false
            }
        });
    });
}

fn render_launcher_into(root: &GtkBox) {
    clear_box(root);
    let content = GtkBox::new(Orientation::Vertical, 8);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);
    append_no_project_state(
        &content,
        "No project open",
        "Create a new project or open an existing .pvproj file.",
    );
    root.append(&content);
}

fn render_system_into(root: &GtkBox) {
    clear_box(root);
    let state = snapshot();
    let scroller = ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .hscrollbar_policy(PolicyType::Automatic)
        .vscrollbar_policy(PolicyType::Automatic)
        .build();
    let content = GtkBox::new(Orientation::Vertical, 8);
    content.set_margin_top(10);
    content.set_margin_bottom(10);
    content.set_margin_start(10);
    content.set_margin_end(10);
    if let Some(project) = &state.project {
        append_scenario_switcher(&content, project);
        content.append(&section_separator());
        append_location_fields(&content, &project.inputs.estimate_request);
        content.append(&section_separator());
        append_array_fields(&content, project);
        content.append(&section_separator());
        append_consumption_fields(&content, &project.inputs);
    } else {
        append_no_project_state(
            &content,
            "No project open",
            "Create a new project or open an existing .pvproj file.",
        );
    }
    scroller.set_child(Some(&content));
    root.append(&scroller);
}

fn append_no_project_state(content: &GtkBox, title: &str, message: &str) {
    content.set_valign(Align::Center);
    content.append(&header_label(title));
    content.append(&body_label(message));

    let actions = GtkBox::new(Orientation::Horizontal, 6);
    actions.set_halign(Align::Start);
    let new_project = Button::with_label("New Project");
    apply_button_role(&new_project, "primary");
    new_project.connect_clicked(|_| create_new_project());
    actions.append(&new_project);
    let open_project = Button::with_label("Open Project");
    apply_button_role(&open_project, "secondary");
    open_project.connect_clicked(|_| show_open_project_dialog());
    actions.append(&open_project);
    content.append(&actions);
}

fn append_scenario_switcher(content: &GtkBox, project: &PvProjectDocument) {
    let row = GtkBox::new(Orientation::Horizontal, 6);
    row.set_hexpand(true);

    let setup_name = project.title_for_window();
    let dropdown = DropDown::from_strings(&[setup_name.as_str()]);
    dropdown.set_hexpand(true);
    dropdown.set_halign(Align::Fill);
    dropdown.set_selected(0);
    dropdown.set_tooltip_text(Some("Active setup"));
    row.append(&dropdown);

    let add = icon_button("list-add-symbolic", "Add setup");
    add.connect_clicked(|_| {
        append_log("Setup creation is not wired yet".to_string());
        refresh_views();
    });
    row.append(&add);

    let more = icon_button("open-menu-symbolic", "Setup actions");
    let menu = Popover::new();
    menu.set_has_arrow(false);
    menu.set_parent(&more);
    let menu_content = GtkBox::new(Orientation::Vertical, 0);
    menu_content.set_margin_top(6);
    menu_content.set_margin_bottom(6);
    menu_content.set_margin_start(6);
    menu_content.set_margin_end(6);
    let rename = Button::with_label("Rename setup");
    apply_button_role(&rename, "secondary");
    rename.set_halign(Align::Fill);
    let menu_for_rename = menu.clone();
    rename.connect_clicked(move |_| {
        menu_for_rename.popdown();
        show_rename_setup_dialog();
    });
    menu_content.append(&rename);
    menu.set_child(Some(&menu_content));
    let menu_for_more = menu.clone();
    more.connect_clicked(move |_| menu_for_more.popup());
    row.append(&more);

    content.append(&row);
}

fn show_rename_setup_dialog() {
    if !gtk::is_initialized_main_thread() && gtk::init().is_err() {
        append_log("GTK is not initialized; cannot rename setup".to_string());
        return;
    }

    let Some(current_name) = STATE.with(|state| {
        state
            .borrow()
            .project
            .as_ref()
            .map(PvProjectDocument::title_for_window)
    }) else {
        append_log("Open or create a project first".to_string());
        refresh_views();
        return;
    };
    let window = Window::builder()
        .title("Rename Setup")
        .modal(true)
        .default_width(360)
        .build();
    window.add_css_class("app-dialog");

    let content = GtkBox::new(Orientation::Vertical, 12);
    content.set_margin_top(14);
    content.set_margin_bottom(8);
    content.set_margin_start(14);
    content.set_margin_end(14);

    let name = field_entry();
    name.set_text(&current_name);
    name.set_placeholder_text(Some("Setup name"));
    content.append(&field_row("Name", &name));

    let footer = GtkBox::new(Orientation::Horizontal, 6);
    footer.set_halign(Align::End);
    footer.set_margin_bottom(0);
    let cancel = Button::with_label("Cancel");
    apply_button_role(&cancel, "secondary");
    let window_for_cancel = window.clone();
    cancel.connect_clicked(move |_| window_for_cancel.close());
    let save = Button::with_label("Save");
    apply_button_role(&save, "primary");
    let window_for_save = window.clone();
    let name_for_save = name.clone();
    save.connect_clicked(move |_| {
        if rename_setup_from_entry(&name_for_save) {
            window_for_save.close();
        }
    });
    footer.append(&cancel);
    footer.append(&save);
    content.append(&footer);

    window.set_child(Some(&content));
    window.present();
    name.grab_focus();
}

fn append_location_fields(content: &GtkBox, request: &EstimateRequest) {
    content.append(&section_label("Location"));
    let name = Button::with_label(&request.name);
    apply_button_role(&name, "secondary");
    name.set_halign(Align::Fill);
    name.connect_clicked(|_| show_location_search_dialog());
    content.append(&field_row("Name", &name));
    let lat = number_entry(request.latitude, 4, |value| {
        update_input_state(|state| {
            if let Some(project) = state.project.as_mut() {
                project.inputs.estimate_request.latitude = value;
            }
        });
    });
    content.append(&field_row("Latitude", &lat));
    let lon = number_entry(request.longitude, 4, |value| {
        update_input_state(|state| {
            if let Some(project) = state.project.as_mut() {
                project.inputs.estimate_request.longitude = value;
            }
        });
    });
    content.append(&field_row("Longitude", &lon));
}

fn show_location_search_dialog() {
    if !gtk::is_initialized_main_thread() && gtk::init().is_err() {
        append_log("GTK is not initialized; cannot search locations".to_string());
        return;
    }

    let Some(current_query) = STATE.with(|state| {
        state
            .borrow()
            .project
            .as_ref()
            .map(|project| project.inputs.estimate_request.name.clone())
    }) else {
        append_log("Open or create a project first".to_string());
        refresh_views();
        return;
    };
    let window = Window::builder()
        .title("Search Location")
        .modal(true)
        .default_width(460)
        .default_height(520)
        .build();
    window.add_css_class("app-dialog");

    let content = GtkBox::new(Orientation::Vertical, 10);
    content.set_margin_top(14);
    content.set_margin_bottom(8);
    content.set_margin_start(14);
    content.set_margin_end(14);

    let search = search_entry();
    search.set_placeholder_text(Some("Search city"));
    search.set_text(&current_query);
    content.append(&search);

    let list = ListBox::new();
    list.set_selection_mode(SelectionMode::None);
    let scroller = ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .hscrollbar_policy(PolicyType::Automatic)
        .vscrollbar_policy(PolicyType::Automatic)
        .child(&list)
        .build();
    content.append(&scroller);

    let footer = GtkBox::new(Orientation::Horizontal, 6);
    footer.set_halign(Align::End);
    footer.set_margin_bottom(0);
    let cancel = Button::with_label("Cancel");
    apply_button_role(&cancel, "secondary");
    let window_for_cancel = window.clone();
    cancel.connect_clicked(move |_| window_for_cancel.close());
    footer.append(&cancel);
    content.append(&footer);

    window.set_child(Some(&content));
    refresh_location_results(&list, &window, &current_query);

    let list_for_search = list.clone();
    let window_for_search = window.clone();
    search.connect_changed(move |entry| {
        refresh_location_results(&list_for_search, &window_for_search, entry.text().as_str());
    });

    window.present();
    search.grab_focus();
}

fn refresh_location_results(list: &ListBox, window: &Window, query: &str) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }

    let query = query.trim();
    if query.len() < 2 {
        list.append(&meta_label("Type at least 2 characters."));
        return;
    }

    let results = search_cities(query, 12);
    if results.is_empty() {
        list.append(&meta_label("No matching locations."));
        return;
    }

    for result in results {
        list.append(&location_result_button(window, result));
    }
}

fn location_result_button(window: &Window, result: CitySearchResult) -> Button {
    let button = Button::new();
    apply_button_role(&button, "secondary");
    button.set_halign(Align::Fill);

    let row = GtkBox::new(Orientation::Vertical, 2);
    row.set_halign(Align::Fill);
    row.set_hexpand(true);

    let title = Label::new(Some(&format!(
        "{}, {}",
        result.display_name, result.country_code
    )));
    title.set_xalign(0.0);
    title.set_hexpand(true);

    let detail = Label::new(Some(&format!(
        "{:.4}, {:.4} | population {}",
        result.latitude_degrees, result.longitude_degrees, result.population
    )));
    detail.set_xalign(0.0);
    apply_text_role(&detail, "meta");

    row.append(&title);
    row.append(&detail);
    button.set_child(Some(&row));

    let window = window.clone();
    button.connect_clicked(move |_| {
        apply_location_result(&result);
        window.close();
    });
    button
}

fn apply_location_result(result: &CitySearchResult) {
    update_state(|state| {
        let Some(project) = state.project.as_mut() else {
            return;
        };
        let request = &mut project.inputs.estimate_request;
        request.name = result.display_name.clone();
        request.region = result.country_code.clone();
        request.latitude = result.latitude_degrees;
        request.longitude = result.longitude_degrees;
    });
}

fn append_array_fields(content: &GtkBox, project: &PvProjectDocument) {
    let arrays = &project.inputs.arrays;
    let header = GtkBox::new(Orientation::Horizontal, 8);
    header.set_hexpand(true);
    let title = section_label("Production");
    title.set_hexpand(true);
    let add = icon_button("list-add-symbolic", "Add array");
    add.connect_clicked(|_| show_array_dialog(None));
    header.append(&title);
    header.append(&add);
    content.append(&header);

    let table = Grid::new();
    table.set_column_spacing(12);
    table.set_row_spacing(6);
    table.set_hexpand(true);
    table.set_column_homogeneous(false);
    add_table_header(&table, 1, "kWp", 1.0);
    add_table_header(&table, 2, "Tilt", 1.0);
    add_table_header(&table, 3, "Azimuth", 1.0);

    for (index, array) in arrays.iter().enumerate() {
        let row = (index + 1) as i32;
        add_table_cell(
            &table,
            0,
            row,
            array.name.as_deref().unwrap_or("Array"),
            0.0,
            true,
        );
        add_table_cell(
            &table,
            1,
            row,
            &format!("{:.2}", array.peak_power_kwp),
            1.0,
            false,
        );
        add_table_cell(
            &table,
            2,
            row,
            &format!("{:.1}", array.tilt_deg),
            1.0,
            false,
        );
        add_table_cell(
            &table,
            3,
            row,
            &format!(
                "{:.1} {}",
                array.azimuth_deg,
                azimuth_direction_label(array.azimuth_deg)
            ),
            1.0,
            false,
        );

        let actions = GtkBox::new(Orientation::Horizontal, 4);
        actions.set_halign(Align::End);
        actions.set_hexpand(true);
        let edit = icon_button("document-edit-symbolic", "Edit array");
        edit.connect_clicked(move |_| show_array_dialog(Some(index)));
        let delete = icon_button("edit-delete-symbolic", "Delete array");
        delete.connect_clicked(move |_| confirm_delete_array(index));
        actions.append(&edit);
        actions.append(&delete);
        table.attach(&actions, 4, row, 1, 1);
    }

    content.append(&table);
    if arrays.is_empty() {
        content.append(&meta_label("No arrays configured."));
    }

    let storage = number_entry(
        project
            .inputs
            .estimate_request
            .storage_usable_kwh
            .unwrap_or(0.0),
        2,
        |value| {
            update_simulation_input_state(|state| {
                if let Some(project) = state.project.as_mut() {
                    project.inputs.estimate_request.storage_usable_kwh =
                        (value > 0.0).then_some(value);
                }
            });
        },
    );
    content.append(&field_row("Storage kWh", &storage));

    let Some(loss) = STATE.with(|state| {
        state
            .borrow()
            .project
            .as_ref()
            .map(|project| project.inputs.estimate_request.loss_pct)
    }) else {
        append_log("Open or create a project first".to_string());
        refresh_views();
        return;
    };
    let loss = number_entry(loss, 1, |value| {
        update_input_state(|state| {
            if let Some(project) = state.project.as_mut() {
                project.inputs.estimate_request.loss_pct = value;
            }
        });
    });
    content.append(&field_row("Loss %", &loss));
}

fn show_array_dialog(index: Option<usize>) {
    if !gtk::is_initialized_main_thread() && gtk::init().is_err() {
        append_log("GTK is not initialized; cannot edit production arrays".to_string());
        return;
    }

    let array = index
        .and_then(|index| {
            STATE.with(|state| {
                state
                    .borrow()
                    .project
                    .as_ref()
                    .and_then(|project| project.inputs.arrays.get(index).cloned())
            })
        })
        .unwrap_or_else(default_array);

    let window = Window::builder()
        .title(if index.is_some() {
            "Edit Array"
        } else {
            "Add Array"
        })
        .modal(true)
        .default_width(420)
        .build();
    window.add_css_class("app-dialog");

    let content = GtkBox::new(Orientation::Vertical, 12);
    content.set_margin_top(14);
    content.set_margin_bottom(8);
    content.set_margin_start(14);
    content.set_margin_end(14);

    let name = field_entry();
    name.set_text(array.name.as_deref().unwrap_or(""));
    content.append(&field_row("Name", &name));

    let kwp = dialog_number_entry(array.peak_power_kwp, 2);
    content.append(&field_row("kWp", &kwp));

    let tilt = dialog_number_entry(array.tilt_deg, 1);
    content.append(&field_row("Tilt", &tilt));

    let azimuth = dialog_number_entry(array.azimuth_deg, 1);
    content.append(&azimuth_field_row(&azimuth));

    let footer = GtkBox::new(Orientation::Horizontal, 6);
    footer.set_halign(Align::End);
    footer.set_margin_bottom(0);
    let cancel = Button::with_label("Cancel");
    apply_button_role(&cancel, "secondary");
    let window_for_cancel = window.clone();
    cancel.connect_clicked(move |_| window_for_cancel.close());
    let save = Button::with_label("Save");
    apply_button_role(&save, "primary");
    let window_for_save = window.clone();
    let name_for_save = name.clone();
    let kwp_for_save = kwp.clone();
    let tilt_for_save = tilt.clone();
    let azimuth_for_save = azimuth.clone();
    save.connect_clicked(move |_| {
        if let Some(array) = read_array_dialog_values(
            &name_for_save,
            &kwp_for_save,
            &tilt_for_save,
            &azimuth_for_save,
        ) {
            save_array(index, array);
            window_for_save.close();
        }
    });
    footer.append(&cancel);
    footer.append(&save);
    content.append(&footer);

    window.set_child(Some(&content));
    window.present();
    name.grab_focus();
}

fn default_array() -> EstimateArray {
    EstimateArray {
        name: Some("New array".to_string()),
        peak_power_kwp: 1.0,
        tilt_deg: 30.0,
        azimuth_deg: 0.0,
    }
}

fn dialog_number_entry(value: f64, digits: u32) -> Entry {
    let entry = field_entry();
    entry.set_input_purpose(gtk::InputPurpose::Number);
    entry.set_text(&format_number(value, digits));
    entry
}

fn read_array_dialog_values(
    name: &Entry,
    kwp: &Entry,
    tilt: &Entry,
    azimuth: &Entry,
) -> Option<EstimateArray> {
    Some(EstimateArray {
        name: non_empty_text(&name.text()),
        peak_power_kwp: parse_number(&kwp.text())?.max(0.0),
        tilt_deg: parse_number(&tilt.text())?,
        azimuth_deg: parse_number(&azimuth.text())?,
    })
}

fn non_empty_text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn save_array(index: Option<usize>, array: EstimateArray) {
    if let Some(index) = index {
        let current = STATE.with(|state| {
            state
                .borrow()
                .project
                .as_ref()
                .and_then(|project| project.inputs.arrays.get(index))
                .cloned()
        });
        if let Some(current) = current {
            if current == array {
                return;
            }
            if !array_edit_is_functional(&current, &array) {
                rename_array(index, array.name);
                return;
            }
        }
    }

    update_state(|state| {
        let Some(project) = state.project.as_mut() else {
            return;
        };
        match index {
            Some(index) if index < project.inputs.arrays.len() => {
                project.inputs.arrays[index] = array;
            }
            _ => project.inputs.arrays.push(array),
        }
        sync_request_from_arrays(state);
    });
}

fn array_edit_is_functional(current: &EstimateArray, updated: &EstimateArray) -> bool {
    current.peak_power_kwp != updated.peak_power_kwp
        || current.tilt_deg != updated.tilt_deg
        || current.azimuth_deg != updated.azimuth_deg
}

fn rename_array(index: usize, name: Option<String>) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(project) = state.project.as_mut() else {
            return;
        };
        let Some(array) = project.inputs.arrays.get_mut(index) else {
            return;
        };
        array.name = name.clone();

        if let Some(reference) = project
            .results
            .estimate
            .as_mut()
            .and_then(|estimate| estimate.references.get_mut("arrays"))
            .and_then(serde_json::Value::as_array_mut)
            .and_then(|arrays| arrays.get_mut(index))
            .and_then(serde_json::Value::as_object_mut)
        {
            if let Some(name) = name {
                reference.insert("name".to_string(), serde_json::Value::String(name));
            } else {
                reference.remove("name");
            }
        }

        state.dirty = true;
        state.status = "Array renamed".to_string();
    });
    schedule_automatic_save();
    refresh_views();
}

fn confirm_delete_array(index: usize) {
    if !gtk::is_initialized_main_thread() && gtk::init().is_err() {
        append_log("GTK is not initialized; cannot confirm array deletion".to_string());
        return;
    }

    let array_name = STATE.with(|state| {
        state
            .borrow()
            .project
            .as_ref()
            .and_then(|project| project.inputs.arrays.get(index))
            .and_then(|array| array.name.clone())
            .unwrap_or_else(|| "this array".to_string())
    });

    let window = Window::builder()
        .title("Delete Array")
        .modal(true)
        .default_width(360)
        .build();
    window.add_css_class("app-dialog");

    let content = GtkBox::new(Orientation::Vertical, 12);
    content.set_margin_top(14);
    content.set_margin_bottom(8);
    content.set_margin_start(14);
    content.set_margin_end(14);

    content.append(&body_label(&format!(
        "Delete {array_name}? This cannot be undone."
    )));

    let footer = GtkBox::new(Orientation::Horizontal, 6);
    footer.set_halign(Align::End);
    footer.set_margin_bottom(0);
    let cancel = Button::with_label("Cancel");
    apply_button_role(&cancel, "secondary");
    let window_for_cancel = window.clone();
    cancel.connect_clicked(move |_| window_for_cancel.close());
    let delete = Button::with_label("Delete");
    apply_button_role(&delete, "danger");
    let window_for_delete = window.clone();
    delete.connect_clicked(move |_| {
        delete_array(index);
        window_for_delete.close();
    });
    footer.append(&cancel);
    footer.append(&delete);
    content.append(&footer);

    window.set_child(Some(&content));
    window.present();
}

fn delete_array(index: usize) {
    update_state(|state| {
        let Some(project) = state.project.as_mut() else {
            return;
        };
        if index >= project.inputs.arrays.len() {
            return;
        }
        project.inputs.arrays.remove(index);
        sync_request_from_arrays(state);
    });
}

fn sync_request_from_arrays(state: &mut DesktopState) {
    let Some(project) = state.project.as_mut() else {
        return;
    };
    if let Some(first) = project.inputs.arrays.first() {
        project.inputs.estimate_request.peak_power_kwp = first.peak_power_kwp;
        project.inputs.estimate_request.tilt_deg = first.tilt_deg;
        project.inputs.estimate_request.azimuth_deg = first.azimuth_deg;
    }
}

fn append_consumption_fields(content: &GtkBox, inputs: &pv_desktop_core::ProjectInputs) {
    content.append(&section_label("Consumption"));
    let load_profile = &inputs.load_profile;
    let annual = match load_profile {
        LoadProfile::AnnualKwh { annual_kwh, .. } => *annual_kwh,
        LoadProfile::DailyKwh { daily_kwh, .. } => *daily_kwh * 365.0,
    };
    let annual_entry = number_entry(annual, 0, |value| {
        update_simulation_input_state(|state| {
            if let Some(project) = state.project.as_mut() {
                let shape = load_shape(&project.inputs.load_profile);
                project.inputs.load_profile = LoadProfile::AnnualKwh {
                    annual_kwh: value,
                    shape,
                };
            }
        });
    });
    content.append(&field_row("Annual kWh", &annual_entry));
    let price = optional_number_entry(inputs.energy_price_eur_per_kwh, 3, set_energy_price);
    content.append(&field_row("EUR/kWh", &price));
    let shape = load_shape(load_profile);
    let dropdown = DropDown::from_strings(&["Residential", "Flat", "Daytime", "Evening"]);
    dropdown.set_selected(shape_index(&shape));
    dropdown.connect_selected_notify(|dropdown| {
        let next_shape = LoadShape::BuiltIn {
            shape_id: shape_from_index(dropdown.selected()),
        };
        update_simulation_state(|state| {
            if let Some(project) = state.project.as_mut() {
                let annual_kwh = match project.inputs.load_profile {
                    LoadProfile::AnnualKwh { annual_kwh, .. } => annual_kwh,
                    LoadProfile::DailyKwh { daily_kwh, .. } => daily_kwh * 365.0,
                };
                project.inputs.load_profile = LoadProfile::AnnualKwh {
                    annual_kwh,
                    shape: next_shape,
                };
            }
        });
    });
    content.append(&field_row("Shape", &dropdown));
}

fn render_estimate_into(root: &GtkBox) {
    clear_box(root);
    let state = snapshot();
    let scroller = workbench_scroller();
    let content = workbench_content();
    if let Some(project) = &state.project {
        if project.results.estimate.is_some() {
            append_estimate_result(&content, project);
        } else {
            append_estimate_empty_state(&content, &state);
        }
    } else {
        append_no_project_state(
            &content,
            "No project open",
            "Open a project to view production estimates.",
        );
    }
    scroller.set_child(Some(&content));
    root.append(&scroller);
}

fn append_estimate_empty_state(content: &GtkBox, state: &DesktopState) {
    let summary = Grid::new();
    summary.set_column_spacing(18);
    summary.set_row_spacing(8);
    summary.set_hexpand(true);
    add_estimate_metric_row(&summary, 0, "Annual kWh", "-", EstimateTone::Strong);
    content.append(&summary);
    content.append(&body_label(computation_empty_message(
        state,
        ComputationStage::Estimate,
    )));
    append_retry_action(content, state, ComputationStage::Estimate);
    append_manual_computation_action(content, state, ComputationStage::Estimate);
}

fn append_estimate_result(content: &GtkBox, project: &PvProjectDocument) {
    let Some(document) = &project.results.estimate else {
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

    let summary = Grid::new();
    summary.set_column_spacing(18);
    summary.set_row_spacing(8);
    summary.set_hexpand(true);
    add_estimate_metric_row(
        &summary,
        0,
        "Annual kWh",
        &annual_energy_value(document),
        EstimateTone::Strong,
    );
    let mut row = 1;
    if let Some(price) = project.inputs.energy_price_eur_per_kwh {
        add_estimate_metric_row(
            &summary,
            row,
            "Revenue €",
            &annual_revenue_value(document, price),
            EstimateTone::Strong,
        );
        row += 1;
    }
    add_estimate_metric_row(
        &summary,
        row,
        "POA",
        &format!(
            "{:.2} kWh/m2",
            estimate
                .annual_in_plane_irradiation
                .mean
                .as_kilowatt_hours_per_square_meter()
        ),
        EstimateTone::Normal,
    );
    row += 1;
    add_estimate_metric_row(&summary, row, "Sources", &sources, EstimateTone::Normal);
    content.append(&summary);

    let rows = monthly_estimate_rows(estimate);
    content.append(&estimate_monthly_table(&rows));
}

fn render_details_into(root: &GtkBox) {
    clear_box(root);
    let state = snapshot();
    let context = ACTIVE_CONTEXT.with(|slot| slot.borrow().clone());
    let scroller = workbench_scroller();
    let content = details_content();

    let initial_view_id = INITIAL_DETAILS_VIEW.with(|slot| *slot.borrow());
    let view_id = context
        .as_ref()
        .and_then(|event| event.current.plugin_view_id.as_deref())
        .or(initial_view_id);
    let tab_id = context.as_ref().map(|event| event.current.tab_id.as_str());

    match (view_id, tab_id) {
        (Some(VIEW_SIMULATION), _) | (_, Some("simulation")) => {
            append_simulation_details(&content, &state);
        }
        (Some(VIEW_ESTIMATE), _) | (_, Some("estimate")) => {
            append_estimate_details(&content, &state);
        }
        _ => append_estimate_details(&content, &state),
    }

    scroller.set_child(Some(&content));
    root.append(&scroller);
}

fn append_estimate_details(content: &GtkBox, state: &DesktopState) {
    content.append(&section_label("Estimate"));
    let Some(project) = &state.project else {
        content.append(&body_label("No project open."));
        append_project_actions(content);
        return;
    };

    let Some(document) = &project.results.estimate else {
        content.append(&body_label(computation_empty_message(
            state,
            ComputationStage::Estimate,
        )));
        append_retry_action(content, state, ComputationStage::Estimate);
        return;
    };

    content.append(&section_label("Result"));
    append_detail_row(content, "Annual", &annual_energy_value(document));
    if let Some(price) = project.inputs.energy_price_eur_per_kwh {
        append_detail_row(content, "Revenue", &annual_revenue_value(document, price));
    }
    append_detail_row(
        content,
        "POA",
        &format!(
            "{:.2} kWh/m2",
            document
                .ensemble_estimate
                .annual_in_plane_irradiation
                .mean
                .as_kilowatt_hours_per_square_meter()
        ),
    );

    content.append(&section_separator());
    content.append(&section_label("Quality"));
    append_detail_row(content, "Sources", &estimate_sources_label(document));
    append_detail_row(content, "Band", &estimate_uncertainty_label(document));
    append_detail_row(content, "Spread", &estimate_source_spread_label(document));
    append_detail_row(content, "Coverage", estimate_coverage_label(document));

    content.append(&section_separator());
    content.append(&section_label("Highlights"));
    if let Some(best) = estimate_month_highlight(&document.ensemble_estimate, MonthRank::Best) {
        append_detail_row(content, "Best month", &best);
    }
    if let Some(worst) = estimate_month_highlight(&document.ensemble_estimate, MonthRank::Worst) {
        append_detail_row(content, "Lowest month", &worst);
    }
    append_detail_note(
        content,
        &estimate_seasonality_label(&document.ensemble_estimate),
    );

    content.append(&section_separator());
    content.append(&section_label("Diagnostics"));
    for note in estimate_diagnostics(project, document) {
        append_detail_note(content, &note);
    }
}

fn append_simulation_details(content: &GtkBox, state: &DesktopState) {
    let Some(project) = &state.project else {
        content.append(&body_label("No project open."));
        append_project_actions(content);
        return;
    };

    if let Some(run) = simulation_run_snapshot() {
        append_detail_row(
            content,
            "Status",
            if run.cancelling {
                "cancelling"
            } else {
                "running"
            },
        );
        append_detail_row(
            content,
            "Runs",
            &format!(
                "{} / {}",
                format_runs(run.completed_runs),
                format_runs(run.requested_runs)
            ),
        );
        append_detail_row(
            content,
            "Started",
            &format_system_time(run.started_wall_time),
        );
        append_detail_row(
            content,
            "Elapsed",
            &format_duration(run.started_at.elapsed()),
        );
        let cancel = Button::with_label(simulation_cancel_label(run.cancelling));
        apply_button_role(&cancel, "secondary");
        cancel.set_sensitive(!run.cancelling);
        cancel.connect_clicked(|_| cancel_simulation_run());
        content.append(&cancel);
        return;
    }

    let Some(result) = &project.results.simulation else {
        content.append(&body_label(computation_empty_message(
            state,
            ComputationStage::Simulation,
        )));
        content.append(&section_separator());
        content.append(&section_label("Diagnostics"));
        for note in missing_simulation_diagnostics(project) {
            append_detail_note(content, &note);
        }
        append_retry_action(content, state, ComputationStage::Simulation);
        return;
    };

    append_detail_row(
        content,
        "Status",
        if result.cancelled {
            "cancelled"
        } else {
            "complete"
        },
    );
    append_detail_row(
        content,
        "Runs",
        &format!(
            "{} / {}",
            format_runs(result.completed_runs),
            format_runs(result.requested_runs)
        ),
    );
    if let Some(metadata) = &project.results.simulation_metadata {
        append_detail_row(content, "Started", &metadata.started_at);
        append_detail_row(content, "Completed", &metadata.completed_at);
        append_detail_row(
            content,
            "Elapsed",
            &format_duration(Duration::from_millis(metadata.elapsed_ms)),
        );
    } else {
        append_detail_note(content, "Timing metadata is not available for this result.");
    }
}

#[derive(Clone, Copy)]
enum MonthRank {
    Best,
    Worst,
}

fn estimate_sources_label(document: &SourceEnsembleEstimateDocument) -> String {
    if document.coverage.applicable_sources.is_empty() {
        "none".to_string()
    } else {
        document
            .coverage
            .applicable_sources
            .iter()
            .map(|source| source.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

fn estimate_uncertainty_label(document: &SourceEnsembleEstimateDocument) -> String {
    let estimate = &document.ensemble_estimate;
    match estimate.uncertainty.annual_energy {
        Some(band) => format!(
            "{:.0}..{:.0} kWh, +/- {:.0} kWh",
            band.low.as_kilowatt_hours().round(),
            band.high.as_kilowatt_hours().round(),
            band.half_width.as_kilowatt_hours().round()
        ),
        None => "not calibrated; only one source contributed".to_string(),
    }
}

fn estimate_source_spread_label(document: &SourceEnsembleEstimateDocument) -> String {
    let source_values = document
        .ensemble_estimate
        .source_estimates
        .iter()
        .map(|estimate| estimate.annual_energy.as_kilowatt_hours())
        .collect::<Vec<_>>();
    if source_values.len() < 2 {
        return "single source".to_string();
    }
    let min = source_values.iter().copied().fold(f64::INFINITY, f64::min);
    let max = source_values
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    let mean = document
        .ensemble_estimate
        .annual_energy
        .mean
        .as_kilowatt_hours();
    let spread_pct = if mean.abs() > f64::EPSILON {
        ((max - min) / mean) * 100.0
    } else {
        0.0
    };
    format!(
        "{:.0}..{:.0} kWh ({spread_pct:.0}%)",
        min.round(),
        max.round()
    )
}

fn estimate_coverage_label(document: &SourceEnsembleEstimateDocument) -> &'static str {
    if document.coverage.pvgis_sarah3_applicable {
        "PVGIS SARAH3 available"
    } else {
        "PVGIS SARAH3 outside coverage"
    }
}

fn estimate_month_highlight(
    estimate: &pv_core::source_model::AnnualPvEnsembleEstimate,
    rank: MonthRank,
) -> Option<String> {
    let monthly = match rank {
        MonthRank::Best => estimate.monthly_estimates.iter().max_by(|left, right| {
            left.energy
                .mean
                .as_kilowatt_hours()
                .total_cmp(&right.energy.mean.as_kilowatt_hours())
        }),
        MonthRank::Worst => estimate.monthly_estimates.iter().min_by(|left, right| {
            left.energy
                .mean
                .as_kilowatt_hours()
                .total_cmp(&right.energy.mean.as_kilowatt_hours())
        }),
    }?;
    let month = short_month_name(monthly.month.value()).unwrap_or("?");
    Some(format!(
        "{} {:.0} kWh",
        month,
        monthly.energy.mean.as_kilowatt_hours().round()
    ))
}

fn estimate_seasonality_label(
    estimate: &pv_core::source_model::AnnualPvEnsembleEstimate,
) -> String {
    let Some(best) = estimate.monthly_estimates.iter().max_by(|left, right| {
        left.energy
            .mean
            .as_kilowatt_hours()
            .total_cmp(&right.energy.mean.as_kilowatt_hours())
    }) else {
        return "No monthly estimate rows are available.".to_string();
    };
    let Some(worst) = estimate.monthly_estimates.iter().min_by(|left, right| {
        left.energy
            .mean
            .as_kilowatt_hours()
            .total_cmp(&right.energy.mean.as_kilowatt_hours())
    }) else {
        return "No monthly estimate rows are available.".to_string();
    };
    let worst_kwh = worst.energy.mean.as_kilowatt_hours();
    if worst_kwh <= f64::EPSILON {
        return "Seasonality cannot be compared because the lowest month is near zero.".to_string();
    }
    format!(
        "Best month produces {:.1}x the lowest month.",
        best.energy.mean.as_kilowatt_hours() / worst_kwh
    )
}

fn estimate_diagnostics(
    project: &PvProjectDocument,
    document: &SourceEnsembleEstimateDocument,
) -> Vec<String> {
    let mut diagnostics = Vec::new();
    let uncertainty = &document.ensemble_estimate.uncertainty;
    if uncertainty.calibrated {
        diagnostics.push(format!(
            "Uncertainty band is calibrated from {} sources.",
            uncertainty.source_count
        ));
    } else {
        diagnostics
            .push("Uncertainty is limited because fewer than two sources contributed.".to_string());
    }
    if document.ensemble_estimate.annual_energy.spread_fraction > 0.20 {
        diagnostics
            .push("Source disagreement is high; review monthly spread before sizing.".to_string());
    }
    if project.inputs.energy_price_eur_per_kwh.is_none() {
        diagnostics.push("Revenue is hidden because no energy price is set.".to_string());
    }
    if !document.coverage.pvgis_sarah3_applicable {
        diagnostics.push("SARAH3 coverage is unavailable at this location; the estimate relies on remaining sources.".to_string());
    }
    if diagnostics.is_empty() {
        diagnostics.push("No estimate quality issues flagged.".to_string());
    }
    diagnostics
}

fn missing_simulation_diagnostics(project: &PvProjectDocument) -> Vec<String> {
    let mut diagnostics = Vec::new();
    if project.results.production_profile.is_none() {
        diagnostics
            .push("The production estimate is being prepared before simulation.".to_string());
    } else if automatic_simulation_updates_enabled() {
        diagnostics.push(
            "Simulation updates automatically when system or consumption inputs change."
                .to_string(),
        );
    } else {
        diagnostics.push("Run the simulation to apply the latest input changes.".to_string());
    }
    if project.inputs.estimate_request.storage_usable_kwh.is_none() {
        diagnostics.push(
            "No battery is configured; simulation will route surplus directly to export."
                .to_string(),
        );
    }
    diagnostics
}

fn computation_empty_message(state: &DesktopState, stage: ComputationStage) -> &'static str {
    if let Some(failed_stage) = state.retry_stage {
        return match (failed_stage, stage) {
            (ComputationStage::Estimate, ComputationStage::Estimate) => "Estimate update failed.",
            (ComputationStage::Estimate, ComputationStage::Simulation) => {
                "Estimate update failed; simulation is waiting."
            }
            (ComputationStage::Simulation, ComputationStage::Simulation) => {
                "Simulation stopped before completion."
            }
            (ComputationStage::Simulation, ComputationStage::Estimate) => {
                "Estimate is unavailable."
            }
        };
    }
    if !automatic_simulation_updates_enabled() {
        return match stage {
            ComputationStage::Estimate => "Estimate update required.",
            ComputationStage::Simulation => "Simulation update required.",
        };
    }
    match (state.computation_phase, stage) {
        (ComputationPhase::Debouncing, _) => "Waiting for input changes to settle…",
        (ComputationPhase::Estimating, ComputationStage::Estimate) => "Updating estimate…",
        (ComputationPhase::Estimating, ComputationStage::Simulation) => {
            "Updating estimate before simulation…"
        }
        (ComputationPhase::WaitingForSimulation, ComputationStage::Simulation) => {
            "Waiting for the previous simulation to stop…"
        }
        (_, ComputationStage::Estimate) => "Preparing estimate…",
        (_, ComputationStage::Simulation) => "Preparing simulation…",
    }
}

fn append_retry_action(content: &GtkBox, state: &DesktopState, stage: ComputationStage) {
    let Some(retry_stage) = state.retry_stage else {
        return;
    };
    if retry_stage != stage && stage != ComputationStage::Simulation {
        return;
    }
    let retry = Button::with_label("Retry");
    apply_button_role(&retry, "primary");
    retry.connect_clicked(move |_| retry_automatic_computation(retry_stage));
    content.append(&retry);
}

fn append_manual_computation_action(
    content: &GtkBox,
    state: &DesktopState,
    stage: ComputationStage,
) {
    if automatic_simulation_updates_enabled() || state.retry_stage.is_some() {
        return;
    }
    let (label, impact) = match stage {
        ComputationStage::Estimate => ("Update estimate", ComputationImpact::EstimateAndSimulation),
        ComputationStage::Simulation => ("Run simulation", ComputationImpact::Simulation),
    };
    let run = Button::with_label(label);
    apply_button_role(&run, "primary");
    mark_clickable(&run);
    run.connect_clicked(move |_| {
        schedule_automatic_computation(impact, ComputationTrigger::Immediate)
    });
    content.append(&run);
}

fn append_project_actions(content: &GtkBox) {
    let actions = GtkBox::new(Orientation::Horizontal, 6);
    actions.set_halign(Align::Start);
    let new_project = Button::with_label("New Project");
    apply_button_role(&new_project, "primary");
    new_project.connect_clicked(|_| create_new_project());
    actions.append(&new_project);
    let open_project = Button::with_label("Open Project");
    apply_button_role(&open_project, "secondary");
    open_project.connect_clicked(|_| show_open_project_dialog());
    actions.append(&open_project);
    content.append(&actions);
}

fn append_detail_row(content: &GtkBox, label: &str, value: &str) {
    let value = Label::new(Some(value));
    value.set_xalign(0.0);
    value.set_wrap(true);
    apply_text_role(&value, "code");
    content.append(&field_row(label, &value));
}

fn append_detail_note(content: &GtkBox, text: &str) {
    content.append(&meta_label(text));
}

fn render_simulation_into(root: &GtkBox) {
    clear_box(root);
    let state = snapshot();
    let scroller = workbench_scroller();
    let content = workbench_content();
    let Some(project) = &state.project else {
        append_no_project_state(
            &content,
            "No project open",
            "Open a project to run consumption simulations.",
        );
        scroller.set_child(Some(&content));
        root.append(&scroller);
        return;
    };
    if let Some(run) = simulation_run_snapshot() {
        append_simulation_progress(&content, run);
    } else if let Some(result) = &project.results.simulation {
        append_simulation_result(&content, project, result);
    } else {
        append_simulation_empty_state(&content, &state);
    }
    scroller.set_child(Some(&content));
    root.append(&scroller);
}

fn append_simulation_progress(content: &GtkBox, run: SimulationRunSnapshot) {
    let heading = section_label(simulation_progress_heading(run.cancelling));
    content.append(&heading);

    let row = GtkBox::new(Orientation::Horizontal, 12);
    row.set_hexpand(true);

    let progress_column = GtkBox::new(Orientation::Vertical, 6);
    progress_column.set_hexpand(true);
    progress_column.set_halign(Align::Fill);

    let count = meta_label(&simulation_progress_label(run));
    progress_column.append(&count);

    let progress = ProgressBar::new();
    progress.set_hexpand(true);
    progress.set_halign(Align::Fill);
    progress.set_show_text(false);
    progress.set_fraction(simulation_progress_fraction(run));
    progress_column.append(&progress);
    row.append(&progress_column);

    let (cancel, cancel_label) = simulation_cancel_button(run.cancelling);
    cancel.connect_clicked(|_| cancel_simulation_run());
    row.append(&cancel);

    content.append(&row);
    let status = meta_label(&simulation_progress_status(
        run.completed_runs,
        run.requested_runs,
        run.cancelling,
    ));
    content.append(&status);

    remember_simulation_progress_widgets(
        &heading,
        &count,
        &progress,
        &status,
        &cancel,
        &cancel_label,
    );
}

fn remember_simulation_progress_widgets(
    heading: &Label,
    count: &Label,
    progress: &ProgressBar,
    status: &Label,
    cancel: &Button,
    cancel_label: &Label,
) {
    let heading_ref = gtk::glib::WeakRef::new();
    heading_ref.set(Some(heading));
    let count_ref = gtk::glib::WeakRef::new();
    count_ref.set(Some(count));
    let progress_ref = gtk::glib::WeakRef::new();
    progress_ref.set(Some(progress));
    let status_ref = gtk::glib::WeakRef::new();
    status_ref.set(Some(status));
    let cancel_ref = gtk::glib::WeakRef::new();
    cancel_ref.set(Some(cancel));
    let cancel_label_ref = gtk::glib::WeakRef::new();
    cancel_label_ref.set(Some(cancel_label));

    SIMULATION_PROGRESS_VIEWS.with(|views| {
        views.borrow_mut().push(SimulationProgressWidgets {
            heading: heading_ref,
            count: count_ref,
            progress: progress_ref,
            status: status_ref,
            cancel: cancel_ref,
            cancel_label: cancel_label_ref,
        });
    });
}

fn update_simulation_progress_views(run: SimulationRunSnapshot) {
    SIMULATION_PROGRESS_VIEWS.with(|views| {
        views.borrow_mut().retain(|view| {
            let Some(heading) = view.heading.upgrade() else {
                return false;
            };
            let Some(count) = view.count.upgrade() else {
                return false;
            };
            let Some(progress) = view.progress.upgrade() else {
                return false;
            };
            let Some(status) = view.status.upgrade() else {
                return false;
            };
            let Some(cancel) = view.cancel.upgrade() else {
                return false;
            };
            let Some(cancel_label) = view.cancel_label.upgrade() else {
                return false;
            };

            heading.set_text(simulation_progress_heading(run.cancelling));
            count.set_text(&simulation_progress_label(run));
            progress.set_fraction(simulation_progress_fraction(run));
            status.set_text(&simulation_progress_status(
                run.completed_runs,
                run.requested_runs,
                run.cancelling,
            ));
            cancel.set_sensitive(!run.cancelling);
            cancel_label.set_text(simulation_cancel_label(run.cancelling));
            true
        });
    });
}

fn simulation_progress_heading(cancelling: bool) -> &'static str {
    if cancelling {
        "Cancelling simulation"
    } else {
        "Simulation running"
    }
}

fn simulation_progress_fraction(run: SimulationRunSnapshot) -> f64 {
    if run.requested_runs > 0 {
        (run.completed_runs as f64 / run.requested_runs as f64).min(1.0)
    } else {
        0.0
    }
}

fn simulation_progress_label(run: SimulationRunSnapshot) -> String {
    let percent = simulation_progress_fraction(run) * 100.0;
    format!(
        "{} of {} runs ({percent:.0}%)",
        format_runs(run.completed_runs),
        format_runs(run.requested_runs)
    )
}

fn simulation_cancel_button(cancelling: bool) -> (Button, Label) {
    let button = Button::new();
    apply_button_role(&button, "secondary");
    button.set_tooltip_text(Some("Cancel simulation"));
    button.set_valign(Align::End);
    button.set_sensitive(!cancelling);

    let content = GtkBox::new(Orientation::Horizontal, 8);
    content.set_margin_top(7);
    content.set_margin_bottom(7);
    content.set_margin_start(12);
    content.set_margin_end(14);
    let icon = Image::from_icon_name("process-stop-symbolic");
    icon.set_icon_size(gtk::IconSize::Normal);
    content.append(&icon);
    let label = Label::new(Some(simulation_cancel_label(cancelling)));
    content.append(&label);
    button.set_child(Some(&content));
    (button, label)
}

fn simulation_cancel_label(cancelling: bool) -> &'static str {
    if cancelling { "Cancelling" } else { "Cancel" }
}

fn append_simulation_result(
    content: &GtkBox,
    project: &PvProjectDocument,
    result: &SimulationResult,
) {
    content.append(&simulation_summary_table(result));
    content.append(&section_separator());
    content.append(&simulation_scenario_table(result));
    append_simulation_graphs(content, project);
}

fn append_simulation_graphs(content: &GtkBox, project: &PvProjectDocument) {
    let Some((production, load)) = simulation_graph_series(project) else {
        return;
    };
    content.append(&section_separator());
    content.append(&chart_legend());
    content.append(&monthly_simulation_chart(&production, &load));
    content.append(&section_separator());
    append_daily_projection_graph(content, &production, &load);
}

fn simulation_graph_series(project: &PvProjectDocument) -> Option<(Vec<f64>, Vec<f64>)> {
    let production = project
        .results
        .production_profile
        .as_ref()?
        .hourly_mean_kwh
        .clone();
    if production.len() != 8760 {
        return None;
    }
    let load = deterministic_hourly_load_kwh(&project.inputs.load_profile).ok()?;
    if load.len() != 8760 {
        return None;
    }
    Some((production, load))
}

fn chart_legend() -> GtkBox {
    let legend = GtkBox::new(Orientation::Horizontal, 16);
    legend.append(&legend_item("Production", ChartColorRole::Production));
    legend.append(&legend_item("Load", ChartColorRole::Load));
    legend
}

fn legend_item(text: &str, role: ChartColorRole) -> GtkBox {
    let item = GtkBox::new(Orientation::Horizontal, 6);
    let swatch = DrawingArea::new();
    swatch.set_content_width(18);
    swatch.set_content_height(10);
    swatch.set_draw_func(move |swatch, context, width, height| {
        let colors = ChartColors::from_widget(swatch);
        set_source_rgba(context, colors.series(role));
        let y = (f64::from(height) / 2.0 - 1.5).max(0.0);
        context.rectangle(0.0, y, f64::from(width), 3.0);
        let _ = context.fill();
    });
    item.append(&swatch);
    item.append(&Label::new(Some(text)));
    item
}

fn monthly_simulation_chart(production: &[f64], load: &[f64]) -> DrawingArea {
    let production_months = monthly_totals(production);
    let load_months = monthly_totals(load);
    let chart = DrawingArea::new();
    chart.set_content_height(260);
    chart.set_hexpand(true);
    chart.set_draw_func(move |chart, context, width, height| {
        let colors = ChartColors::from_widget(chart);
        draw_monthly_chart(
            context,
            width,
            height,
            &colors,
            &production_months,
            &load_months,
        );
    });
    chart
}

fn append_daily_projection_graph(content: &GtkBox, production: &[f64], load: &[f64]) {
    let date = selected_daily_projection_date();
    content.append(&daily_projection_controls(date));
    let production_day = daily_slice(production, date);
    let load_day = daily_slice(load, date);
    content.append(&daily_projection_chart(production_day, load_day));
}

fn daily_projection_controls(date: DailyProjectionDate) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 10);
    row.set_hexpand(true);

    let month_names = (1..=12)
        .map(|month| short_month_name(month).unwrap_or("?").to_string())
        .collect::<Vec<_>>();
    let month_refs = month_names.iter().map(String::as_str).collect::<Vec<_>>();
    let month = DropDown::from_strings(&month_refs);
    month.set_tooltip_text(Some("Projection month"));
    month.set_selected(u32::from(date.month.saturating_sub(1)));
    month.connect_selected_notify(|dropdown| {
        let month = (dropdown.selected() as u8).saturating_add(1).clamp(1, 12);
        SIMULATION_GRAPH_DATE.with(|selection| {
            let mut selection = selection.borrow_mut();
            selection.month = month;
            selection.day = selection.day.min(calendar_days_in_month(month));
        });
        refresh_workbench_views();
    });
    row.append(&field_row("Month", &month));

    let days = calendar_days_in_month(date.month);
    let day_labels = (1..=days).map(|day| day.to_string()).collect::<Vec<_>>();
    let day_refs = day_labels.iter().map(String::as_str).collect::<Vec<_>>();
    let day = DropDown::from_strings(&day_refs);
    day.set_tooltip_text(Some("Projection day"));
    day.set_selected(u32::from(
        date.day.saturating_sub(1).min(days.saturating_sub(1)),
    ));
    day.connect_selected_notify(|dropdown| {
        let day = (dropdown.selected() as u8).saturating_add(1).clamp(1, 31);
        SIMULATION_GRAPH_DATE.with(|selection| selection.borrow_mut().day = day);
        refresh_workbench_views();
    });
    row.append(&field_row("Day", &day));
    row
}

fn daily_projection_chart(production: Vec<f64>, load: Vec<f64>) -> DrawingArea {
    let chart = DrawingArea::new();
    chart.set_content_height(260);
    chart.set_hexpand(true);
    chart.set_draw_func(move |chart, context, width, height| {
        let colors = ChartColors::from_widget(chart);
        draw_daily_chart(context, width, height, &colors, &production, &load);
    });
    chart
}

fn selected_daily_projection_date() -> DailyProjectionDate {
    SIMULATION_GRAPH_DATE.with(|selection| {
        let mut selection = selection.borrow_mut();
        selection.month = selection.month.clamp(1, 12);
        let days = calendar_days_in_month(selection.month);
        selection.day = selection.day.clamp(1, days);
        *selection
    })
}

fn calendar_days_in_month(month: u8) -> u8 {
    days_in_month(month).unwrap_or(30.0) as u8
}

fn monthly_totals(values: &[f64]) -> Vec<f64> {
    let mut totals = Vec::with_capacity(12);
    let mut start = 0usize;
    for month in 1..=12 {
        let hours = calendar_days_in_month(month) as usize * 24;
        let end = start.saturating_add(hours).min(values.len());
        totals.push(values[start..end].iter().sum());
        start = end;
    }
    totals
}

fn daily_slice(values: &[f64], date: DailyProjectionDate) -> Vec<f64> {
    let day_index = day_of_year_index(date);
    let start = day_index.saturating_mul(24).min(values.len());
    let end = start.saturating_add(24).min(values.len());
    let mut output = values[start..end].to_vec();
    output.resize(24, 0.0);
    output
}

fn day_of_year_index(date: DailyProjectionDate) -> usize {
    let mut days = 0usize;
    for month in 1..date.month {
        days += calendar_days_in_month(month) as usize;
    }
    days + usize::from(date.day.saturating_sub(1))
}

#[derive(Clone, Copy)]
struct ChartScale {
    top: f64,
    height: f64,
    max_value: f64,
}

#[derive(Clone, Copy)]
enum ChartColorRole {
    Production,
    Load,
}

struct ChartColors {
    background: gtk::gdk::RGBA,
    grid: gtk::gdk::RGBA,
    text: gtk::gdk::RGBA,
    production: gtk::gdk::RGBA,
    load: gtk::gdk::RGBA,
}

impl ChartColors {
    fn from_widget<W: IsA<gtk::Widget>>(widget: &W) -> Self {
        Self {
            background: theme_color(widget, "workbench", gtk::gdk::RGBA::new(0.0, 0.0, 0.0, 1.0)),
            grid: theme_color(widget, "border", gtk::gdk::RGBA::new(0.35, 0.35, 0.35, 1.0)),
            text: theme_color(widget, "text_1", gtk::gdk::RGBA::new(0.8, 0.8, 0.8, 1.0)),
            production: theme_color(widget, "accent", gtk::gdk::RGBA::new(0.2, 0.45, 0.9, 1.0)),
            load: theme_color(widget, "warning", gtk::gdk::RGBA::new(0.95, 0.7, 0.25, 1.0)),
        }
    }

    fn series(&self, role: ChartColorRole) -> &gtk::gdk::RGBA {
        match role {
            ChartColorRole::Production => &self.production,
            ChartColorRole::Load => &self.load,
        }
    }
}

#[derive(Clone, Copy)]
struct BarStyle {
    width: f64,
    role: ChartColorRole,
}

fn draw_monthly_chart(
    context: &gtk::cairo::Context,
    width: i32,
    height: i32,
    colors: &ChartColors,
    production: &[f64],
    load: &[f64],
) {
    let max_value = production
        .iter()
        .chain(load.iter())
        .copied()
        .fold(0.0_f64, f64::max)
        .max(1.0);
    draw_chart_background(context, width, height, colors, max_value, "kWh");
    let left = 52.0;
    let right = 14.0;
    let top = 18.0;
    let bottom = 42.0;
    let chart_width = (f64::from(width) - left - right).max(1.0);
    let chart_height = (f64::from(height) - top - bottom).max(1.0);
    let group_width = chart_width / 12.0;
    let bar_width = (group_width * 0.30).max(2.0);
    for index in 0..12 {
        let x = left + group_width * index as f64 + group_width * 0.19;
        let scale = ChartScale {
            top,
            height: chart_height,
            max_value,
        };
        draw_bar(
            context,
            x,
            production[index],
            scale,
            colors,
            BarStyle {
                width: bar_width,
                role: ChartColorRole::Production,
            },
        );
        draw_bar(
            context,
            x + bar_width + 3.0,
            load[index],
            scale,
            colors,
            BarStyle {
                width: bar_width,
                role: ChartColorRole::Load,
            },
        );
        draw_axis_label(
            context,
            x + bar_width * 0.5,
            f64::from(height) - 18.0,
            colors,
            short_month_name((index + 1) as u8).unwrap_or("?"),
        );
    }
}

fn draw_daily_chart(
    context: &gtk::cairo::Context,
    width: i32,
    height: i32,
    colors: &ChartColors,
    production: &[f64],
    load: &[f64],
) {
    let max_value = production
        .iter()
        .chain(load.iter())
        .copied()
        .fold(0.0_f64, f64::max)
        .max(1.0);
    draw_chart_background(context, width, height, colors, max_value, "kWh/h");
    let left = 52.0;
    let right = 14.0;
    let top = 18.0;
    let bottom = 42.0;
    let chart_width = (f64::from(width) - left - right).max(1.0);
    let chart_height = (f64::from(height) - top - bottom).max(1.0);
    let group_width = chart_width / 24.0;
    let bar_width = (group_width * 0.28).max(1.0);
    for hour in 0..24 {
        let x = left + group_width * hour as f64 + group_width * 0.18;
        let scale = ChartScale {
            top,
            height: chart_height,
            max_value,
        };
        draw_bar(
            context,
            x,
            production[hour],
            scale,
            colors,
            BarStyle {
                width: bar_width,
                role: ChartColorRole::Production,
            },
        );
        draw_bar(
            context,
            x + bar_width + 2.0,
            load[hour],
            scale,
            colors,
            BarStyle {
                width: bar_width,
                role: ChartColorRole::Load,
            },
        );
        if hour % 3 == 0 {
            draw_axis_label(
                context,
                x,
                f64::from(height) - 18.0,
                colors,
                &format!("{hour}"),
            );
        }
    }
}

fn draw_chart_background(
    context: &gtk::cairo::Context,
    width: i32,
    height: i32,
    colors: &ChartColors,
    max_value: f64,
    unit: &str,
) {
    let width = f64::from(width);
    let height = f64::from(height);
    let left = 52.0;
    let right = 14.0;
    let top = 18.0;
    let bottom = 42.0;
    let chart_width = (width - left - right).max(1.0);
    let chart_height = (height - top - bottom).max(1.0);

    set_source_rgba(context, &colors.background);
    context.rectangle(0.0, 0.0, width, height);
    let _ = context.fill();

    context.set_line_width(1.0);
    set_source_rgba(context, &colors.grid);
    for tick in 0..=4 {
        let y = top + chart_height * tick as f64 / 4.0;
        context.move_to(left, y);
        context.line_to(left + chart_width, y);
        let _ = context.stroke();
        let value = max_value * (1.0 - tick as f64 / 4.0);
        draw_axis_label(context, 6.0, y + 4.0, colors, &format!("{value:.0}"));
    }
    draw_axis_label(context, 6.0, top - 4.0, colors, unit);
}

fn draw_bar(
    context: &gtk::cairo::Context,
    x: f64,
    value: f64,
    scale: ChartScale,
    colors: &ChartColors,
    style: BarStyle,
) {
    let height = (value / scale.max_value).clamp(0.0, 1.0) * scale.height;
    set_source_rgba(context, colors.series(style.role));
    context.rectangle(
        x,
        scale.top + scale.height - height,
        style.width,
        height.max(1.0),
    );
    let _ = context.fill();
}

fn draw_axis_label(
    context: &gtk::cairo::Context,
    x: f64,
    y: f64,
    colors: &ChartColors,
    text: &str,
) {
    set_source_rgba(context, &colors.text);
    context.select_font_face(
        "Sans",
        gtk::cairo::FontSlant::Normal,
        gtk::cairo::FontWeight::Normal,
    );
    context.set_font_size(10.0);
    context.move_to(x, y);
    let _ = context.show_text(text);
}

fn set_source_rgba(context: &gtk::cairo::Context, color: &gtk::gdk::RGBA) {
    context.set_source_rgba(
        f64::from(color.red()),
        f64::from(color.green()),
        f64::from(color.blue()),
        f64::from(color.alpha()),
    );
}

fn theme_color<W: IsA<gtk::Widget>>(
    widget: &W,
    name: &str,
    fallback: gtk::gdk::RGBA,
) -> gtk::gdk::RGBA {
    widget
        .style_context()
        .lookup_color(name)
        .unwrap_or(fallback)
}

fn rgba_to_hex(color: &gtk::gdk::RGBA) -> String {
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(color.red()),
        channel(color.green()),
        channel(color.blue())
    )
}

fn simulation_summary_table(result: &SimulationResult) -> Grid {
    let grid = Grid::new();
    grid.set_column_spacing(14);
    grid.set_row_spacing(5);
    grid.set_hexpand(true);
    grid.set_column_homogeneous(true);

    add_estimate_table_header(&grid, 0, 0, "Metric", 0.0);
    add_estimate_table_header(&grid, 1, 0, "mean", 1.0);
    add_estimate_table_header(&grid, 2, 0, "p50", 1.0);
    add_estimate_table_header(&grid, 3, 0, "p10", 1.0);
    add_estimate_table_header(&grid, 4, 0, "p90", 1.0);

    let summaries = &result.summaries;
    let rows = [
        (
            "Production kWh",
            summaries.production_kwh,
            SimulationValueKind::Kwh,
        ),
        ("Load kWh", summaries.load_kwh, SimulationValueKind::Kwh),
        (
            "Self consumed kWh",
            summaries.self_consumed_kwh,
            SimulationValueKind::Kwh,
        ),
        (
            "Grid import kWh",
            summaries.grid_import_kwh,
            SimulationValueKind::Kwh,
        ),
        (
            "Grid export kWh",
            summaries.grid_export_kwh,
            SimulationValueKind::Kwh,
        ),
        (
            "Storage consumed kWh",
            summaries.storage_consumed_kwh,
            SimulationValueKind::Kwh,
        ),
        (
            "Battery losses kWh",
            summaries.battery_losses_kwh,
            SimulationValueKind::Kwh,
        ),
        (
            "Ending charge kWh",
            summaries.ending_soc_kwh,
            SimulationValueKind::Kwh,
        ),
        (
            "Self consumption %",
            summaries.self_consumption_ratio,
            SimulationValueKind::Percent,
        ),
        (
            "Self sufficiency %",
            summaries.self_sufficiency_ratio,
            SimulationValueKind::Percent,
        ),
    ];

    for (index, (label, summary, kind)) in rows.iter().enumerate() {
        add_simulation_summary_row(&grid, index as i32 + 1, label, *summary, *kind);
    }

    grid
}

fn add_simulation_summary_row(
    grid: &Grid,
    row: i32,
    label: &str,
    summary: MetricSummary,
    kind: SimulationValueKind,
) {
    add_estimate_table_cell(grid, 0, row, label, 0.0, EstimateTone::Muted);
    add_estimate_table_cell(
        grid,
        1,
        row,
        &format_simulation_value(summary.mean, kind),
        1.0,
        EstimateTone::Mean,
    );
    add_estimate_table_cell(
        grid,
        2,
        row,
        &format_simulation_value(summary.p50, kind),
        1.0,
        EstimateTone::Strong,
    );
    add_estimate_table_cell(
        grid,
        3,
        row,
        &format_simulation_value(summary.p10, kind),
        1.0,
        EstimateTone::Normal,
    );
    add_estimate_table_cell(
        grid,
        4,
        row,
        &format_simulation_value(summary.p90, kind),
        1.0,
        EstimateTone::Normal,
    );
}

fn simulation_scenario_table(result: &SimulationResult) -> Grid {
    let grid = Grid::new();
    grid.set_column_spacing(14);
    grid.set_row_spacing(5);
    grid.set_hexpand(true);
    grid.set_column_homogeneous(true);

    add_estimate_table_group_header(&grid, 1, 8, "kWh");
    add_estimate_table_group_header(&grid, 9, 2, "%");
    add_estimate_table_header(&grid, 0, 1, "Case", 0.0);
    add_estimate_table_header(&grid, 1, 1, "prod", 1.0);
    add_estimate_table_header(&grid, 2, 1, "load", 1.0);
    add_estimate_table_header(&grid, 3, 1, "self", 1.0);
    add_estimate_table_header(&grid, 4, 1, "import", 1.0);
    add_estimate_table_header(&grid, 5, 1, "export", 1.0);
    add_estimate_table_header(&grid, 6, 1, "storage", 1.0);
    add_estimate_table_header(&grid, 7, 1, "loss", 1.0);
    add_estimate_table_header(&grid, 8, 1, "end", 1.0);
    add_estimate_table_header(&grid, 9, 1, "cons", 1.0);
    add_estimate_table_header(&grid, 10, 1, "suff", 1.0);

    add_simulation_scenario_row(&grid, 2, "Low", result.scenarios.low);
    add_simulation_scenario_row(&grid, 3, "Mean", result.scenarios.mean);
    add_simulation_scenario_row(&grid, 4, "High", result.scenarios.high);
    grid
}

fn add_simulation_scenario_row(grid: &Grid, row: i32, label: &str, metrics: SimulationRunMetrics) {
    add_estimate_table_cell(grid, 0, row, label, 0.0, EstimateTone::Muted);
    add_estimate_table_cell(
        grid,
        1,
        row,
        &format_kwh_value(metrics.production_kwh),
        1.0,
        EstimateTone::Normal,
    );
    add_estimate_table_cell(
        grid,
        2,
        row,
        &format_kwh_value(metrics.load_kwh),
        1.0,
        EstimateTone::Normal,
    );
    add_estimate_table_cell(
        grid,
        3,
        row,
        &format_kwh_value(metrics.self_consumed_kwh),
        1.0,
        EstimateTone::Normal,
    );
    add_estimate_table_cell(
        grid,
        4,
        row,
        &format_kwh_value(metrics.grid_import_kwh),
        1.0,
        EstimateTone::Normal,
    );
    add_estimate_table_cell(
        grid,
        5,
        row,
        &format_kwh_value(metrics.grid_export_kwh),
        1.0,
        EstimateTone::Normal,
    );
    add_estimate_table_cell(
        grid,
        6,
        row,
        &format_kwh_value(metrics.storage_consumed_kwh),
        1.0,
        EstimateTone::Normal,
    );
    add_estimate_table_cell(
        grid,
        7,
        row,
        &format_kwh_value(metrics.battery_losses_kwh),
        1.0,
        EstimateTone::Normal,
    );
    add_estimate_table_cell(
        grid,
        8,
        row,
        &format_kwh_value(metrics.ending_soc_kwh),
        1.0,
        EstimateTone::Normal,
    );
    add_estimate_table_cell(
        grid,
        9,
        row,
        &format_percent_value(metrics.self_consumption_ratio),
        1.0,
        EstimateTone::Mean,
    );
    add_estimate_table_cell(
        grid,
        10,
        row,
        &format_percent_value(metrics.self_sufficiency_ratio),
        1.0,
        EstimateTone::Mean,
    );
}

#[derive(Clone, Copy)]
enum SimulationValueKind {
    Kwh,
    Percent,
}

fn format_simulation_value(value: f64, kind: SimulationValueKind) -> String {
    match kind {
        SimulationValueKind::Kwh => format_kwh_value(value),
        SimulationValueKind::Percent => format_percent_value(value),
    }
}

fn format_kwh_value(value: f64) -> String {
    format!("{value:.0}")
}

fn format_percent_value(value: f64) -> String {
    format!("{:.0}%", value * 100.0)
}

fn append_simulation_empty_state(content: &GtkBox, state: &DesktopState) {
    let label = body_label(computation_empty_message(
        state,
        ComputationStage::Simulation,
    ));
    apply_text_role(&label, "section-label");
    content.append(&label);
    append_retry_action(content, state, ComputationStage::Simulation);
    append_manual_computation_action(content, state, ComputationStage::Simulation);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefreshScope {
    All,
    Workbench,
}

fn update_state(update: impl FnOnce(&mut DesktopState)) {
    update_project_inputs(
        update,
        RefreshScope::All,
        ComputationImpact::EstimateAndSimulation,
        ComputationTrigger::Immediate,
    );
}

fn update_input_state(update: impl FnOnce(&mut DesktopState)) {
    update_project_inputs(
        update,
        RefreshScope::Workbench,
        ComputationImpact::EstimateAndSimulation,
        ComputationTrigger::Debounced,
    );
}

fn update_simulation_input_state(update: impl FnOnce(&mut DesktopState)) {
    update_project_inputs(
        update,
        RefreshScope::Workbench,
        ComputationImpact::Simulation,
        ComputationTrigger::Debounced,
    );
}

fn update_simulation_state(update: impl FnOnce(&mut DesktopState)) {
    update_project_inputs(
        update,
        RefreshScope::All,
        ComputationImpact::Simulation,
        ComputationTrigger::Immediate,
    );
}

fn rename_setup_from_entry(name: &Entry) -> bool {
    let name = name.text().trim().to_string();
    if name.is_empty() {
        append_log("Setup name cannot be empty".to_string());
        refresh_workbench_views();
        return false;
    }

    rename_setup(name);
    true
}

fn rename_setup(name: String) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(project) = state.project.as_mut() else {
            return;
        };
        project.metadata.title = name.clone();
        state.dirty = true;
        state.status = format!("Renamed setup to {name}");
    });
    schedule_automatic_save();
    refresh_views();
}

fn update_project_inputs(
    update: impl FnOnce(&mut DesktopState),
    refresh_scope: RefreshScope,
    impact: ComputationImpact,
    trigger: ComputationTrigger,
) {
    let updated = STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.project.is_none() {
            let message = "Open or create a project first".to_string();
            state.status = message.clone();
            state.log.push(message);
            return false;
        }
        update(&mut state);
        state.dirty = true;
        if let Some(project) = state.project.as_mut() {
            if impact == ComputationImpact::EstimateAndSimulation {
                project.results.estimate = None;
                project.results.production_profile = None;
            } else if let Some(document) = project.results.estimate.as_mut() {
                document.system.storage_usable_kwh =
                    project.inputs.estimate_request.storage_usable_kwh;
            }
            project.results.simulation = None;
            project.results.simulation_metadata = None;
            invalidate_running_simulation(&mut state);
        }
        true
    });

    if !updated {
        refresh_views();
        return;
    }

    schedule_automatic_save();
    if automatic_simulation_updates_enabled() {
        schedule_automatic_computation(impact, trigger);
    } else {
        clear_automatic_computation();
    }

    match refresh_scope {
        RefreshScope::All => refresh_views(),
        RefreshScope::Workbench => refresh_workbench_views(),
    }
}

fn set_energy_price(value: Option<f64>) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(project) = state.project.as_mut() else {
            return;
        };
        project.inputs.energy_price_eur_per_kwh = value;
        state.dirty = true;
        state.status = "Energy price updated".to_string();
    });
    schedule_automatic_save();
    refresh_workbench_views();
}

fn append_log(message: String) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.status = message.clone();
        state.log.push(message);
        if state.log.len() > 200 {
            let excess = state.log.len() - 200;
            state.log.drain(0..excess);
        }
    });
}

fn snapshot() -> DesktopState {
    STATE.with(|state| state.borrow().clone())
}

fn icon_button(icon_name: &str, tooltip: &str) -> Button {
    let button = Button::new();
    apply_button_role(&button, "icon");
    button.set_tooltip_text(Some(tooltip));
    let icon = Image::from_icon_name(icon_name);
    icon.set_icon_size(gtk::IconSize::Normal);
    button.set_child(Some(&icon));
    button
}

fn number_entry(value: f64, digits: u32, update: impl Fn(f64) + 'static) -> Entry {
    let entry = field_entry();
    entry.set_input_purpose(gtk::InputPurpose::Number);
    entry.set_text(&format_number(value, digits));
    entry.connect_changed(move |entry| {
        if let Some(value) = parse_number(&entry.text()) {
            update(value);
        }
    });
    entry.connect_activate(|_| flush_pending_computation());
    entry
}

fn optional_number_entry(
    value: Option<f64>,
    digits: u32,
    update: impl Fn(Option<f64>) + 'static,
) -> Entry {
    let entry = field_entry();
    entry.set_input_purpose(gtk::InputPurpose::Number);
    if let Some(value) = value {
        entry.set_text(&format_number(value, digits));
    }
    entry.connect_changed(move |entry| {
        let text = entry.text();
        if text.trim().is_empty() {
            update(None);
        } else if let Some(value) = parse_number(&text) {
            update(Some(value));
        }
    });
    entry.connect_activate(|_| flush_pending_computation());
    entry
}

fn format_number(value: f64, digits: u32) -> String {
    if digits == 0 {
        format!("{value:.0}")
    } else {
        format!("{value:.digits$}", digits = digits as usize).replace('.', ",")
    }
}

fn parse_number(value: &str) -> Option<f64> {
    let normalized = value.trim().replace(',', ".");
    if normalized.is_empty() {
        return None;
    }
    normalized.parse::<f64>().ok()
}

fn format_runs(runs: usize) -> String {
    let text = runs.to_string();
    let mut output = String::new();
    for (index, character) in text.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            output.push(',');
        }
        output.push(character);
    }
    output.chars().rev().collect()
}

fn azimuth_field_row(azimuth: &Entry) -> GtkBox {
    let value_row = GtkBox::new(Orientation::Horizontal, 8);
    value_row.set_hexpand(true);
    azimuth.set_hexpand(true);
    azimuth.set_halign(Align::Fill);

    let direction = Label::new(Some(&azimuth_direction_label(
        parse_number(&azimuth.text()).unwrap_or(0.0),
    )));
    direction.set_width_chars(3);
    direction.set_xalign(0.0);
    apply_text_role(&direction, "meta");

    let direction_for_change = direction.clone();
    azimuth.connect_changed(move |entry| {
        if let Some(value) = parse_number(&entry.text()) {
            direction_for_change.set_text(&azimuth_direction_label(value));
        } else {
            direction_for_change.set_text("");
        }
    });

    value_row.append(azimuth);
    value_row.append(&direction);
    field_row("Azimuth", &value_row)
}

fn azimuth_direction_label(value: f64) -> String {
    const DIRECTIONS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    let compass_degrees = (180.0 + value).rem_euclid(360.0);
    let index = ((compass_degrees + 22.5) / 45.0).floor() as usize % DIRECTIONS.len();
    DIRECTIONS[index].to_string()
}

fn annual_energy_value(document: &pv_core::source_model::SourceEnsembleEstimateDocument) -> String {
    let estimate = &document.ensemble_estimate;
    let mean = estimate.annual_energy.mean.as_kilowatt_hours().round();
    estimate
        .uncertainty
        .annual_energy
        .map(|band| {
            format!(
                "{mean:.0} low..high {:.0}..{:.0}",
                band.low.as_kilowatt_hours().round(),
                band.high.as_kilowatt_hours().round()
            )
        })
        .unwrap_or_else(|| format!("{mean:.0} low..high -..-"))
}

fn annual_revenue_value(
    document: &pv_core::source_model::SourceEnsembleEstimateDocument,
    price: f64,
) -> String {
    let estimate = &document.ensemble_estimate;
    let mean = (estimate.annual_energy.mean.as_kilowatt_hours() * price).round();
    estimate
        .uncertainty
        .annual_energy
        .map(|band| {
            format!(
                "{mean:.0} low..high {:.0}..{:.0}",
                (band.low.as_kilowatt_hours() * price).round(),
                (band.high.as_kilowatt_hours() * price).round()
            )
        })
        .unwrap_or_else(|| format!("{mean:.0} low..high -..-"))
}

fn monthly_estimate_rows(
    estimate: &pv_core::source_model::AnnualPvEnsembleEstimate,
) -> Vec<[String; 7]> {
    estimate
        .monthly_estimates
        .iter()
        .map(|monthly| {
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
            [
                month_name.to_string(),
                format!("{total_kwh:.1}"),
                total_min,
                total_max,
                format!("{:.1}", total_kwh / days),
                daily_min,
                daily_max,
            ]
        })
        .collect()
}

fn estimate_monthly_table(rows: &[[String; 7]]) -> Grid {
    let grid = Grid::new();
    grid.set_column_spacing(14);
    grid.set_row_spacing(5);
    grid.set_hexpand(true);
    grid.set_column_homogeneous(true);
    add_estimate_table_group_header(&grid, 1, 3, "Monthly kWh");
    add_estimate_table_group_header(&grid, 4, 3, "Daily kWh");
    add_estimate_table_header(&grid, 0, 1, "Month", 0.0);
    add_estimate_table_header(&grid, 1, 1, "mean", 1.0);
    add_estimate_table_header(&grid, 2, 1, "min", 1.0);
    add_estimate_table_header(&grid, 3, 1, "max", 1.0);
    add_estimate_table_header(&grid, 4, 1, "mean", 1.0);
    add_estimate_table_header(&grid, 5, 1, "min", 1.0);
    add_estimate_table_header(&grid, 6, 1, "max", 1.0);

    let minimums = monthly_table_minimums(rows);
    for (index, row_data) in rows.iter().enumerate() {
        let row = index as i32 + 2;
        add_estimate_table_cell(&grid, 0, row, &row_data[0], 0.0, EstimateTone::Muted);
        add_estimate_table_cell(&grid, 1, row, &row_data[1], 1.0, EstimateTone::Mean);
        add_estimate_table_cell(
            &grid,
            2,
            row,
            &row_data[2],
            1.0,
            minimums
                .filter(|(monthly_min, _)| is_table_minimum(&row_data[2], *monthly_min))
                .map(|_| EstimateTone::Minimum)
                .unwrap_or(EstimateTone::Normal),
        );
        add_estimate_table_cell(&grid, 3, row, &row_data[3], 1.0, EstimateTone::Normal);
        add_estimate_table_cell(&grid, 4, row, &row_data[4], 1.0, EstimateTone::Mean);
        add_estimate_table_cell(
            &grid,
            5,
            row,
            &row_data[5],
            1.0,
            minimums
                .filter(|(_, daily_min)| is_table_minimum(&row_data[5], *daily_min))
                .map(|_| EstimateTone::Minimum)
                .unwrap_or(EstimateTone::Normal),
        );
        add_estimate_table_cell(&grid, 6, row, &row_data[6], 1.0, EstimateTone::Normal);
    }
    grid
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EstimateTone {
    Normal,
    Muted,
    Strong,
    Mean,
    Minimum,
}

fn add_estimate_metric_row(grid: &Grid, row: i32, label: &str, value: &str, tone: EstimateTone) {
    grid.attach(
        &estimate_text_label(label, 0.0, EstimateTone::Muted, false),
        0,
        row,
        1,
        1,
    );
    let value = estimate_text_label(value, 0.0, tone, true);
    value.set_hexpand(true);
    value.set_wrap(true);
    grid.attach(&value, 1, row, 1, 1);
}

fn add_estimate_table_group_header(grid: &Grid, column: i32, width: i32, text: &str) {
    grid.attach(
        &estimate_text_label(text, 0.5, EstimateTone::Muted, true),
        column,
        0,
        width,
        1,
    );
}

fn add_estimate_table_header(grid: &Grid, column: i32, row: i32, text: &str, xalign: f32) {
    grid.attach(
        &estimate_text_label(text, xalign, EstimateTone::Muted, true),
        column,
        row,
        1,
        1,
    );
}

fn add_estimate_table_cell(
    grid: &Grid,
    column: i32,
    row: i32,
    text: &str,
    xalign: f32,
    tone: EstimateTone,
) {
    grid.attach(
        &estimate_text_label(text, xalign, tone, true),
        column,
        row,
        1,
        1,
    );
}

fn estimate_text_label(text: &str, xalign: f32, tone: EstimateTone, monospace: bool) -> Label {
    let label = Label::new(None);
    label.set_xalign(xalign);
    label.set_hexpand(true);
    label.set_halign(Align::Fill);
    if monospace {
        apply_text_role(&label, "code");
    }
    label.set_markup(&estimate_markup(&label, text, tone));
    label
}

fn estimate_markup(label: &Label, text: &str, tone: EstimateTone) -> String {
    let text = escape_markup(text);
    match tone {
        EstimateTone::Normal => format!(r##"<span size="large">{text}</span>"##),
        EstimateTone::Muted => {
            let color = theme_color_hex(label, "text_2", gtk::gdk::RGBA::new(0.55, 0.55, 0.6, 1.0));
            format!(r##"<span size="large" foreground="{color}">{text}</span>"##)
        }
        EstimateTone::Strong | EstimateTone::Mean => {
            let color = theme_color_hex(
                label,
                "accent_strong",
                gtk::gdk::RGBA::new(0.7, 0.78, 1.0, 1.0),
            );
            format!(r##"<span size="large" foreground="{color}" weight="bold">{text}</span>"##)
        }
        EstimateTone::Minimum => {
            let color = theme_color_hex(label, "danger", gtk::gdk::RGBA::new(1.0, 0.7, 0.67, 1.0));
            format!(r##"<span size="large" foreground="{color}" weight="bold">{text}</span>"##)
        }
    }
}

fn theme_color_hex<W: IsA<gtk::Widget>>(
    widget: &W,
    name: &str,
    fallback: gtk::gdk::RGBA,
) -> String {
    rgba_to_hex(&theme_color(widget, name, fallback))
}
fn escape_markup(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn field_row<W: IsA<gtk::Widget>>(label: &str, widget: &W) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 12);
    row.set_hexpand(true);

    let label = Label::new(Some(label));
    label.set_xalign(0.0);
    label.set_valign(Align::Center);
    label.set_width_chars(15);
    label.set_max_width_chars(15);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    apply_text_role(&label, "meta");

    widget.set_hexpand(true);
    widget.set_halign(Align::Fill);

    row.append(&label);
    row.append(widget);
    row
}

fn header_label(text: &str) -> Label {
    let label = Label::new(Some(text));
    label.set_xalign(0.0);
    apply_text_role(&label, "title");
    label
}

fn section_label(text: &str) -> Label {
    let label = Label::new(Some(text));
    label.set_xalign(0.0);
    apply_text_role(&label, "section-label");
    label
}

fn body_label(text: &str) -> Label {
    let label = Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label
}

fn meta_label(text: &str) -> Label {
    let label = body_label(text);
    apply_text_role(&label, "meta");
    label
}

fn section_separator() -> Separator {
    Separator::new(Orientation::Horizontal)
}

fn workbench_scroller() -> ScrolledWindow {
    ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .hscrollbar_policy(PolicyType::Automatic)
        .vscrollbar_policy(PolicyType::Automatic)
        .build()
}

fn workbench_content() -> GtkBox {
    let content = GtkBox::new(Orientation::Vertical, 8);
    content.set_margin_top(12);
    content.set_margin_bottom(12);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content
}

fn details_content() -> GtkBox {
    let content = GtkBox::new(Orientation::Vertical, 8);
    content.set_margin_top(10);
    content.set_margin_bottom(10);
    content.set_margin_start(10);
    content.set_margin_end(10);
    content
}

fn add_table_header(grid: &Grid, column: i32, text: &str, xalign: f32) {
    let label = Label::new(Some(text));
    label.set_xalign(xalign);
    label.set_hexpand(column == 0);
    apply_text_role(&label, "section-label");
    grid.attach(&label, column, 0, 1, 1);
}

fn add_table_cell(grid: &Grid, column: i32, row: i32, text: &str, xalign: f32, expands: bool) {
    let label = Label::new(Some(text));
    label.set_xalign(xalign);
    label.set_hexpand(expands);
    if !expands {
        apply_text_role(&label, "code");
    }
    grid.attach(&label, column, row, 1, 1);
}

fn load_shape(load_profile: &LoadProfile) -> LoadShape {
    match load_profile {
        LoadProfile::AnnualKwh { shape, .. } | LoadProfile::DailyKwh { shape, .. } => shape.clone(),
    }
}

fn shape_index(shape: &LoadShape) -> u32 {
    match shape {
        LoadShape::BuiltIn { shape_id } => match shape_id {
            BuiltInLoadShapeId::ResidentialDefault => 0,
            BuiltInLoadShapeId::Flat => 1,
            BuiltInLoadShapeId::Daytime => 2,
            BuiltInLoadShapeId::Evening => 3,
        },
        LoadShape::HourlyWeights { .. } => 0,
    }
}

fn shape_from_index(index: u32) -> BuiltInLoadShapeId {
    match index {
        1 => BuiltInLoadShapeId::Flat,
        2 => BuiltInLoadShapeId::Daytime,
        3 => BuiltInLoadShapeId::Evening,
        _ => BuiltInLoadShapeId::ResidentialDefault,
    }
}

fn clear_box(root: &GtkBox) {
    while let Some(child) = root.first_child() {
        root.remove(&child);
    }
}

export_plugin!(PvDesktopPlugin);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_desktop_session_uses_safe_settings_defaults() {
        let session: DesktopSession =
            serde_json::from_str(r#"{"schema_version":1,"last_project_path":null}"#)
                .expect("legacy desktop session should deserialize");

        assert!(session.settings.automatic_simulation_updates);
        assert_eq!(
            session.settings.color_palette,
            ColorPalettePreference::System
        );
        assert!(!session.settings.automatic_project_save);
    }

    #[test]
    fn application_settings_roundtrip_through_session_json() {
        let session = DesktopSession {
            schema_version: DESKTOP_SESSION_SCHEMA_VERSION,
            last_project_path: Some(PathBuf::from("example.pvproj")),
            settings: ApplicationSettings {
                automatic_simulation_updates: false,
                color_palette: ColorPalettePreference::Light,
                automatic_project_save: true,
            },
        };

        let json = serde_json::to_string(&session).expect("session should serialize");
        let restored: DesktopSession =
            serde_json::from_str(&json).expect("session should deserialize");
        assert_eq!(restored.settings, session.settings);
        assert_eq!(restored.last_project_path, session.last_project_path);
    }

    #[test]
    fn desktop_azimuth_label_matches_pvgis_convention() {
        assert_eq!(azimuth_direction_label(0.0), "S");
        assert_eq!(azimuth_direction_label(-90.0), "E");
        assert_eq!(azimuth_direction_label(90.0), "W");
        assert_eq!(azimuth_direction_label(180.0), "N");
        assert_eq!(azimuth_direction_label(-180.0), "N");
        assert_eq!(azimuth_direction_label(45.0), "SW");
        assert_eq!(azimuth_direction_label(-45.0), "SE");
    }

    #[test]
    fn computation_impact_keeps_the_strongest_dependency() {
        assert_eq!(
            ComputationImpact::Simulation.merge(ComputationImpact::Simulation),
            ComputationImpact::Simulation
        );
        assert_eq!(
            ComputationImpact::Simulation.merge(ComputationImpact::EstimateAndSimulation),
            ComputationImpact::EstimateAndSimulation
        );
        assert_eq!(
            ComputationImpact::EstimateAndSimulation.merge(ComputationImpact::Simulation),
            ComputationImpact::EstimateAndSimulation
        );
    }

    #[test]
    fn missing_or_stale_results_select_the_minimum_computation() {
        assert_eq!(
            required_computation(false, false, None, 10_000),
            Some(ComputationImpact::EstimateAndSimulation)
        );
        assert_eq!(
            required_computation(true, true, None, 10_000),
            Some(ComputationImpact::Simulation)
        );
        assert_eq!(
            required_computation(true, true, Some((true, 10_000)), 10_000),
            Some(ComputationImpact::Simulation)
        );
        assert_eq!(
            required_computation(true, true, Some((false, 1_000)), 10_000),
            Some(ComputationImpact::Simulation)
        );
        assert_eq!(
            required_computation(true, true, Some((false, 10_000)), 10_000),
            None
        );
    }

    #[test]
    fn array_name_is_cosmetic_but_electrical_fields_are_functional() {
        let current = EstimateArray {
            name: Some("Roof".to_string()),
            peak_power_kwp: 4.0,
            tilt_deg: 30.0,
            azimuth_deg: 0.0,
        };
        let mut updated = current.clone();
        updated.name = Some("South roof".to_string());
        assert!(!array_edit_is_functional(&current, &updated));

        updated.peak_power_kwp = 4.1;
        assert!(array_edit_is_functional(&current, &updated));
        updated = current.clone();
        updated.tilt_deg = 31.0;
        assert!(array_edit_is_functional(&current, &updated));
        updated = current.clone();
        updated.azimuth_deg = 1.0;
        assert!(array_edit_is_functional(&current, &updated));
    }
}
