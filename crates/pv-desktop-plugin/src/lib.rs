#![allow(deprecated)]

use std::cell::RefCell;
use std::ffi::c_void;
use std::path::PathBuf;

use gtk::glib::translate::IntoGlibPtr;
use gtk::prelude::*;
use gtk::{
    Align, Box as GtkBox, Button, DropDown, Entry, FileChooserAction, FileChooserNative, Grid,
    Image, Label, ListBox, Orientation, PolicyType, ResponseType, ScrolledWindow, SelectionMode,
    Separator, Window,
};
use maruzzella_sdk::{
    CommandSpec, HostApi, MzStatusCode, MzViewPlacement, Plugin, PluginDependency,
    PluginDescriptor, SurfaceContributionSpec, Version, ViewFactorySpec, export_plugin,
};
use pv_core::simulation::{
    BuiltInLoadShapeId, LoadProfile, LoadShape, SimulationRequest, StorageConfig, simulate,
};
use pv_data::{CitySearchResult, search_cities};
use pv_desktop_core::{PROJECT_EXTENSION, PvProjectDocument, load_project, save_project};
use pv_model::{EstimateArray, EstimateRequest, SourceModelEstimator};

pub struct PvDesktopPlugin;

const PLUGIN_ID: &str = "com.lelloman.pv_estimator.desktop";
const VIEW_SYSTEM: &str = "com.lelloman.pv_estimator.system";
const VIEW_ESTIMATE: &str = "com.lelloman.pv_estimator.estimate";
const VIEW_SIMULATION: &str = "com.lelloman.pv_estimator.simulation";

const CMD_NEW: &str = "pv.project.new";
const CMD_OPEN: &str = "pv.project.open";
const CMD_SAVE: &str = "pv.project.save";
const CMD_SAVE_AS: &str = "pv.project.save_as";
const CMD_RUN_ESTIMATE: &str = "pv.project.run_estimate";
const CMD_RUN_SIMULATION: &str = "pv.project.run_simulation";
const CMD_SET_SIMULATION_RUNS: &str = "pv.project.set_simulation_runs";

#[derive(Clone, Debug)]
struct DesktopState {
    project: PvProjectDocument,
    path: Option<PathBuf>,
    dirty: bool,
    status: String,
    log: Vec<String>,
}

impl Default for DesktopState {
    fn default() -> Self {
        Self {
            project: PvProjectDocument::default(),
            path: None,
            dirty: false,
            status: "New PV project".to_string(),
            log: vec!["New PV project".to_string()],
        }
    }
}

thread_local! {
    static STATE: RefCell<DesktopState> = RefCell::new(DesktopState::default());
    static SYSTEM_VIEWS: RefCell<Vec<gtk::glib::WeakRef<GtkBox>>> = const { RefCell::new(Vec::new()) };
    static ESTIMATE_VIEWS: RefCell<Vec<gtk::glib::WeakRef<GtkBox>>> = const { RefCell::new(Vec::new()) };
    static SIMULATION_VIEWS: RefCell<Vec<gtk::glib::WeakRef<GtkBox>>> = const { RefCell::new(Vec::new()) };
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
            CommandSpec::new(PLUGIN_ID, CMD_SAVE, "Save Project")
                .with_handler(command_save_project),
        )?;
        host.register_command(
            CommandSpec::new(PLUGIN_ID, CMD_SAVE_AS, "Save Project As")
                .with_handler(command_save_project_as),
        )?;
        host.register_command(
            CommandSpec::new(PLUGIN_ID, CMD_RUN_ESTIMATE, "Run Estimate")
                .with_handler(command_run_estimate),
        )?;
        host.register_command(
            CommandSpec::new(PLUGIN_ID, CMD_RUN_SIMULATION, "Run Simulation")
                .with_handler(command_run_simulation),
        )?;
        host.register_command(
            CommandSpec::new(PLUGIN_ID, CMD_SET_SIMULATION_RUNS, "Set Simulation Runs")
                .with_handler(command_set_simulation_runs),
        )?;

        host.register_surface_contribution(SurfaceContributionSpec::about_section(
            PLUGIN_ID,
            "pv-desktop-about",
            "PV Estimator Desktop",
            "Engineering workbench for photovoltaic production and consumption simulations.",
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
    STATE.with(|state| {
        *state.borrow_mut() = DesktopState::default();
    });
    append_log("New project created".to_string());
    refresh_views();
    maruzzella_sdk::ffi::MzStatus::OK
}

extern "C" fn command_open_project(
    _payload: maruzzella_sdk::ffi::MzBytes,
) -> maruzzella_sdk::ffi::MzStatus {
    show_open_project_dialog();
    maruzzella_sdk::ffi::MzStatus::OK
}

extern "C" fn command_save_project(
    _payload: maruzzella_sdk::ffi::MzBytes,
) -> maruzzella_sdk::ffi::MzStatus {
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
    show_save_project_dialog();
    maruzzella_sdk::ffi::MzStatus::OK
}

extern "C" fn command_run_estimate(
    _payload: maruzzella_sdk::ffi::MzBytes,
) -> maruzzella_sdk::ffi::MzStatus {
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
    match run_simulation() {
        Ok(()) => maruzzella_sdk::ffi::MzStatus::OK,
        Err(message) => {
            append_log(message);
            refresh_views();
            maruzzella_sdk::ffi::MzStatus::new(MzStatusCode::InternalError)
        }
    }
}

extern "C" fn command_set_simulation_runs(
    payload: maruzzella_sdk::ffi::MzBytes,
) -> maruzzella_sdk::ffi::MzStatus {
    let Some(runs) = simulation_runs_from_payload(payload) else {
        append_log("Invalid simulation run count".to_string());
        refresh_views();
        return maruzzella_sdk::ffi::MzStatus::new(MzStatusCode::InvalidArgument);
    };

    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.project.inputs.simulation_options.runs = runs;
        state.project.results.simulation = None;
        state.dirty = true;
        let message = format!("Simulation runs set to {runs}");
        state.status = message.clone();
        state.log.push(message);
    });
    refresh_views();
    maruzzella_sdk::ffi::MzStatus::OK
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
    append_log("Running source-model estimate".to_string());
    let (request, arrays) = STATE.with(|state| {
        let state = state.borrow();
        (
            state.project.inputs.estimate_request.clone(),
            state.project.inputs.arrays.clone(),
        )
    });
    let mut estimator = SourceModelEstimator::load_embedded()
        .map_err(|error| format!("Failed to load embedded model artifacts: {error:#}"))?;
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
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.project.results.estimate = Some(estimate);
        state.project.results.production_profile = Some(production_profile);
        state.project.results.simulation = None;
        state.dirty = true;
        let status = format!("Estimate complete: {annual_kwh:.0} kWh/year");
        state.status = status.clone();
        state.log.push(status);
    });
    refresh_views();
    Ok(())
}

fn run_simulation() -> Result<(), String> {
    let (production, load, storage, options) = STATE.with(|state| {
        let state = state.borrow();
        (
            state.project.results.production_profile.clone(),
            state.project.inputs.load_profile.clone(),
            state.project.inputs.estimate_request.storage_usable_kwh,
            state.project.inputs.simulation_options,
        )
    });
    let Some(production) = production else {
        return Err("Run an estimate before simulation".to_string());
    };
    append_log(format!("Running simulation with {} runs", options.runs));
    let request = SimulationRequest {
        production,
        load,
        storage: storage.map(|usable_capacity_kwh| StorageConfig {
            usable_capacity_kwh,
        }),
        options,
    };
    let result = simulate(&request).map_err(|error| format!("Simulation failed: {error}"))?;
    let self_sufficiency = result.summaries.self_sufficiency_ratio.p50;
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.project.results.simulation = Some(result);
        state.dirty = true;
        let status = format!(
            "Simulation complete: {:.0}% self sufficiency",
            self_sufficiency * 100.0
        );
        state.status = status.clone();
        state.log.push(status);
    });
    refresh_views();
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SaveDisposition {
    Saved,
    NeedsPath,
}

fn save_current_project() -> Result<SaveDisposition, String> {
    let (path, project) = STATE.with(|state| {
        let state = state.borrow();
        (state.path.clone(), state.project.clone())
    });
    let Some(path) = path else {
        return Ok(SaveDisposition::NeedsPath);
    };
    save_project(&path, &project).map_err(|error| format!("Save failed: {error:#}"))?;
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
    match load_project(&path) {
        Ok(project) => {
            STATE.with(|state| {
                let mut state = state.borrow_mut();
                state.project = project;
                state.path = Some(path.clone());
                state.dirty = false;
                let status = format!("Opened {}", path.display());
                state.status = status.clone();
                state.log.push(status);
            });
        }
        Err(error) => append_log(format!("Open failed: {error:#}")),
    }
    refresh_views();
}

fn save_project_as(path: PathBuf) {
    let path = ensure_project_extension(path);
    let project = STATE.with(|state| state.borrow().project.clone());
    match save_project(&path, &project) {
        Ok(()) => STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.path = Some(path.clone());
            state.dirty = false;
            let status = format!("Saved {}", path.display());
            state.status = status.clone();
            state.log.push(status);
        }),
        Err(error) => append_log(format!("Save failed: {error:#}")),
    }
    refresh_views();
}

fn ensure_project_extension(path: PathBuf) -> PathBuf {
    if path.extension().and_then(|extension| extension.to_str()) == Some(PROJECT_EXTENSION) {
        path
    } else {
        path.with_extension(PROJECT_EXTENSION)
    }
}

fn show_open_project_dialog() {
    if !gtk::is_initialized_main_thread() && gtk::init().is_err() {
        append_log("GTK is not initialized; cannot open file dialog".to_string());
        return;
    }
    let dialog = FileChooserNative::new(
        Some("Open PV Project"),
        None::<&gtk::Window>,
        FileChooserAction::Open,
        Some("Open"),
        Some("Cancel"),
    );
    dialog.connect_response(|dialog, response| {
        if response == ResponseType::Accept
            && let Some(file) = dialog.file()
            && let Some(path) = file.path()
        {
            open_project(path);
        }
        dialog.destroy();
    });
    dialog.show();
}

fn show_save_project_dialog() {
    if !gtk::is_initialized_main_thread() && gtk::init().is_err() {
        append_log("GTK is not initialized; cannot open file dialog".to_string());
        return;
    }
    let dialog = FileChooserNative::new(
        Some("Save PV Project"),
        None::<&gtk::Window>,
        FileChooserAction::Save,
        Some("Save"),
        Some("Cancel"),
    );
    dialog.set_current_name("untitled.pvproj");
    dialog.connect_response(|dialog, response| {
        if response == ResponseType::Accept
            && let Some(file) = dialog.file()
            && let Some(path) = file.path()
        {
            save_project_as(path);
        }
        dialog.destroy();
    });
    dialog.show();
}

extern "C" fn create_system_view(
    _host: *const maruzzella_sdk::ffi::MzHostApi,
    _request: *const maruzzella_sdk::ffi::MzViewRequest,
) -> *mut c_void {
    if !gtk::is_initialized_main_thread() && gtk::init().is_err() {
        return std::ptr::null_mut();
    }
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
    refresh_view_group(&ESTIMATE_VIEWS, render_estimate_into);
    refresh_view_group(&SIMULATION_VIEWS, render_simulation_into);
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

fn render_system_into(root: &GtkBox) {
    clear_box(root);
    let state = snapshot();
    let scroller = ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .hscrollbar_policy(PolicyType::Never)
        .vscrollbar_policy(PolicyType::Automatic)
        .build();
    let content = GtkBox::new(Orientation::Vertical, 14);
    content.set_margin_top(10);
    content.set_margin_bottom(10);
    content.set_margin_start(10);
    content.set_margin_end(10);
    append_location_fields(&content, &state.project.inputs.estimate_request);
    content.append(&section_separator());
    append_array_fields(&content, &state.project);
    content.append(&section_separator());
    append_consumption_fields(&content, &state.project.inputs.load_profile);
    scroller.set_child(Some(&content));
    root.append(&scroller);
}

fn append_location_fields(content: &GtkBox, request: &EstimateRequest) {
    content.append(&section_label("Location"));
    let name = Button::with_label(&request.name);
    name.set_halign(Align::Fill);
    name.connect_clicked(|_| show_location_search_dialog());
    content.append(&field_row("Name", &name));
    let lat = number_entry(request.latitude, 4, |value| {
        update_state(|state| state.project.inputs.estimate_request.latitude = value);
    });
    content.append(&field_row("Latitude", &lat));
    let lon = number_entry(request.longitude, 4, |value| {
        update_state(|state| state.project.inputs.estimate_request.longitude = value);
    });
    content.append(&field_row("Longitude", &lon));
}

fn show_location_search_dialog() {
    if !gtk::is_initialized_main_thread() && gtk::init().is_err() {
        append_log("GTK is not initialized; cannot search locations".to_string());
        return;
    }

    let current_query =
        STATE.with(|state| state.borrow().project.inputs.estimate_request.name.clone());
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
        .hscrollbar_policy(PolicyType::Never)
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
        let request = &mut state.project.inputs.estimate_request;
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
            update_state(|state| {
                state.project.inputs.estimate_request.storage_usable_kwh =
                    (value > 0.0).then_some(value);
            });
        },
    );
    content.append(&field_row("Storage kWh", &storage));

    let loss = STATE.with(|state| state.borrow().project.inputs.estimate_request.loss_pct);
    let loss = number_entry(loss, 1, |value| {
        update_state(|state| state.project.inputs.estimate_request.loss_pct = value);
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
            STATE.with(|state| state.borrow().project.inputs.arrays.get(index).cloned())
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
        match index {
            Some(index) if index < state.project.inputs.arrays.len() => {
                state.project.inputs.arrays[index] = array;
            }
            _ => state.project.inputs.arrays.push(array),
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
            .inputs
            .arrays
            .get(index)
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
        if index >= state.project.inputs.arrays.len() {
            return;
        }
        state.project.inputs.arrays.remove(index);
        sync_request_from_arrays(state);
    });
}

fn sync_request_from_arrays(state: &mut DesktopState) {
    if let Some(first) = state.project.inputs.arrays.first() {
        state.project.inputs.estimate_request.peak_power_kwp = first.peak_power_kwp;
        state.project.inputs.estimate_request.tilt_deg = first.tilt_deg;
        state.project.inputs.estimate_request.azimuth_deg = first.azimuth_deg;
    }
}

fn append_consumption_fields(content: &GtkBox, load_profile: &LoadProfile) {
    content.append(&section_label("Consumption"));
    let annual = match load_profile {
        LoadProfile::AnnualKwh { annual_kwh, .. } => *annual_kwh,
        LoadProfile::DailyKwh { daily_kwh, .. } => *daily_kwh * 365.0,
    };
    let annual_entry = number_entry(annual, 0, |value| {
        update_state(|state| {
            let shape = load_shape(&state.project.inputs.load_profile);
            state.project.inputs.load_profile = LoadProfile::AnnualKwh {
                annual_kwh: value,
                shape,
            };
        });
    });
    content.append(&field_row("Annual kWh", &annual_entry));
    let shape = load_shape(load_profile);
    let dropdown = DropDown::from_strings(&["Residential", "Flat", "Daytime", "Evening"]);
    dropdown.set_selected(shape_index(&shape));
    dropdown.connect_selected_notify(|dropdown| {
        let next_shape = LoadShape::BuiltIn {
            shape_id: shape_from_index(dropdown.selected()),
        };
        update_state(|state| {
            let annual_kwh = match state.project.inputs.load_profile {
                LoadProfile::AnnualKwh { annual_kwh, .. } => annual_kwh,
                LoadProfile::DailyKwh { daily_kwh, .. } => daily_kwh * 365.0,
            };
            state.project.inputs.load_profile = LoadProfile::AnnualKwh {
                annual_kwh,
                shape: next_shape,
            };
        });
    });
    content.append(&field_row("Shape", &dropdown));
}

fn render_estimate_into(root: &GtkBox) {
    clear_box(root);
    let state = snapshot();
    let scroller = workbench_scroller();
    let content = workbench_content();
    content.append(&header_label("Estimate"));
    content.append(&meta_label(&state.status));
    content.append(&section_separator());
    if let Some(estimate) = &state.project.results.estimate {
        let annual = estimate
            .ensemble_estimate
            .annual_energy
            .mean
            .as_kilowatt_hours();
        content.append(&metric_label(
            "Annual production",
            &format!("{annual:.0} kWh"),
        ));
        content.append(&metric_label(
            "Sources",
            &estimate
                .coverage
                .applicable_sources
                .iter()
                .map(|source| source.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        ));
        let grid = Grid::new();
        grid.set_column_spacing(12);
        grid.set_row_spacing(4);
        add_grid_header(&grid, 0, "Month");
        add_grid_header(&grid, 1, "Mean kWh");
        add_grid_header(&grid, 2, "Low kWh");
        add_grid_header(&grid, 3, "High kWh");
        for (row, monthly) in estimate
            .ensemble_estimate
            .monthly_estimates
            .iter()
            .enumerate()
        {
            let row = (row + 1) as i32;
            add_grid_text(&grid, 0, row, &monthly.month.value().to_string());
            add_grid_text(
                &grid,
                1,
                row,
                &format!("{:.0}", monthly.energy.mean.as_kilowatt_hours()),
            );
            add_grid_text(
                &grid,
                2,
                row,
                &format!("{:.0}", monthly.energy.low.as_kilowatt_hours()),
            );
            add_grid_text(
                &grid,
                3,
                row,
                &format!("{:.0}", monthly.energy.high.as_kilowatt_hours()),
            );
        }
        content.append(&grid);
    } else {
        content.append(&body_label(
            "No estimate yet. Run Estimate from the toolbar.",
        ));
    }
    content.append(&section_separator());
    append_log_tail(&content, &state.log);
    scroller.set_child(Some(&content));
    root.append(&scroller);
}

fn render_simulation_into(root: &GtkBox) {
    clear_box(root);
    let state = snapshot();
    let scroller = workbench_scroller();
    let content = workbench_content();
    content.append(&header_label("Simulation"));
    content.append(&meta_label(&state.status));
    content.append(&section_separator());
    if let Some(result) = &state.project.results.simulation {
        content.append(&metric_label(
            "Completed runs",
            &format!("{} / {}", result.completed_runs, result.requested_runs),
        ));
        content.append(&metric_label(
            "Self consumption",
            &percent(result.summaries.self_consumption_ratio.p50),
        ));
        content.append(&metric_label(
            "Self sufficiency",
            &percent(result.summaries.self_sufficiency_ratio.p50),
        ));
        content.append(&metric_label(
            "Grid import",
            &format!("{:.0} kWh", result.summaries.grid_import_kwh.p50),
        ));
        content.append(&metric_label(
            "Grid export",
            &format!("{:.0} kWh", result.summaries.grid_export_kwh.p50),
        ));
    } else {
        content.append(&body_label(
            "No simulation yet. Run an estimate first, then Run Simulation.",
        ));
    }
    content.append(&section_separator());
    append_log_tail(&content, &state.log);
    scroller.set_child(Some(&content));
    root.append(&scroller);
}

fn update_state(update: impl FnOnce(&mut DesktopState)) {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        update(&mut state);
        state.dirty = true;
        state.project.results.estimate = None;
        state.project.results.production_profile = None;
        state.project.results.simulation = None;
        state.status = "Project inputs changed".to_string();
    });
    refresh_views();
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
    let normalized = value.rem_euclid(360.0);
    let index = ((normalized + 22.5) / 45.0).floor() as usize % DIRECTIONS.len();
    DIRECTIONS[index].to_string()
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

fn metric_label(label: &str, value: &str) -> GtkBox {
    let row = GtkBox::new(Orientation::Horizontal, 12);
    row.set_hexpand(true);
    let left = Label::new(Some(label));
    left.set_xalign(0.0);
    left.set_hexpand(true);
    let right = Label::new(Some(value));
    right.set_xalign(1.0);
    right.add_css_class("monospace");
    row.append(&left);
    row.append(&right);
    row
}

fn section_separator() -> Separator {
    Separator::new(Orientation::Horizontal)
}

fn workbench_scroller() -> ScrolledWindow {
    ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .hscrollbar_policy(PolicyType::Never)
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

fn append_log_tail(content: &GtkBox, log: &[String]) {
    content.append(&section_label("Log"));
    for entry in log.iter().rev().take(8) {
        content.append(&meta_label(entry));
    }
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

fn add_grid_header(grid: &Grid, column: i32, text: &str) {
    let label = Label::new(Some(text));
    label.set_xalign(0.0);
    label.add_css_class("heading");
    grid.attach(&label, column, 0, 1, 1);
}

fn add_grid_text(grid: &Grid, column: i32, row: i32, text: &str) {
    let label = Label::new(Some(text));
    label.set_xalign(if column == 0 { 0.0 } else { 1.0 });
    if column != 0 {
        label.add_css_class("monospace");
    }
    grid.attach(&label, column, row, 1, 1);
}

fn percent(value: f64) -> String {
    format!("{:.0}%", value * 100.0)
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
