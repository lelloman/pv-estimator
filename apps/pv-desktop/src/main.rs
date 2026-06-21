use gtk::gio::prelude::ApplicationExtManual;
use maruzzella::{
    CommandSpec, LauncherSpec, MaruzzellaConfig, MenuItemSpec, MenuRootSpec, PanelResizePolicy,
    ShellChrome, ShellMode, ShellSpec, TabGroupSpec, TabSpec, ToolbarDisplayMode, ToolbarItemSpec,
    ToolbarOptionSpec, WorkbenchNodeSpec, WorkspaceSession, build_application_with_handle,
    default_product_spec, layout, load_static_plugin, plugin_tab,
};

const PERSISTENCE_ID: &str = "pv-estimator-desktop";
const WORKSPACE_SLOT: &str = "workspace";
const WORKBENCH_GROUP_ID: &str = "workbench-main";
const ESTIMATE_TAB_ID: &str = "estimate";
const ESTIMATE_VIEW_ID: &str = "com.lelloman.pv_estimator.estimate";
const SIMULATION_TAB_ID: &str = "simulation";
const SIMULATION_VIEW_ID: &str = "com.lelloman.pv_estimator.simulation";

fn main() {
    let mut product = default_product_spec();
    product.branding.title = "PV Estimator".to_string();
    product.branding.status_text = "PV engineering workbench".to_string();
    product.include_base_toolbar_items = false;
    product.include_base_menu_items = false;
    product.include_base_startup_tabs = false;

    product.menu_roots = vec![
        MenuRootSpec {
            id: "file".to_string(),
            label: "File".to_string(),
        },
        MenuRootSpec {
            id: "project".to_string(),
            label: "Project".to_string(),
        },
        MenuRootSpec {
            id: "help".to_string(),
            label: "Help".to_string(),
        },
    ];
    product.menu_items = vec![
        menu_item("file-new", "file", "New Project", "pv.project.new"),
        menu_item("file-open", "file", "Open Project", "pv.project.open"),
        menu_item("file-close", "file", "Close Project", "pv.project.close"),
        menu_item("file-save", "file", "Save Project", "pv.project.save"),
        menu_item(
            "file-save-as",
            "file",
            "Save Project As",
            "pv.project.save_as",
        ),
        menu_item("file-exit", "file", "Exit", "pv.app.exit"),
        menu_item(
            "project-estimate",
            "project",
            "Run Estimate",
            "pv.project.run_estimate",
        ),
        menu_item(
            "project-simulation",
            "project",
            "Run Simulation",
            "pv.project.run_simulation",
        ),
        menu_item("help-about", "help", "About", "shell.about"),
    ];
    product.commands = vec![
        command("pv.project.new", "New Project"),
        command("pv.project.open", "Open Project"),
        command("pv.project.close", "Close Project"),
        command("pv.project.save", "Save Project"),
        command("pv.project.save_as", "Save Project As"),
        command("pv.project.run_estimate", "Run Estimate"),
        command("pv.project.run_simulation", "Run Simulation"),
        command("pv.project.set_simulation_runs", "Set Simulation Runs"),
        command("pv.app.exit", "Exit"),
    ];
    product.toolbar_items = vec![
        toolbar_item_with_display(
            "save",
            Some("document-save-symbolic"),
            "Save",
            "pv.project.save",
            false,
            ToolbarDisplayMode::IconAndText,
        ),
        toolbar_item(
            "estimate",
            Some("x-office-spreadsheet-symbolic"),
            "Estimate",
            "pv.project.run_estimate",
            true,
        ),
        toolbar_item(
            "simulation",
            Some("media-playback-start-symbolic"),
            "Simulation",
            "pv.project.run_simulation",
            true,
        ),
        toolbar_dropdown_item(
            "simulation-runs",
            "Simulation runs",
            "pv.project.set_simulation_runs",
            &[
                ("1,000 runs", "1000"),
                ("10,000 runs", "10000"),
                ("100,000 runs", "100000"),
                ("1,000,000 runs", "1000000"),
                ("10,000,000 runs", "10000000"),
                ("100,000,000 runs", "100000000"),
                ("1,000,000,000 runs", "1000000000"),
            ],
            1,
            true,
        ),
    ];

    product.layout.left_panel_resize = PanelResizePolicy::Fixed;
    product.layout.right_panel_resize = PanelResizePolicy::Fixed;
    product.layout.bottom_panel_resize = PanelResizePolicy::Fixed;
    product.layout.left_panel = TabGroupSpec::new(
        "panel-left",
        Some("system"),
        vec![plugin_tab(
            "system",
            "panel-left",
            "",
            "com.lelloman.pv_estimator.system",
            "The PV system view could not be created.",
            false,
        )],
    )
    .with_tab_strip_hidden()
    .with_panel_appearance("primary")
    .with_panel_header_appearance("secondary")
    .with_tab_strip_appearance("utility");
    product.layout.right_panel = TabGroupSpec::new("panel-right", None, Vec::new());
    product.layout.bottom_panel = TabGroupSpec::new("panel-bottom", None, Vec::new());
    product.layout.workbench = WorkbenchNodeSpec::Group(
        TabGroupSpec::new("workbench-main", Some("estimate"), project_workbench_tabs())
            .with_panel_appearance("workbench")
            .with_panel_header_appearance("secondary")
            .with_tab_strip_appearance("editor"),
    );

    let has_restorable_session = pv_desktop_plugin::has_restorable_desktop_session();
    if has_restorable_session {
        sync_simulation_runs_toolbar_items(&mut product.toolbar_items);
    }
    let project_shell = product.shell_spec();
    let launcher = no_project_launcher(&product);
    if has_restorable_session {
        repair_empty_workspace_workbench(&project_shell);
    }
    let startup_mode = if has_restorable_session {
        ShellMode::Workspace
    } else {
        ShellMode::Launcher
    };

    let config = MaruzzellaConfig::new("com.lelloman.pv-estimator.desktop")
        .with_persistence_id(PERSISTENCE_ID)
        .with_product(product)
        .with_startup_mode(startup_mode)
        .with_launcher(launcher)
        .with_workspace_chrome(workspace_chrome())
        .with_builtin_plugin(embedded_pv_plugin);

    let (application, handle) = build_application_with_handle(config);
    let workspace_handle = handle.clone();
    let launcher_handle = handle.clone();
    let estimate_handle = handle.clone();
    let simulation_handle = handle.clone();
    let workspace_shell = project_shell.clone();
    let estimate_shell = project_shell.clone();
    let simulation_shell = project_shell.clone();
    pv_desktop_plugin::install_shell_mode_handlers(
        move || {
            switch_to_project_workspace(&workspace_handle, &workspace_shell);
        },
        move || {
            let _ = launcher_handle.switch_to_launcher();
        },
        move || {
            ensure_workbench_tab(&estimate_handle, &estimate_shell, estimate_workbench_tab());
        },
        move || {
            ensure_workbench_tab(
                &simulation_handle,
                &simulation_shell,
                simulation_workbench_tab(),
            );
        },
    );
    application.run();
}

fn workspace_chrome() -> ShellChrome {
    ShellChrome {
        show_menu_bar: true,
        show_toolbar: true,
        show_search: false,
    }
}

fn project_workbench_tabs() -> Vec<TabSpec> {
    vec![estimate_workbench_tab(), simulation_workbench_tab()]
}

fn estimate_workbench_tab() -> TabSpec {
    plugin_tab(
        ESTIMATE_TAB_ID,
        WORKBENCH_GROUP_ID,
        "Estimate",
        ESTIMATE_VIEW_ID,
        "The PV estimate view could not be created.",
        true,
    )
}

fn simulation_workbench_tab() -> TabSpec {
    plugin_tab(
        SIMULATION_TAB_ID,
        WORKBENCH_GROUP_ID,
        "Simulation",
        SIMULATION_VIEW_ID,
        "The PV simulation view could not be created.",
        true,
    )
}

fn switch_to_project_workspace(handle: &maruzzella::MaruzzellaHandle, default_shell: &ShellSpec) {
    let spec = repaired_workspace_spec(default_shell);
    let _ = handle.switch_to_workspace(WorkspaceSession::new(spec));
}

fn repair_empty_workspace_workbench(default_shell: &ShellSpec) {
    let _ = repaired_workspace_spec(default_shell);
}

fn repaired_workspace_spec(default_shell: &ShellSpec) -> ShellSpec {
    let workspace_persistence_id = layout::scoped_persistence_id(PERSISTENCE_ID, WORKSPACE_SLOT);
    let mut shell = layout::load_for_slot(PERSISTENCE_ID, WORKSPACE_SLOT, default_shell);
    let mut changed = normalize_empty_workbench_groups(&mut shell.spec.workbench);
    if ensure_workbench_has_any_tab(&mut shell.spec.workbench) {
        changed = true;
    }
    sync_simulation_runs_toolbar(&mut shell.spec);
    if changed {
        layout::save(&workspace_persistence_id, &shell);
    }
    shell.spec
}

fn sync_simulation_runs_toolbar(spec: &mut ShellSpec) {
    sync_simulation_runs_toolbar_items(&mut spec.toolbar_items);
}

fn sync_simulation_runs_toolbar_items(items: &mut [ToolbarItemSpec]) {
    let selected_index =
        simulation_runs_toolbar_index(pv_desktop_plugin::current_simulation_runs());
    for item in items {
        if item.id == "simulation-runs" {
            item.selected_index = selected_index;
        }
    }
}

fn simulation_runs_toolbar_index(runs: usize) -> u32 {
    match runs {
        1_000 => 0,
        10_000 => 1,
        100_000 => 2,
        1_000_000 => 3,
        10_000_000 => 4,
        100_000_000 => 5,
        1_000_000_000 => 6,
        _ => 1,
    }
}

fn ensure_workbench_has_any_tab(node: &mut WorkbenchNodeSpec) -> bool {
    if workbench_has_tabs(node) {
        false
    } else {
        *node = WorkbenchNodeSpec::Group(
            TabGroupSpec::new(WORKBENCH_GROUP_ID, None, Vec::new())
                .with_panel_appearance("workbench")
                .with_panel_header_appearance("secondary")
                .with_tab_strip_appearance("editor"),
        );
        insert_tab_in_first_workbench_group(node, estimate_workbench_tab())
    }
}

fn normalize_empty_workbench_groups(node: &mut WorkbenchNodeSpec) -> bool {
    normalize_empty_workbench_node(node).1
}

fn normalize_empty_workbench_node(node: &mut WorkbenchNodeSpec) -> (bool, bool) {
    match node {
        WorkbenchNodeSpec::Group(group) => (group.tabs.is_empty(), false),
        WorkbenchNodeSpec::Split { children, .. } => {
            let mut changed = false;
            let mut index = 0usize;
            while index < children.len() {
                let (empty, child_changed) = normalize_empty_workbench_node(&mut children[index]);
                changed |= child_changed;
                if empty {
                    children.remove(index);
                    changed = true;
                } else {
                    index += 1;
                }
            }
            if children.is_empty() {
                (true, changed)
            } else if children.len() == 1 {
                *node = children.remove(0);
                (false, true)
            } else {
                (false, changed)
            }
        }
    }
}

fn workbench_has_tabs(node: &WorkbenchNodeSpec) -> bool {
    match node {
        WorkbenchNodeSpec::Group(group) => !group.tabs.is_empty(),
        WorkbenchNodeSpec::Split { children, .. } => children.iter().any(workbench_has_tabs),
    }
}

fn ensure_workbench_tab(
    handle: &maruzzella::MaruzzellaHandle,
    default_shell: &ShellSpec,
    tab: TabSpec,
) {
    let workspace_persistence_id = layout::scoped_persistence_id(PERSISTENCE_ID, WORKSPACE_SLOT);
    let mut shell = layout::load_for_slot(PERSISTENCE_ID, WORKSPACE_SLOT, default_shell);
    if !ensure_tab_active_in_workbench(&mut shell.spec.workbench, tab) {
        return;
    }

    let spec = shell.spec.clone();
    layout::save(&workspace_persistence_id, &shell);
    let _ = handle.switch_to_workspace(WorkspaceSession::new(spec));
}

fn ensure_tab_active_in_workbench(node: &mut WorkbenchNodeSpec, tab: TabSpec) -> bool {
    if let Some(changed) = activate_existing_workbench_tab(node, &tab) {
        return changed;
    }
    insert_tab_in_first_workbench_group(node, tab)
}

fn activate_existing_workbench_tab(node: &mut WorkbenchNodeSpec, tab: &TabSpec) -> Option<bool> {
    match node {
        WorkbenchNodeSpec::Group(group) => {
            let tab_id = group
                .tabs
                .iter()
                .find(|candidate| tab_matches(candidate, tab))
                .map(|candidate| candidate.id.clone())?;
            if group.active_tab_id.as_deref() == Some(tab_id.as_str()) {
                Some(false)
            } else {
                group.active_tab_id = Some(tab_id);
                Some(true)
            }
        }
        WorkbenchNodeSpec::Split { children, .. } => children
            .iter_mut()
            .find_map(|child| activate_existing_workbench_tab(child, tab)),
    }
}

fn insert_tab_in_first_workbench_group(node: &mut WorkbenchNodeSpec, mut tab: TabSpec) -> bool {
    match node {
        WorkbenchNodeSpec::Group(group) => {
            tab.panel_id = group.id.clone();
            let tab_id = tab.id.clone();
            group.tabs.push(tab);
            group.active_tab_id = Some(tab_id);
            true
        }
        WorkbenchNodeSpec::Split { children, .. } => children
            .iter_mut()
            .any(|child| insert_tab_in_first_workbench_group(child, tab.clone())),
    }
}

fn tab_matches(candidate: &TabSpec, tab: &TabSpec) -> bool {
    candidate.id == tab.id
        || match (
            candidate.plugin_view_id.as_deref(),
            tab.plugin_view_id.as_deref(),
        ) {
            (Some(candidate_view_id), Some(view_id)) => candidate_view_id == view_id,
            _ => false,
        }
}

fn no_project_launcher(product: &maruzzella::ProductSpec) -> LauncherSpec {
    let mut launcher = LauncherSpec::new(
        "PV Estimator",
        TabGroupSpec::new(
            "launcher-main",
            Some("no-project"),
            vec![plugin_tab(
                "no-project",
                "launcher-main",
                "PV Estimator",
                "com.lelloman.pv_estimator.launcher",
                "The PV start view could not be created.",
                false,
            )],
        )
        .with_tab_strip_hidden()
        .with_panel_appearance("workbench")
        .with_panel_header_appearance("secondary")
        .with_tab_strip_appearance("editor"),
    );
    launcher.menu_roots = product.menu_roots.clone();
    launcher.menu_items = product.menu_items.clone();
    launcher.commands = product.commands.clone();
    launcher.toolbar_items = product.toolbar_items.clone();
    launcher.include_base_toolbar_items = product.include_base_toolbar_items;
    launcher.chrome = workspace_chrome();
    launcher
}

fn menu_item(id: &str, root_id: &str, label: &str, command_id: &str) -> MenuItemSpec {
    MenuItemSpec {
        id: id.to_string(),
        root_id: root_id.to_string(),
        label: label.to_string(),
        command_id: command_id.to_string(),
        payload: Vec::new(),
    }
}

fn command(id: &str, title: &str) -> CommandSpec {
    CommandSpec {
        id: id.to_string(),
        title: title.to_string(),
    }
}

fn toolbar_item(
    id: &str,
    icon_name: Option<&str>,
    label: &str,
    command_id: &str,
    secondary: bool,
) -> ToolbarItemSpec {
    toolbar_item_with_display(
        id,
        icon_name,
        label,
        command_id,
        secondary,
        ToolbarDisplayMode::IconOnly,
    )
}

fn toolbar_item_with_display(
    id: &str,
    icon_name: Option<&str>,
    label: &str,
    command_id: &str,
    secondary: bool,
    display_mode: ToolbarDisplayMode,
) -> ToolbarItemSpec {
    ToolbarItemSpec {
        id: id.to_string(),
        icon_name: icon_name.map(str::to_string),
        label: Some(label.to_string()),
        command_id: command_id.to_string(),
        payload: Vec::new(),
        secondary,
        display_mode,
        appearance_id: "toolbar".to_string(),
        options: Vec::new(),
        selected_index: 0,
    }
}

fn toolbar_dropdown_item(
    id: &str,
    label: &str,
    command_id: &str,
    options: &[(&str, &str)],
    selected_index: u32,
    secondary: bool,
) -> ToolbarItemSpec {
    ToolbarItemSpec {
        id: id.to_string(),
        icon_name: None,
        label: Some(label.to_string()),
        command_id: command_id.to_string(),
        payload: Vec::new(),
        secondary,
        display_mode: ToolbarDisplayMode::Dropdown,
        appearance_id: "toolbar".to_string(),
        options: options
            .iter()
            .map(|(label, payload)| ToolbarOptionSpec {
                label: (*label).to_string(),
                payload: payload.as_bytes().to_vec(),
            })
            .collect(),
        selected_index,
    }
}

fn embedded_pv_plugin() -> Result<maruzzella::LoadedPlugin, maruzzella::PluginLoadError> {
    load_static_plugin(
        "builtin:pv-desktop-plugin",
        pv_desktop_plugin::maruzzella_plugin_entry,
    )
}
