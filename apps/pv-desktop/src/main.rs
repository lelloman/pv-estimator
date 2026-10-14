use gtk::gio::prelude::ApplicationExtManual;
use maruzzella::{
    AboutSpec, CommandSpec, LauncherSpec, MaruzzellaConfig, MenuItemSpec, MenuRootSpec,
    PanelResizePolicy, ShellChrome, ShellMode, ShellSpec, TabGroupSpec, TabSpec,
    ToolbarDisplayMode, ToolbarItemSpec, ToolbarPlacement, WorkbenchNodeSpec, WorkspaceSession,
    build_application_with_handle, default_product_spec, layout, load_static_plugin, plugin_tab,
    theme,
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
/// The PV settings view; Maruzzella's Settings page embeds it.
const SETTINGS_VIEW_ID: &str = "com.lelloman.pv_estimator.settings";
/// The base plugin's config stores the palette picked on Maruzzella's
/// Appearance settings page under this key.
const MARUZZELLA_BASE_PLUGIN_ID: &str = "maruzzella.base";
const MARUZZELLA_PALETTE_KEY: &str = "palette";

fn main() {
    migrate_legacy_color_palette();
    let mut product = default_product_spec();
    product.branding.title = "PV Estimator".to_string();
    product.branding.status_text = "PV engineering workbench".to_string();
    product.about = AboutSpec::new(
        env!("CARGO_PKG_VERSION"),
        "Estimates photovoltaic production from coordinates and simple system settings, \
         and simulates how much of it a household consumes. It runs locally, with an \
         embedded city search catalog and source-model bundle, so estimates do not require \
         a hosted service.",
    )
    .with_link("Source code", env!("CARGO_PKG_REPOSITORY"));
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
        menu_item("file-settings", "file", "Settings", "shell.settings"),
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
    // The panels keep the default product's appearances; only their content
    // changes.
    product.layout.left_panel = TabGroupSpec {
        active_tab_id: Some("system".to_string()),
        tabs: vec![plugin_tab(
            "system",
            "panel-left",
            "",
            "com.lelloman.pv_estimator.system",
            "The PV system view could not be created.",
            false,
        )],
        ..product.layout.left_panel
    }
    .with_tab_strip_hidden();
    product.layout.right_panel = TabGroupSpec {
        active_tab_id: Some(DETAILS_TAB_ID.to_string()),
        tabs: vec![details_panel_tab()],
        ..product.layout.right_panel
    }
    .with_tab_strip_hidden();
    product.layout.workbench = WorkbenchNodeSpec::Group(workbench_group(
        Some(ESTIMATE_TAB_ID),
        project_workbench_tabs(),
    ));

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
    application.run();
}

/// Carries the palette picked in the color palette setting of earlier versions
/// over to Maruzzella's Appearance settings, once. "System" is resolved to the
/// palette the desktop prefers right now, since Maruzzella has no palette that
/// follows the desktop.
fn migrate_legacy_color_palette() {
    let Some(legacy) = pv_desktop_plugin::legacy_color_palette() else {
        return;
    };
    let light = match legacy {
        pv_desktop_plugin::LegacyColorPalette::Light => true,
        pv_desktop_plugin::LegacyColorPalette::Dark => false,
        pv_desktop_plugin::LegacyColorPalette::System => !system_prefers_dark_palette(),
    };
    if light && !save_light_palette_choice() {
        // Keep the old setting so the next start can try again.
        return;
    }
    pv_desktop_plugin::forget_legacy_color_palette();
}

/// The check the old "System" option used: a dark GTK theme name.
fn system_prefers_dark_palette() -> bool {
    let named_dark = |name: &str| name.to_ascii_lowercase().contains("dark");
    std::env::var("GTK_THEME")
        .ok()
        .is_some_and(|name| named_dark(&name))
        || (gtk::init().is_ok()
            && gtk::Settings::default()
                .and_then(|settings| settings.gtk_theme_name())
                .is_some_and(|name| named_dark(&name)))
}

/// Records Maruzzella's built-in light palette as the one picked on its
/// Appearance settings page, unless a palette was already picked there.
fn save_light_palette_choice() -> bool {
    let Some(light_id) = theme::builtin_palettes()
        .into_iter()
        .find(|builtin| builtin.palette.is_light())
        .map(|builtin| builtin.id)
    else {
        return false;
    };
    let mut configs = layout::load_plugin_configs(PERSISTENCE_ID);
    match record_palette_choice(&mut configs, light_id) {
        Some(true) => layout::save_plugin_configs(PERSISTENCE_ID, &configs).is_ok(),
        Some(false) => true,
        None => false,
    }
}

/// Stores `palette_id` in the base plugin's config the way Maruzzella's
/// Appearance page does. Returns whether `configs` changed, or `None` when
/// they cannot be safely rewritten.
fn record_palette_choice(configs: &mut layout::PluginConfigs, palette_id: &str) -> Option<bool> {
    if !configs.invalid_entries.is_empty() {
        // Saving would drop the entries Maruzzella could not read.
        return None;
    }
    let entry = configs
        .entries
        .entry(MARUZZELLA_BASE_PLUGIN_ID.to_string())
        .or_default();
    let mut config = if entry.payload.is_empty() {
        serde_json::Map::new()
    } else {
        match serde_json::from_slice(&entry.payload).ok()? {
            serde_json::Value::Object(config) => config,
            _ => return None,
        }
    };
    if config.contains_key(MARUZZELLA_PALETTE_KEY) {
        return Some(false);
    }
    config.insert(
        MARUZZELLA_PALETTE_KEY.to_string(),
        serde_json::Value::String(palette_id.to_string()),
    );
    entry.payload = serde_json::to_vec(&config).ok()?;
    Some(true)
}

fn workspace_chrome() -> ShellChrome {
    ShellChrome {
        show_menu_bar: true,
        show_toolbar: true,
        show_search: false,
        toolbar_placement: ToolbarPlacement::BelowMenu,
    }
}

/// The editor area group, with the appearances of Maruzzella's default
/// workbench.
fn workbench_group(active_tab_id: Option<&str>, tabs: Vec<TabSpec>) -> TabGroupSpec {
    TabGroupSpec::new(WORKBENCH_GROUP_ID, active_tab_id, tabs)
        .with_panel_appearance("workbench")
        .with_tab_strip_appearance("editor")
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
    let mut changed = remove_standalone_settings_tabs(&mut shell.spec.workbench);
    if normalize_empty_workbench_groups(&mut shell.spec.workbench) {
        changed = true;
    }
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

/// Earlier versions opened the PV settings in a tab of their own; they now
/// live in Maruzzella's Settings, so restored layouts drop those tabs.
fn remove_standalone_settings_tabs(node: &mut WorkbenchNodeSpec) -> bool {
    match node {
        WorkbenchNodeSpec::Group(group) => {
            let previous_len = group.tabs.len();
            group
                .tabs
                .retain(|tab| tab.plugin_view_id.as_deref() != Some(SETTINGS_VIEW_ID));
            if group.tabs.len() == previous_len {
                return false;
            }
            let active_is_present = group
                .active_tab_id
                .as_ref()
                .is_some_and(|active| group.tabs.iter().any(|tab| &tab.id == active));
            if !active_is_present {
                group.active_tab_id = group.tabs.first().map(|tab| tab.id.clone());
            }
            true
        }
        WorkbenchNodeSpec::Split { children, .. } => {
            let mut changed = false;
            for child in children {
                changed |= remove_standalone_settings_tabs(child);
            }
            changed
        }
    }
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
        *node = WorkbenchNodeSpec::Group(workbench_group(None, Vec::new()));
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
        .with_tab_strip_appearance("editor"),
    );
    launcher.menu_roots = product.menu_roots.clone();
    launcher.menu_items = product.menu_items.clone();
    launcher.commands = product.commands.clone();
    launcher.toolbar_items = product.toolbar_items.clone();
    // Keep the View root in place: Maruzzella's own View items still land in
    // it, and a removed root would be re-added after Help.
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
        // Maruzzella's defaults for toolbar items contributed by plugins.
        appearance_id: if secondary { "ghost" } else { "primary" }.to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_choice_is_added_next_to_other_base_plugin_settings() {
        let mut configs = layout::PluginConfigs::default();
        configs.entries.insert(
            MARUZZELLA_BASE_PLUGIN_ID.to_string(),
            layout::PluginConfigEntry {
                schema_version: Some(1),
                payload: br#"{"editor_drafts":{}}"#.to_vec(),
            },
        );

        assert_eq!(record_palette_choice(&mut configs, "light"), Some(true));
        let entry = &configs.entries[MARUZZELLA_BASE_PLUGIN_ID];
        let config: serde_json::Value = serde_json::from_slice(&entry.payload).unwrap();
        assert_eq!(config["palette"], "light");
        assert!(config["editor_drafts"].is_object());
        assert_eq!(entry.schema_version, Some(1));
    }

    #[test]
    fn palette_choice_keeps_a_palette_already_picked_in_maruzzella() {
        let mut configs = layout::PluginConfigs::default();
        configs.entries.insert(
            MARUZZELLA_BASE_PLUGIN_ID.to_string(),
            layout::PluginConfigEntry {
                schema_version: Some(1),
                payload: br#"{"palette":"dark"}"#.to_vec(),
            },
        );

        assert_eq!(record_palette_choice(&mut configs, "light"), Some(false));
        let config: serde_json::Value =
            serde_json::from_slice(&configs.entries[MARUZZELLA_BASE_PLUGIN_ID].payload).unwrap();
        assert_eq!(config["palette"], "dark");
    }

    #[test]
    fn palette_choice_is_recorded_in_a_fresh_config() {
        let mut configs = layout::PluginConfigs::default();
        assert_eq!(record_palette_choice(&mut configs, "light"), Some(true));
        let config: serde_json::Value =
            serde_json::from_slice(&configs.entries[MARUZZELLA_BASE_PLUGIN_ID].payload).unwrap();
        assert_eq!(config["palette"], "light");
    }

    #[test]
    fn palette_choice_leaves_unreadable_configs_alone() {
        let mut configs = layout::PluginConfigs::default();
        configs
            .invalid_entries
            .insert("other".to_string(), "unreadable".to_string());
        assert_eq!(record_palette_choice(&mut configs, "light"), None);
    }

    #[test]
    fn standalone_settings_tabs_are_dropped_from_restored_layouts() {
        let settings_tab = plugin_tab(
            "settings",
            WORKBENCH_GROUP_ID,
            "Settings",
            SETTINGS_VIEW_ID,
            "",
            true,
        );
        let mut workbench = WorkbenchNodeSpec::Group(TabGroupSpec::new(
            WORKBENCH_GROUP_ID,
            Some("settings"),
            vec![estimate_workbench_tab(), settings_tab],
        ));

        assert!(remove_standalone_settings_tabs(&mut workbench));
        let WorkbenchNodeSpec::Group(group) = &workbench else {
            panic!("workbench should stay a group");
        };
        assert_eq!(group.tabs.len(), 1);
        assert_eq!(group.active_tab_id.as_deref(), Some(ESTIMATE_TAB_ID));
        assert!(!remove_standalone_settings_tabs(&mut workbench));
    }
}
