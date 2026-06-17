use gtk::gio::prelude::ApplicationExtManual;
use maruzzella::{
    CommandSpec, LauncherSpec, MaruzzellaConfig, MenuItemSpec, MenuRootSpec, PanelResizePolicy,
    ShellChrome, ShellMode, TabGroupSpec, ToolbarDisplayMode, ToolbarItemSpec, ToolbarOptionSpec,
    WorkbenchNodeSpec, WorkspaceSession, build_application_with_handle, default_product_spec,
    load_static_plugin, plugin_tab,
};

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
                ("1_000 runs", "1000"),
                ("10_000 runs", "10000"),
                ("100_000 runs", "100000"),
                ("1_000_000 runs", "1000000"),
                ("10_000_000 runs", "10000000"),
                ("100_000_000 runs", "100000000"),
                ("1_000_000_000 runs", "1000000000"),
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
        TabGroupSpec::new(
            "workbench-main",
            Some("estimate"),
            vec![
                plugin_tab(
                    "estimate",
                    "workbench-main",
                    "Estimate",
                    "com.lelloman.pv_estimator.estimate",
                    "The PV estimate view could not be created.",
                    false,
                ),
                plugin_tab(
                    "simulation",
                    "workbench-main",
                    "Simulation",
                    "com.lelloman.pv_estimator.simulation",
                    "The PV simulation view could not be created.",
                    false,
                ),
            ],
        )
        .with_panel_appearance("workbench")
        .with_panel_header_appearance("secondary")
        .with_tab_strip_appearance("editor"),
    );

    let project_shell = product.shell_spec();
    let launcher = no_project_launcher(&product);
    let startup_mode = if pv_desktop_plugin::has_restorable_desktop_session() {
        ShellMode::Workspace
    } else {
        ShellMode::Launcher
    };

    let config = MaruzzellaConfig::new("com.lelloman.pv-estimator.desktop")
        .with_persistence_id("pv-estimator-desktop")
        .with_product(product)
        .with_startup_mode(startup_mode)
        .with_launcher(launcher)
        .with_workspace_chrome(workspace_chrome())
        .with_builtin_plugin(embedded_pv_plugin);

    let (application, handle) = build_application_with_handle(config);
    let workspace_handle = handle.clone();
    let launcher_handle = handle.clone();
    pv_desktop_plugin::install_shell_mode_handlers(
        move || {
            let _ =
                workspace_handle.switch_to_workspace(WorkspaceSession::new(project_shell.clone()));
        },
        move || {
            let _ = launcher_handle.switch_to_launcher();
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
    ToolbarItemSpec {
        id: id.to_string(),
        icon_name: icon_name.map(str::to_string),
        label: Some(label.to_string()),
        command_id: command_id.to_string(),
        payload: Vec::new(),
        secondary,
        display_mode: ToolbarDisplayMode::IconOnly,
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
