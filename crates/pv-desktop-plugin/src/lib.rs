#![allow(deprecated)]

use std::cell::RefCell;
use std::ffi::c_void;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use directories::ProjectDirs;
use gtk::glib::translate::IntoGlibPtr;
use gtk::prelude::*;
use gtk::{
    Align, Box as GtkBox, Button, DrawingArea, DropDown, Entry, FileChooserAction,
    FileChooserDialog, FileFilter, Grid, Image, Label, ListBox, Orientation, PolicyType, Popover,
    ProgressBar, ResponseType, ScrolledWindow, SelectionMode, Separator, Window,
};
use maruzzella_sdk::{
    CommandSpec, HostApi, MzStatusCode, MzViewPlacement, Plugin, PluginDependency,
    PluginDescriptor, SurfaceContributionSpec, Version, ViewFactorySpec, export_plugin,
};
use pv_core::simulation::{
    BuiltInLoadShapeId, LoadProfile, LoadShape, MetricSummary, ProductionProfile,
    SimulationRequest, SimulationResult, SimulationRunMetrics, StorageConfig,
    deterministic_hourly_load_kwh, simulate_with_progress,
};
use pv_core::source_model::SourceEnsembleEstimateDocument;
use pv_data::{CitySearchResult, search_cities};
use pv_desktop_core::{PROJECT_EXTENSION, PvProjectDocument, load_project, save_project};
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
const DESKTOP_SESSION_SCHEMA_VERSION: u32 = 1;

const CMD_NEW: &str = "pv.project.new";
const CMD_OPEN: &str = "pv.project.open";
const CMD_CLOSE: &str = "pv.project.close";
const CMD_SAVE: &str = "pv.project.save";
const CMD_SAVE_AS: &str = "pv.project.save_as";
const CMD_RUN_ESTIMATE: &str = "pv.project.run_estimate";
const CMD_RUN_SIMULATION: &str = "pv.project.run_simulation";
const CMD_SET_SIMULATION_RUNS: &str = "pv.project.set_simulation_runs";
const CMD_EXIT: &str = "pv.app.exit";
const SAVE_ACTION_IDS: &[&str] = &["pv-project-save", "file-save", "save"];

#[derive(Clone, Debug)]
struct DesktopState {
    project: Option<PvProjectDocument>,
    path: Option<PathBuf>,
    dirty: bool,
    status: String,
    log: Vec<String>,
    session_loaded: bool,
    simulation_generation: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DesktopSession {
    schema_version: u32,
    last_project_path: Option<PathBuf>,
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
        }
    }
}

struct ShellModeHandlers {
    show_workspace: Box<dyn Fn()>,
    show_launcher: Box<dyn Fn()>,
    show_estimate_panel: Box<dyn Fn()>,
    show_simulation_panel: Box<dyn Fn()>,
}

struct SimulationRunState {
    requested_runs: usize,
    completed_runs: usize,
    cancel: Arc<AtomicBool>,
    cancelling: bool,
    receiver: mpsc::Receiver<SimulationRunMessage>,
    generation: u64,
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
    static SIMULATION_GRAPH_DATE: RefCell<DailyProjectionDate> = const { RefCell::new(DailyProjectionDate { month: 6, day: 21 }) };
    static SIMULATION_PROGRESS_VIEWS: RefCell<Vec<SimulationProgressWidgets>> = const { RefCell::new(Vec::new()) };
    static SYSTEM_VIEWS: RefCell<Vec<gtk::glib::WeakRef<GtkBox>>> = const { RefCell::new(Vec::new()) };
    static ESTIMATE_VIEWS: RefCell<Vec<gtk::glib::WeakRef<GtkBox>>> = const { RefCell::new(Vec::new()) };
    static SIMULATION_VIEWS: RefCell<Vec<gtk::glib::WeakRef<GtkBox>>> = const { RefCell::new(Vec::new()) };
}

pub fn install_shell_mode_handlers(
    show_workspace: impl Fn() + 'static,
    show_launcher: impl Fn() + 'static,
    show_estimate_panel: impl Fn() + 'static,
    show_simulation_panel: impl Fn() + 'static,
) {
    SHELL_MODE_HANDLERS.with(|handlers| {
        *handlers.borrow_mut() = Some(ShellModeHandlers {
            show_workspace: Box::new(show_workspace),
            show_launcher: Box::new(show_launcher),
            show_estimate_panel: Box::new(show_estimate_panel),
            show_simulation_panel: Box::new(show_simulation_panel),
        });
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
            CommandSpec::new(PLUGIN_ID, CMD_RUN_ESTIMATE, "Run Estimate")
                .with_handler(command_run_estimate)
                .with_enabled(has_open_project_for_command),
        )?;
        host.register_command(
            CommandSpec::new(PLUGIN_ID, CMD_RUN_SIMULATION, "Run Simulation")
                .with_handler(command_run_simulation)
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
    ensure_estimate_loaded();
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

extern "C" fn command_run_estimate(
    _payload: maruzzella_sdk::ffi::MzBytes,
) -> maruzzella_sdk::ffi::MzStatus {
    if !has_open_project() {
        append_log("Open or create a project first".to_string());
        refresh_views();
        return maruzzella_sdk::ffi::MzStatus::OK;
    }

    show_estimate_workbench_panel();

    match run_estimate() {
        Ok(()) => maruzzella_sdk::ffi::MzStatus::OK,
        Err(message) => {
            append_log(message);
            refresh_views();
            maruzzella_sdk::ffi::MzStatus::new(MzStatusCode::InternalError)
        }
    }
}

extern "C" fn command_run_simulation(
    _payload: maruzzella_sdk::ffi::MzBytes,
) -> maruzzella_sdk::ffi::MzStatus {
    run_simulation_action()
}

fn run_simulation_action() -> maruzzella_sdk::ffi::MzStatus {
    if !has_open_project() {
        append_log("Open or create a project first".to_string());
        refresh_views();
        return maruzzella_sdk::ffi::MzStatus::OK;
    }

    show_simulation_workbench_panel();

    match run_simulation() {
        Ok(()) => maruzzella_sdk::ffi::MzStatus::OK,
        Err(RunSimulationError::NeedsProject) => {
            append_log("Open or create a project first".to_string());
            refresh_views();
            maruzzella_sdk::ffi::MzStatus::OK
        }
        Err(RunSimulationError::NeedsEstimate) => {
            append_log("Run an estimate before simulation".to_string());
            refresh_views();
            maruzzella_sdk::ffi::MzStatus::OK
        }
    }
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
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(project) = state.project.as_mut() else {
            let message = "Open or create a project first".to_string();
            state.status = message.clone();
            state.log.push(message);
            return;
        };
        project.inputs.simulation_options.runs = runs;
        state.dirty = true;
        let message = format!("Simulation runs set to {}", format_runs(runs));
        state.status = message.clone();
        state.log.push(message);
    });
    refresh_views();
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

fn run_estimate() -> Result<(), String> {
    let Some(project) = STATE.with(|state| state.borrow().project.clone()) else {
        return Err("Open or create a project first".to_string());
    };
    append_log("Running source-model estimate".to_string());
    let (estimate, production_profile, annual_kwh) = compute_estimate_for_project(&project)?;
    store_estimate_result(
        estimate,
        production_profile,
        format!("Estimate complete: {annual_kwh:.0} kWh/year"),
        true,
        true,
    );
    refresh_views();
    Ok(())
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
        let estimate = estimator
            .estimate_arrays(&request, &arrays)
            .map_err(|error| format!("Estimate failed: {error:#}"))?;
        let production_profile = estimator
            .production_profile_arrays(&request, &arrays)
            .map_err(|error| format!("Production profile failed: {error:#}"))?;
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

fn ensure_estimate_loaded() {
    let needs_estimate = STATE.with(|state| {
        let state = state.borrow();
        state.project.as_ref().is_some_and(|project| {
            project.results.estimate.is_none() || project.results.production_profile.is_none()
        })
    });
    if needs_estimate
        && let Err(message) = recompute_current_estimate("Estimate ready", false, false)
    {
        append_log(message);
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
        });
    });
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        if let Some(project) = state.project.as_mut() {
            project.results.simulation = None;
            state.dirty = true;
        }
        let message = format!(
            "Running simulation with {} runs",
            format_runs(requested_runs)
        );
        state.status = message.clone();
        state.log.push(message);
    });
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
        })
    })
}

fn cancel_simulation_run() {
    if request_simulation_cancel() {
        STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.status = "Cancelling simulation".to_string();
        });
        if let Some(run) = simulation_run_snapshot() {
            update_simulation_progress_views(run);
        }
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
                    finished = Some((*result, run.generation));
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
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
        finish_simulation_run(result, generation);
        refresh_workbench_views();
    } else if let Some(status) = progress_status {
        STATE.with(|state| {
            state.borrow_mut().status = status;
        });
        if let Some(run) = simulation_run_snapshot() {
            update_simulation_progress_views(run);
        }
    }
}

fn finish_simulation_run(result: Result<SimulationResult, String>, generation: u64) {
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
                if state.simulation_generation != generation {
                    let status = "Simulation result discarded because inputs changed".to_string();
                    state.status = status.clone();
                    state.log.push(status);
                    return;
                }
                let Some(project) = state.project.as_mut() else {
                    return;
                };
                project.results.simulation = Some(result);
                state.dirty = true;
                state.status = status.clone();
                state.log.push(status);
            });
        }
        Err(message) => append_log(message),
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

fn save_current_project() -> Result<SaveDisposition, String> {
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
    if load_project_into_state(&path, true) {
        ensure_estimate_loaded();
        show_project_workspace();
    }
    refresh_views();
}

fn close_project() {
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

fn show_estimate_workbench_panel() {
    SHELL_MODE_HANDLERS.with(|handlers| {
        if let Some(handlers) = handlers.borrow().as_ref() {
            (handlers.show_estimate_panel)();
        }
    });
}

fn show_simulation_workbench_panel() {
    SHELL_MODE_HANDLERS.with(|handlers| {
        if let Some(handlers) = handlers.borrow().as_ref() {
            (handlers.show_simulation_panel)();
        }
    });
}

fn load_project_into_state(path: &Path, remember: bool) -> bool {
    match load_project(path) {
        Ok(project) => {
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

fn prefer_dark_gtk_theme() {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(true);
    }
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
    prefer_dark_gtk_theme();
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
    prefer_dark_gtk_theme();
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
    ensure_estimate_loaded();
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
    ensure_estimate_loaded();
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
    ensure_estimate_loaded();
    let root = GtkBox::new(Orientation::Vertical, 0);
    root.set_hexpand(true);
    root.set_vexpand(true);
    render_simulation_into(&root);
    remember_view(&SIMULATION_VIEWS, &root);
    widget_ptr(root)
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
    refresh_save_action_enabled();
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
    let content = GtkBox::new(Orientation::Vertical, 14);
    content.set_margin_top(32);
    content.set_margin_bottom(32);
    content.set_margin_start(32);
    content.set_margin_end(32);
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
    let content = GtkBox::new(Orientation::Vertical, 14);
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
    new_project.connect_clicked(|_| create_new_project());
    actions.append(&new_project);
    let open_project = Button::with_label("Open Project");
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

    let name = Entry::new();
    name.set_text(&current_name);
    name.set_placeholder_text(Some("Setup name"));
    content.append(&field_row("Name", &name));

    let footer = GtkBox::new(Orientation::Horizontal, 6);
    footer.set_halign(Align::End);
    footer.set_margin_bottom(0);
    let cancel = Button::with_label("Cancel");
    let window_for_cancel = window.clone();
    cancel.connect_clicked(move |_| window_for_cancel.close());
    let save = Button::with_label("Save");
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

    let search = Entry::new();
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
    detail.add_css_class("dim-label");

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
            update_input_state(|state| {
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

    let name = Entry::new();
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
    let window_for_cancel = window.clone();
    cancel.connect_clicked(move |_| window_for_cancel.close());
    let save = Button::with_label("Save");
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
    let entry = Entry::new();
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
    let window_for_cancel = window.clone();
    cancel.connect_clicked(move |_| window_for_cancel.close());
    let delete = Button::with_label("Delete");
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
        update_input_state(|state| {
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
        update_input_state(|state| {
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
            append_estimate_empty_state(&content);
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

fn append_estimate_empty_state(content: &GtkBox) {
    let summary = Grid::new();
    summary.set_column_spacing(18);
    summary.set_row_spacing(8);
    summary.set_hexpand(true);
    add_estimate_metric_row(&summary, 0, "Annual kWh", "-", EstimateTone::Strong);
    content.append(&summary);
    content.append(&body_label("No estimate"));
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
        append_simulation_empty_state(&content);
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
    content.append(&simulation_run_summary(result));
    content.append(&section_separator());
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
    legend.append(&legend_label("Production", "#2ec27e"));
    legend.append(&legend_label("Load", "#f6a43a"));
    legend
}

fn legend_label(text: &str, color: &str) -> Label {
    let label = Label::new(None);
    label.set_markup(&format!(
        r##"<span foreground="{color}" weight="bold">--</span> {text}"##,
        text = escape_markup(text),
    ));
    label
}

fn monthly_simulation_chart(production: &[f64], load: &[f64]) -> DrawingArea {
    let production_months = monthly_totals(production);
    let load_months = monthly_totals(load);
    let chart = DrawingArea::new();
    chart.set_content_height(260);
    chart.set_hexpand(true);
    chart.set_draw_func(move |_, context, width, height| {
        draw_monthly_chart(context, width, height, &production_months, &load_months);
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
    chart.set_draw_func(move |_, context, width, height| {
        draw_daily_chart(context, width, height, &production, &load);
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
struct BarStyle {
    width: f64,
    color: (f64, f64, f64),
}

fn draw_monthly_chart(
    context: &gtk::cairo::Context,
    width: i32,
    height: i32,
    production: &[f64],
    load: &[f64],
) {
    let max_value = production
        .iter()
        .chain(load.iter())
        .copied()
        .fold(0.0_f64, f64::max)
        .max(1.0);
    draw_chart_background(context, width, height, max_value, "kWh");
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
            BarStyle {
                width: bar_width,
                color: (0.18, 0.76, 0.43),
            },
        );
        draw_bar(
            context,
            x + bar_width + 3.0,
            load[index],
            scale,
            BarStyle {
                width: bar_width,
                color: (0.96, 0.64, 0.23),
            },
        );
        draw_axis_label(
            context,
            x + bar_width * 0.5,
            f64::from(height) - 18.0,
            short_month_name((index + 1) as u8).unwrap_or("?"),
        );
    }
}

fn draw_daily_chart(
    context: &gtk::cairo::Context,
    width: i32,
    height: i32,
    production: &[f64],
    load: &[f64],
) {
    let max_value = production
        .iter()
        .chain(load.iter())
        .copied()
        .fold(0.0_f64, f64::max)
        .max(1.0);
    draw_chart_background(context, width, height, max_value, "kWh/h");
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
            BarStyle {
                width: bar_width,
                color: (0.18, 0.76, 0.43),
            },
        );
        draw_bar(
            context,
            x + bar_width + 2.0,
            load[hour],
            scale,
            BarStyle {
                width: bar_width,
                color: (0.96, 0.64, 0.23),
            },
        );
        if hour % 3 == 0 {
            draw_axis_label(context, x, f64::from(height) - 18.0, &format!("{hour}"));
        }
    }
}

fn draw_chart_background(
    context: &gtk::cairo::Context,
    width: i32,
    height: i32,
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

    context.set_source_rgb(0.12, 0.12, 0.13);
    context.rectangle(0.0, 0.0, width, height);
    let _ = context.fill();

    context.set_line_width(1.0);
    context.set_source_rgb(0.28, 0.28, 0.30);
    for tick in 0..=4 {
        let y = top + chart_height * tick as f64 / 4.0;
        context.move_to(left, y);
        context.line_to(left + chart_width, y);
        let _ = context.stroke();
        let value = max_value * (1.0 - tick as f64 / 4.0);
        draw_axis_label(context, 6.0, y + 4.0, &format!("{value:.0}"));
    }
    draw_axis_label(context, 6.0, top - 4.0, unit);
}

fn draw_bar(context: &gtk::cairo::Context, x: f64, value: f64, scale: ChartScale, style: BarStyle) {
    let height = (value / scale.max_value).clamp(0.0, 1.0) * scale.height;
    context.set_source_rgb(style.color.0, style.color.1, style.color.2);
    context.rectangle(
        x,
        scale.top + scale.height - height,
        style.width,
        height.max(1.0),
    );
    let _ = context.fill();
}

fn draw_axis_label(context: &gtk::cairo::Context, x: f64, y: f64, text: &str) {
    context.set_source_rgb(0.70, 0.70, 0.72);
    context.select_font_face(
        "Sans",
        gtk::cairo::FontSlant::Normal,
        gtk::cairo::FontWeight::Normal,
    );
    context.set_font_size(10.0);
    context.move_to(x, y);
    let _ = context.show_text(text);
}

fn simulation_run_summary(result: &SimulationResult) -> Grid {
    let status = if result.cancelled {
        "Cancelled"
    } else {
        "Completed"
    };
    let grid = Grid::new();
    grid.set_column_spacing(18);
    grid.set_row_spacing(8);
    grid.set_hexpand(true);
    let status_tone = if result.cancelled {
        EstimateTone::Minimum
    } else {
        EstimateTone::Strong
    };
    add_estimate_metric_row(&grid, 0, "Status", status, status_tone);
    add_estimate_metric_row(
        &grid,
        1,
        "Runs",
        &format!(
            "{} / {}",
            format_runs(result.completed_runs),
            format_runs(result.requested_runs)
        ),
        EstimateTone::Normal,
    );
    grid
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

    add_estimate_table_group_header(&grid, 1, 7, "kWh");
    add_estimate_table_group_header(&grid, 8, 2, "%");
    add_estimate_table_header(&grid, 0, 1, "Case", 0.0);
    add_estimate_table_header(&grid, 1, 1, "prod", 1.0);
    add_estimate_table_header(&grid, 2, 1, "load", 1.0);
    add_estimate_table_header(&grid, 3, 1, "self", 1.0);
    add_estimate_table_header(&grid, 4, 1, "import", 1.0);
    add_estimate_table_header(&grid, 5, 1, "export", 1.0);
    add_estimate_table_header(&grid, 6, 1, "loss", 1.0);
    add_estimate_table_header(&grid, 7, 1, "end", 1.0);
    add_estimate_table_header(&grid, 8, 1, "cons", 1.0);
    add_estimate_table_header(&grid, 9, 1, "suff", 1.0);

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
        &format_kwh_value(metrics.battery_losses_kwh),
        1.0,
        EstimateTone::Normal,
    );
    add_estimate_table_cell(
        grid,
        7,
        row,
        &format_kwh_value(metrics.ending_soc_kwh),
        1.0,
        EstimateTone::Normal,
    );
    add_estimate_table_cell(
        grid,
        8,
        row,
        &format_percent_value(metrics.self_consumption_ratio),
        1.0,
        EstimateTone::Mean,
    );
    add_estimate_table_cell(
        grid,
        9,
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

fn append_simulation_empty_state(content: &GtkBox) {
    content.append(&simulation_empty_label());
    let run = Button::new();
    run.set_halign(Align::Start);
    run.set_tooltip_text(Some("Run simulation"));
    run.set_size_request(96, 40);
    run.add_css_class("suggested-action");
    run.set_child(Some(&simulation_run_button_content()));
    run.connect_clicked(|_| {
        let _ = run_simulation_action();
    });
    content.append(&run);
}

fn simulation_run_button_content() -> GtkBox {
    let content = GtkBox::new(Orientation::Horizontal, 8);
    content.set_margin_top(7);
    content.set_margin_bottom(7);
    content.set_margin_start(12);
    content.set_margin_end(14);
    let icon = Image::from_icon_name("media-playback-start-symbolic");
    icon.set_icon_size(gtk::IconSize::Normal);
    content.append(&icon);
    content.append(&Label::new(Some("Run")));
    content
}

fn simulation_empty_label() -> Label {
    let label = body_label("No simulation run yet.");
    label.add_css_class("title-4");
    label
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefreshScope {
    All,
    Workbench,
}

fn update_state(update: impl FnOnce(&mut DesktopState)) {
    update_project_inputs(update, RefreshScope::All);
}

fn update_input_state(update: impl FnOnce(&mut DesktopState)) {
    update_project_inputs(update, RefreshScope::Workbench);
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
    refresh_views();
}

fn update_project_inputs(update: impl FnOnce(&mut DesktopState), refresh_scope: RefreshScope) {
    let project = STATE.with(|state| {
        let mut state = state.borrow_mut();
        if state.project.is_none() {
            let message = "Open or create a project first".to_string();
            state.status = message.clone();
            state.log.push(message);
            return None;
        }
        update(&mut state);
        state.dirty = true;
        let project = if let Some(project) = state.project.as_mut() {
            project.results.simulation = None;
            Some(project.clone())
        } else {
            None
        };
        if project.is_some() {
            invalidate_running_simulation(&mut state);
        }
        project
    });

    let Some(project) = project else {
        refresh_views();
        return;
    };

    match compute_estimate_for_project(&project) {
        Ok((estimate, production_profile, annual_kwh)) => store_estimate_result(
            estimate,
            production_profile,
            format!("Estimate updated: {annual_kwh:.0} kWh/year"),
            false,
            true,
        ),
        Err(message) => STATE.with(|state| {
            let mut state = state.borrow_mut();
            let invalidated = if let Some(project) = state.project.as_mut() {
                project.results.estimate = None;
                project.results.production_profile = None;
                project.results.simulation = None;
                true
            } else {
                false
            };
            if invalidated {
                invalidate_running_simulation(&mut state);
            }
            state.status = message;
        }),
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
    button.set_tooltip_text(Some(tooltip));
    let icon = Image::from_icon_name(icon_name);
    icon.set_icon_size(gtk::IconSize::Normal);
    button.set_child(Some(&icon));
    button
}

fn number_entry(value: f64, digits: u32, update: impl Fn(f64) + 'static) -> Entry {
    let entry = Entry::new();
    entry.set_input_purpose(gtk::InputPurpose::Number);
    entry.set_text(&format_number(value, digits));
    entry.connect_changed(move |entry| {
        if let Some(value) = parse_number(&entry.text()) {
            update(value);
        }
    });
    entry
}

fn optional_number_entry(
    value: Option<f64>,
    digits: u32,
    update: impl Fn(Option<f64>) + 'static,
) -> Entry {
    let entry = Entry::new();
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
    direction.add_css_class("dim-label");

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
        label.add_css_class("monospace");
    }
    label.set_markup(&estimate_markup(text, tone));
    label
}

fn estimate_markup(text: &str, tone: EstimateTone) -> String {
    let text = escape_markup(text);
    match tone {
        EstimateTone::Normal => format!(r##"<span size="large">{text}</span>"##),
        EstimateTone::Muted => {
            format!(r##"<span size="large" foreground="#77767b">{text}</span>"##)
        }
        EstimateTone::Strong | EstimateTone::Mean => {
            format!(r##"<span size="large" foreground="#2ec27e" weight="bold">{text}</span>"##)
        }
        EstimateTone::Minimum => {
            format!(r##"<span size="large" foreground="#e01b24" weight="bold">{text}</span>"##)
        }
    }
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
    label.add_css_class("dim-label");

    widget.set_hexpand(true);
    widget.set_halign(Align::Fill);

    row.append(&label);
    row.append(widget);
    row
}

fn header_label(text: &str) -> Label {
    let label = Label::new(Some(text));
    label.set_xalign(0.0);
    label.add_css_class("title-2");
    label
}

fn section_label(text: &str) -> Label {
    let label = Label::new(Some(text));
    label.set_xalign(0.0);
    label.add_css_class("title-4");
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
    label.add_css_class("dim-label");
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
    let content = GtkBox::new(Orientation::Vertical, 14);
    content.set_margin_top(22);
    content.set_margin_bottom(22);
    content.set_margin_start(22);
    content.set_margin_end(22);
    content
}

fn add_table_header(grid: &Grid, column: i32, text: &str, xalign: f32) {
    let label = Label::new(Some(text));
    label.set_xalign(xalign);
    label.set_hexpand(column == 0);
    label.add_css_class("heading");
    grid.attach(&label, column, 0, 1, 1);
}

fn add_table_cell(grid: &Grid, column: i32, row: i32, text: &str, xalign: f32, expands: bool) {
    let label = Label::new(Some(text));
    label.set_xalign(xalign);
    label.set_hexpand(expands);
    if !expands {
        label.add_css_class("monospace");
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
    fn desktop_azimuth_label_matches_pvgis_convention() {
        assert_eq!(azimuth_direction_label(0.0), "S");
        assert_eq!(azimuth_direction_label(-90.0), "E");
        assert_eq!(azimuth_direction_label(90.0), "W");
        assert_eq!(azimuth_direction_label(180.0), "N");
        assert_eq!(azimuth_direction_label(-180.0), "N");
        assert_eq!(azimuth_direction_label(45.0), "SW");
        assert_eq!(azimuth_direction_label(-45.0), "SE");
    }
}
