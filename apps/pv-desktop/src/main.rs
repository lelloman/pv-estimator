use gtk::gio::prelude::ApplicationExtManual;
use gtk::prelude::*;
use maruzzella::{
    CommandSpec, LauncherSpec, MaruzzellaConfig, MenuItemSpec, MenuRootSpec, PanelResizePolicy,
    ShellChrome, ShellMode, ShellSpec, TabGroupSpec, TabSpec, ThemePalette, ThemeSpec,
    ToolbarDisplayMode, ToolbarItemSpec, ToolbarPlacement, WorkbenchNodeSpec, WorkspaceSession,
    build_application_with_handle, default_product_spec, layout, load_static_plugin, plugin_tab,
};
use maruzzella::{MzContextActivationPolicy, MzSurfaceRole};

const PERSISTENCE_ID: &str = "pv-estimator-desktop";
const WORKSPACE_SLOT: &str = "workspace";
const WORKBENCH_GROUP_ID: &str = "workbench-main";
const ESTIMATE_TAB_ID: &str = "estimate";
const ESTIMATE_VIEW_ID: &str = "com.lelloman.pv_estimator.estimate";
const SIMULATION_TAB_ID: &str = "simulation";
const SIMULATION_VIEW_ID: &str = "com.lelloman.pv_estimator.simulation";
const DETAILS_TAB_ID: &str = "details";
const DETAILS_VIEW_ID: &str = "com.lelloman.pv_estimator.details";
const SETTINGS_VIEW_ID: &str = "com.lelloman.pv_estimator.settings";

fn main() {
    let initial_palette = pv_desktop_plugin::current_color_palette();
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
            id: "view".to_string(),
            label: "View".to_string(),
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
        menu_item("file-exit-separator", "file", "", ""),
        menu_item_with_payload(
            "file-settings",
            "file",
            "Settings",
            "shell.settings",
            SETTINGS_VIEW_ID.as_bytes(),
        ),
        menu_item("file-exit", "file", "Exit", "pv.app.exit"),
        menu_item_with_payload(
            "view-estimate",
            "view",
            "Estimate",
            "shell.settings",
            ESTIMATE_VIEW_ID.as_bytes(),
        ),
        menu_item_with_payload(
            "view-simulation",
            "view",
            "Simulation",
            "shell.settings",
            SIMULATION_VIEW_ID.as_bytes(),
        ),
        menu_item("help-about", "help", "About", "shell.about"),
    ];
    product.commands = vec![
        command("pv.project.new", "New Project"),
        command("pv.project.open", "Open Project"),
        command("pv.project.close", "Close Project"),
        command("pv.project.save", "Save Project"),
        command("pv.project.save_as", "Save Project As"),
        command("shell.settings", "Settings"),
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
        toolbar_view_item(
            "view-estimate",
            "x-office-spreadsheet-symbolic",
            "Estimate",
            ESTIMATE_VIEW_ID,
        ),
        toolbar_view_item(
            "view-simulation",
            "media-playback-start-symbolic",
            "Simulation",
            SIMULATION_VIEW_ID,
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
    product.layout.right_panel = TabGroupSpec::new(
        "panel-right",
        Some(DETAILS_TAB_ID),
        vec![details_panel_tab()],
    )
    .with_tab_strip_hidden()
    .with_panel_appearance("primary")
    .with_panel_header_appearance("secondary")
    .with_tab_strip_appearance("utility");
    product.layout.bottom_panel = TabGroupSpec::new("panel-bottom", None, Vec::new());
    product.layout.workbench = WorkbenchNodeSpec::Group(
        TabGroupSpec::new("workbench-main", Some("estimate"), project_workbench_tabs())
            .with_panel_appearance("workbench")
            .with_panel_header_appearance("secondary")
            .with_tab_strip_appearance("editor"),
    );

    let has_restorable_session = pv_desktop_plugin::has_restorable_desktop_session();
    let project_shell = product.shell_spec();
    let startup_workspace_shell = if has_restorable_session {
        Some(repaired_workspace_spec(&project_shell))
    } else {
        None
    };
    seed_initial_details_view(startup_workspace_shell.as_ref().unwrap_or(&project_shell));
    let launcher = no_project_launcher(&product);
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
        .with_theme(theme_for_palette(initial_palette))
        .with_builtin_plugin(embedded_pv_plugin);

    let (application, handle) = build_application_with_handle(config);
    let workspace_handle = handle.clone();
    let launcher_handle = handle.clone();
    let workspace_shell = project_shell.clone();
    pv_desktop_plugin::install_shell_mode_handlers(
        move || {
            switch_to_project_workspace(&workspace_handle, &workspace_shell);
        },
        move || {
            let _ = launcher_handle.switch_to_launcher();
        },
    );
    pv_desktop_plugin::install_color_palette_handler(apply_color_palette);
    application
        .connect_activate(|_| apply_color_palette(pv_desktop_plugin::current_color_palette()));
    application.run();
}

fn apply_color_palette(palette: pv_desktop_plugin::ColorPalettePreference) {
    let dark = match palette {
        pv_desktop_plugin::ColorPalettePreference::System => system_prefers_dark_palette(),
        pv_desktop_plugin::ColorPalettePreference::Light => false,
        pv_desktop_plugin::ColorPalettePreference::Dark => true,
    };
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(dark);
    }
    maruzzella::theme::install(if dark {
        ThemeSpec::default()
    } else {
        light_theme()
    });
}

fn theme_for_palette(palette: pv_desktop_plugin::ColorPalettePreference) -> ThemeSpec {
    match palette {
        pv_desktop_plugin::ColorPalettePreference::Light => light_theme(),
        pv_desktop_plugin::ColorPalettePreference::System
        | pv_desktop_plugin::ColorPalettePreference::Dark => ThemeSpec::default(),
    }
}

fn system_prefers_dark_palette() -> bool {
    gtk::Settings::default()
        .and_then(|settings| settings.gtk_theme_name())
        .is_some_and(|name| name.to_ascii_lowercase().contains("dark"))
        || std::env::var("GTK_THEME")
            .ok()
            .is_some_and(|name| name.to_ascii_lowercase().contains("dark"))
}

fn light_theme() -> ThemeSpec {
    ThemeSpec {
        palette: ThemePalette {
            bg_0: "#f4f6f8".to_string(),
            bg_1: "#ffffff".to_string(),
            workbench: "#ffffff".to_string(),
            panel_left: "#edf1f5".to_string(),
            panel_right: "#edf1f5".to_string(),
            panel_bottom: "#edf1f5".to_string(),
            border: "#d3d9e1".to_string(),
            border_strong: "#b8c1cc".to_string(),
            text_0: "#171a1f".to_string(),
            text_1: "#38414c".to_string(),
            text_2: "#687482".to_string(),
            accent: "#2463a7".to_string(),
            accent_strong: "#174c82".to_string(),
        },
        ..ThemeSpec::default()
    }
}

fn workspace_chrome() -> ShellChrome {
    ShellChrome {
        show_menu_bar: true,
        show_toolbar: true,
        show_search: false,
        toolbar_placement: ToolbarPlacement::BelowMenu,
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
        false,
    )
}

fn simulation_workbench_tab() -> TabSpec {
    plugin_tab(
        SIMULATION_TAB_ID,
        WORKBENCH_GROUP_ID,
        "Simulation",
        SIMULATION_VIEW_ID,
        "The PV simulation view could not be created.",
        false,
    )
}

fn details_panel_tab() -> TabSpec {
    plugin_tab(
        DETAILS_TAB_ID,
        "panel-right",
        "Details",
        DETAILS_VIEW_ID,
        "The PV details view could not be created.",
        false,
    )
    .with_surface_role(MzSurfaceRole::Inspector)
    .with_context_activation(MzContextActivationPolicy::Never)
}

fn switch_to_project_workspace(handle: &maruzzella::MaruzzellaHandle, default_shell: &ShellSpec) {
    let spec = repaired_workspace_spec(default_shell);
    let _ = handle.switch_to_workspace(WorkspaceSession::new(spec));
}

fn repaired_workspace_spec(default_shell: &ShellSpec) -> ShellSpec {
    let workspace_persistence_id = layout::scoped_persistence_id(PERSISTENCE_ID, WORKSPACE_SLOT);
    let mut shell = layout::load_for_slot(PERSISTENCE_ID, WORKSPACE_SLOT, default_shell);
    let mut changed = normalize_empty_workbench_groups(&mut shell.spec.workbench);
    if ensure_workbench_has_any_tab(&mut shell.spec.workbench) {
        changed = true;
    }
    if ensure_workbench_tab_present(&mut shell.spec.workbench, estimate_workbench_tab()) {
        changed = true;
    }
    if ensure_workbench_tab_present(&mut shell.spec.workbench, simulation_workbench_tab()) {
        changed = true;
    }
    if ensure_right_panel_detail_tab(&mut shell.spec.right_panel) {
        changed = true;
    }
    if remove_legacy_toolbar_items(&mut shell.spec.toolbar_items) {
        changed = true;
    }
    if changed {
        layout::save(&workspace_persistence_id, &shell);
    }
    shell.spec
}

fn seed_initial_details_view(shell: &ShellSpec) {
    if let Some(view_id) = active_workbench_plugin_view_id(&shell.workbench) {
        pv_desktop_plugin::set_initial_details_view(view_id);
    }
}

fn active_workbench_plugin_view_id(node: &WorkbenchNodeSpec) -> Option<&'static str> {
    match node {
        WorkbenchNodeSpec::Group(group) => {
            let tab_id = group.active_tab_id.as_deref()?;
            let tab = group.tabs.iter().find(|tab| tab.id == tab_id)?;
            match tab.plugin_view_id.as_deref() {
                Some(ESTIMATE_VIEW_ID) => Some(ESTIMATE_VIEW_ID),
                Some(SIMULATION_VIEW_ID) => Some(SIMULATION_VIEW_ID),
                _ => None,
            }
        }
        WorkbenchNodeSpec::Split { children, .. } => {
            children.iter().find_map(active_workbench_plugin_view_id)
        }
    }
}

fn remove_legacy_toolbar_items(items: &mut Vec<ToolbarItemSpec>) -> bool {
    let previous_len = items.len();
    items.retain(|item| {
        item.id != "estimate" && item.id != "simulation" && item.id != "simulation-runs"
    });
    items.len() != previous_len
}

fn ensure_right_panel_detail_tab(group: &mut TabGroupSpec) -> bool {
    if let Some(tab_id) = group
        .tabs
        .iter()
        .find(|tab| {
            tab.id == DETAILS_TAB_ID || tab.plugin_view_id.as_deref() == Some(DETAILS_VIEW_ID)
        })
        .map(|tab| tab.id.clone())
    {
        group.active_tab_id = Some(tab_id);
        false
    } else {
        group.tabs.push(details_panel_tab());
        group.active_tab_id = Some(DETAILS_TAB_ID.to_string());
        true
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

fn ensure_workbench_tab_present(node: &mut WorkbenchNodeSpec, tab: TabSpec) -> bool {
    match node {
        WorkbenchNodeSpec::Group(group) => {
            if let Some(existing) = group.tabs.iter_mut().find(|candidate| {
                candidate.id == tab.id
                    || candidate.plugin_view_id.as_deref() == tab.plugin_view_id.as_deref()
            }) {
                let changed = existing.closable != tab.closable;
                existing.closable = tab.closable;
                changed
            } else {
                insert_tab_in_first_workbench_group(node, tab)
            }
        }
        WorkbenchNodeSpec::Split { children, .. } => {
            for child in children.iter_mut() {
                if workbench_contains_tab(child, &tab) {
                    return ensure_workbench_tab_present(child, tab);
                }
            }
            insert_tab_in_first_workbench_group(node, tab)
        }
    }
}

fn workbench_contains_tab(node: &WorkbenchNodeSpec, tab: &TabSpec) -> bool {
    match node {
        WorkbenchNodeSpec::Group(group) => group.tabs.iter().any(|candidate| {
            candidate.id == tab.id
                || candidate.plugin_view_id.as_deref() == tab.plugin_view_id.as_deref()
        }),
        WorkbenchNodeSpec::Split { children, .. } => children
            .iter()
            .any(|child| workbench_contains_tab(child, tab)),
    }
}

fn insert_tab_in_first_workbench_group(node: &mut WorkbenchNodeSpec, mut tab: TabSpec) -> bool {
    match node {
        WorkbenchNodeSpec::Group(group) => {
            tab.panel_id = group.id.clone();
            let tab_id = tab.id.clone();
            group.tabs.push(tab);
            if group.active_tab_id.is_none() {
                group.active_tab_id = Some(tab_id);
            }
            true
        }
        WorkbenchNodeSpec::Split { children, .. } => children
            .iter_mut()
            .any(|child| insert_tab_in_first_workbench_group(child, tab.clone())),
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
    launcher.menu_roots.retain(|root| root.id != "view");
    launcher.menu_items.retain(|item| item.root_id != "view");
    launcher
        .toolbar_items
        .retain(|item| item.id != "view-estimate" && item.id != "view-simulation");
    launcher.include_base_toolbar_items = product.include_base_toolbar_items;
    launcher.chrome = workspace_chrome();
    launcher
}

fn toolbar_view_item(id: &str, icon_name: &str, label: &str, view_id: &str) -> ToolbarItemSpec {
    let mut item = toolbar_item_with_display(
        id,
        Some(icon_name),
        label,
        "shell.settings",
        true,
        ToolbarDisplayMode::IconAndText,
    );
    item.payload = view_id.as_bytes().to_vec();
    item
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

fn menu_item_with_payload(
    id: &str,
    root_id: &str,
    label: &str,
    command_id: &str,
    payload: &[u8],
) -> MenuItemSpec {
    let mut item = menu_item(id, root_id, label, command_id);
    item.payload = payload.to_vec();
    item
}

fn command(id: &str, title: &str) -> CommandSpec {
    CommandSpec {
        id: id.to_string(),
        title: title.to_string(),
    }
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

fn embedded_pv_plugin() -> Result<maruzzella::LoadedPlugin, maruzzella::PluginLoadError> {
    load_static_plugin(
        "builtin:pv-desktop-plugin",
        pv_desktop_plugin::maruzzella_plugin_entry,
    )
}
