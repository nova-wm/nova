use kdl::KdlDocument;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Debug)]
pub struct MonitorConfig {
    pub name: String,
    pub x: i32,
    pub y: i32,
}

/// Match + actions for a newly mapped window (Hyprland-style window rules).
///
#[derive(Clone, Debug, Default)]
pub struct WindowRule {
    pub app_id: String,
    pub title: String,
    pub float: bool,
    pub opacity: Option<f32>,
    pub workspace: Option<u8>,
    pub center: bool,
}

/// One parsed keybinding from the `keybinds` config block.
#[derive(Clone, Debug)]
pub struct Keybind {
    /// Modifier tokens, e.g. `["mod"]` or `["mod", "shift"]`.
    /// `"mod"` resolves to the top-level `modifier` setting.
    pub modifiers: Vec<String>,
    /// The xkbkeysym name as written in the config, e.g. "Return", "c", "grave".
    pub key: String,
    /// Parsed keysym; `None` if the name was unknown (binding is inert).
    pub key_sym: Option<smithay::input::keyboard::Keysym>,
    /// For `bindm` entries: the Linux input-event-code button, e.g. 272 = left click.
    pub mouse_button: Option<u32>,
    /// The action name, e.g. "focus", "spawn_terminal", "toggle_scratchpad".
    pub action: String,
    /// Extra action arguments, e.g. `["left"]` for `focus left`.
    pub args: Vec<String>,
}

impl Keybind {
    /// Parse `spec` like `"mod+shift+h"` or `"mod+mouse:272"` and an action
    /// string like `"focus left"` into a binding.
    pub fn parse(spec: &str, action: &str) -> Option<Self> {
        let parts: Vec<&str> = spec.split('+').filter(|p| !p.is_empty()).collect();
        let combos = *parts.last()?;
        let modifiers: Vec<String> = parts[..parts.len().saturating_sub(1)]
            .iter()
            .map(|s| s.to_string())
            .collect();

        let mouse_button = combos
            .strip_prefix("mouse:")
            .and_then(|b| b.parse::<u32>().ok());
        let key = combos.strip_prefix("mouse:").unwrap_or(combos).to_string();

        let key_sym = {
            use xkbcommon::xkb::keysym_from_name;
            let sym = keysym_from_name(&key, xkbcommon::xkb::KEYSYM_NO_FLAGS);
            (sym != xkbcommon::xkb::Keysym::default()).then_some(sym)
        };

        let mut action_words = action.split_whitespace();
        let action_name = action_words.next().unwrap_or("").to_string();
        let args: Vec<String> = action_words.map(|s| s.to_string()).collect();

        Some(Self {
            modifiers,
            key,
            key_sym,
            mouse_button,
            action: action_name,
            args,
        })
    }

    /// True if this is a mouse (mod+drag) binding.
    pub fn is_mouse(&self) -> bool {
        self.mouse_button.is_some()
    }
}

/// The keybinds used when the config has no `keybinds` block.
pub fn default_keybindings() -> Vec<Keybind> {
    [
        ("mod+Return", "spawn_terminal"),
        ("mod+c", "close_focused"),
        ("mod+h", "focus left"),
        ("mod+l", "focus right"),
        ("mod+j", "focus down"),
        ("mod+k", "focus up"),
        ("mod+shift+h", "move_list previous"),
        ("mod+shift+l", "move_list next"),
        ("mod+shift+j", "move_row next"),
        ("mod+shift+k", "move_row previous"),
        ("mod+comma", "consume_into_column"),
        ("mod+period", "expel_from_column"),
        ("mod+o", "toggle_overview"),
        ("mod+shift+r", "toggle_record"),
        ("mod+1", "workspace 1"),
        ("mod+2", "workspace 2"),
        ("mod+3", "workspace 3"),
        ("mod+4", "workspace 4"),
        ("mod+5", "workspace 5"),
        ("mod+6", "workspace 6"),
        ("mod+7", "workspace 7"),
        ("mod+8", "workspace 8"),
        ("mod+9", "workspace 9"),
        ("mod+x", "canvas_center_window"),
        ("mod+w", "canvas_zoom_to_fit"),
        ("mod+shift+Left", "canvas_nudge left"),
        ("mod+shift+Right", "canvas_nudge right"),
        ("mod+shift+Up", "canvas_nudge up"),
        ("mod+shift+Down", "canvas_nudge down"),
        ("mod+ctrl+Left", "canvas_pan left"),
        ("mod+ctrl+Right", "canvas_pan right"),
        ("mod+ctrl+Up", "canvas_pan up"),
        ("mod+ctrl+Down", "canvas_pan down"),
        ("mod+t", "toggle_pin"),
        ("mod+a", "canvas_home"),
        ("mod+Tab", "cycle_focus next"),
        ("mod+shift+Tab", "cycle_focus previous"),
        ("mod+grave", "toggle_scratchpad"),
        ("mod+F1", "welcome"),
        ("mod+f", "fullscreen"),
        ("mod+v", "toggle_float"),
        ("mod+equal", "canvas_zoom_in"),
        ("mod+minus", "canvas_zoom_out"),
        ("mod+0", "canvas_zoom_reset"),
        ("mod+mouse:272", "move_window"),
        ("mod+mouse:273", "resize_window"),
    ]
    .iter()
    .filter_map(|(spec, action)| Keybind::parse(spec, action))
    .collect()
}

#[derive(Clone, Debug)]
pub struct Config {
    pub inner_gap: i32,
    pub outer_gap: i32,
    pub modifier: String,
    pub animation_enabled: bool,
    pub animation_duration: f64,
    pub animation_bezier: (f64, f64, f64, f64),
    pub animation_open: String,
    pub focus_follows_mouse: bool,
    pub rounding: i32,
    pub border_width: i32,
    pub active_border_color: [f32; 4],
    pub inactive_border_color: [f32; 4],
    pub border_effect: String,
    pub border_effect_speed: f32,
    pub border_gradient_color: [f32; 4],
    pub blur: bool,
    pub opacity: f32,
    pub layout: String,
    pub scroll_speed: i32,
    pub dwindle_split_ratio: f32,
    pub dwindle_split_direction: String,
    pub startup_apps: Vec<String>,
    pub terminal: String,
    pub background: String,
    pub xcursor_theme: String,
    pub xcursor_size: i32,
    pub xkb_rules: String,
    pub xkb_model: String,
    pub xkb_layout: String,
    pub xkb_variant: String,
    pub xkb_options: String,
    pub touchpad_tap: bool,
    pub touchpad_natural_scroll: bool,
    pub touchpad_accel_profile: String,
    pub touchpad_accel_speed: f64,
    pub touchpad_click_method: String,
    pub touchpad_dwt: bool,
    pub mouse_natural_scroll: bool,
    pub mouse_accel_profile: String,
    pub mouse_accel_speed: f64,
    pub welcome: bool,
    pub focus_opacity: f32,
    pub grain: bool,
    pub grain_intensity: f32,
    pub monitor_configs: Vec<MonitorConfig>,
    pub window_rules: Vec<WindowRule>,
    pub keybindings: Vec<Keybind>,
    /// Merge state (not a config key): set once a file's `keybinds`
    /// block replaces the built-in defaults, so later include files merge
    pub keybindings_from_file: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            inner_gap: 10,
            outer_gap: 15,
            modifier: "alt".to_string(),
            animation_enabled: true,
            animation_duration: 300.0,
            animation_bezier: (0.25, 0.1, 0.25, 1.0),
            animation_open: "scale_up".to_string(),
            focus_follows_mouse: true,
            rounding: 12,
            border_width: 2,
            active_border_color: [1.0, 0.0, 0.498, 1.0],
            inactive_border_color: [0.266, 0.266, 0.266, 1.0],
            border_effect: "none".to_string(),
            border_effect_speed: 1.0,
            border_gradient_color: [0.0, 0.85, 1.0, 1.0],
            blur: true,
            opacity: 0.85,
            layout: "dwindle".to_string(),
            scroll_speed: 400,
            dwindle_split_ratio: 0.5,
            dwindle_split_direction: "auto".to_string(),
            startup_apps: vec![],
            terminal: "foot".to_string(),
            background: "wallpaper.png".to_string(),
            xcursor_theme: String::new(),
            xcursor_size: 24,
            xkb_rules: String::new(),
            xkb_model: String::new(),
            xkb_layout: String::new(),
            xkb_variant: String::new(),
            xkb_options: String::new(),
            touchpad_tap: false,
            touchpad_natural_scroll: false,
            touchpad_accel_profile: "adaptive".to_string(),
            touchpad_accel_speed: 0.0,
            touchpad_click_method: "button-areas".to_string(),
            touchpad_dwt: true,
            mouse_natural_scroll: false,
            mouse_accel_profile: "adaptive".to_string(),
            mouse_accel_speed: 0.0,
            welcome: true,
            focus_opacity: 1.0,
            grain: true,
            grain_intensity: 0.05,
            monitor_configs: vec![],
            window_rules: vec![],
            keybindings: default_keybindings(),
            keybindings_from_file: false,
        }
    }
}

impl Config {
    /// Owned XKB parts for `XkbConfig` construction. The keymap is baked
    /// into the keyboard when the seat is created, so editing the `xkb`
    pub fn xkb_parts(&self) -> (String, String, String, String, Option<String>) {
        let options = if self.xkb_options.is_empty() {
            None
        } else {
            Some(self.xkb_options.clone())
        };
        (
            self.xkb_rules.clone(),
            self.xkb_model.clone(),
            self.xkb_layout.clone(),
            self.xkb_variant.clone(),
            options,
        )
    }
}

pub struct ConfigWatcher {
    pub config: Config,
    last_modified: Option<SystemTime>,
    file_path: String,
    /// The master config plus every transitively included file that the
    /// current `config` was built from. Used to fingerprint file changes.
    loaded_paths: Vec<std::path::PathBuf>,
}

/// Locate the config file. Order (first hit wins):
/// 1. `$NOVAWM_CONFIG` if set
fn locate_config(fallback: &str) -> String {
    let mut candidates: Vec<String> = Vec::new();

    if let Ok(path) = std::env::var("NOVAWM_CONFIG") {
        if !path.is_empty() {
            candidates.push(path);
        }
    }
    candidates.push("config.kdl".to_string());

    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    if let Some(home) = config_home {
        candidates.push(
            home.join("novawm")
                .join("config.kdl")
                .to_string_lossy()
                .into_owned(),
        );
    }
    candidates.push("/usr/share/novawm/config/config.kdl".to_string());
    candidates.push("/usr/local/share/novawm/config/config.kdl".to_string());

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("config.kdl").to_string_lossy().into_owned());
            if let Some(root) = dir.parent().and_then(|p| p.parent()) {
                candidates.push(root.join("config.kdl").to_string_lossy().into_owned());
            }
        }
    }
    candidates.push(fallback.to_string());

    for c in candidates {
        if !c.is_empty() && Path::new(&c).exists() {
            return c;
        }
    }
    fallback.to_string()
}

impl ConfigWatcher {
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        let requested = path.as_ref().to_string_lossy().into_owned();
        let file_path = locate_config(&requested);
        let mut watcher = Self {
            config: Config::default(),
            last_modified: None,
            file_path,
            loaded_paths: Vec::new(),
        };
        
        crate::life(format!("NovaWM config path: {}", watcher.file_path));
        watcher.check_and_reload();
        watcher
    }

    /// The files currently contributing to `config`: the master file plus
    /// every transitively included file.
    fn current_sources(&self) -> Vec<std::path::PathBuf> {
        if self.loaded_paths.is_empty() {
            vec![std::path::PathBuf::from(&self.file_path)]
        } else {
            self.loaded_paths.clone()
        }
    }

    /// Most recent modification time across `paths`, if any are readable.
    fn newest_mtime(paths: &[std::path::PathBuf]) -> Option<SystemTime> {
        paths
            .iter()
            .filter_map(|p| fs::metadata(p).ok())
            .filter_map(|m| m.modified().ok())
            .max()
    }

    pub fn check_and_reload(&mut self) -> bool {
        let path = Path::new(&self.file_path);
        if !path.exists() {
            let default_kdl = r##"// NovaWM Configuration

modifier "alt"
layout "dwindle"

keybinds {
    bind "mod+Return"    "spawn_terminal"
    bind "mod+c"         "close_focused"
    bind "mod+h"         "focus left"
    bind "mod+l"         "focus right"
    bind "mod+j"         "focus down"
    bind "mod+k"         "focus up"
    bind "mod+shift+h"   "move_list previous"
    bind "mod+shift+l"   "move_list next"
    bind "mod+shift+j"   "move_row next"
    bind "mod+shift+k"   "move_row previous"
    bind "mod+comma"     "consume_into_column"
    bind "mod+period"    "expel_from_column"
    bind "mod+o"         "toggle_overview"
    bind "mod+1"         "workspace 1"
    bind "mod+2"         "workspace 2"
    bind "mod+3"         "workspace 3"
    bind "mod+4"         "workspace 4"
    bind "mod+5"         "workspace 5"
    bind "mod+6"         "workspace 6"
    bind "mod+7"         "workspace 7"
    bind "mod+8"         "workspace 8"
    bind "mod+9"         "workspace 9"
    bind "mod+x"         "canvas_center_window"
    bind "mod+w"         "canvas_zoom_to_fit"
    bind "mod+shift+Left"  "canvas_nudge left"
    bind "mod+shift+Right" "canvas_nudge right"
    bind "mod+shift+Up"    "canvas_nudge up"
    bind "mod+shift+Down"  "canvas_nudge down"
    bind "mod+ctrl+Left"   "canvas_pan left"
    bind "mod+ctrl+Right"  "canvas_pan right"
    bind "mod+ctrl+Up"     "canvas_pan up"
    bind "mod+ctrl+Down"   "canvas_pan down"
    bind "mod+t"         "toggle_pin"
    bind "mod+a"         "canvas_home"
    bind "mod+Tab"       "cycle_focus next"
    bind "mod+shift+Tab" "cycle_focus previous"
    bind "mod+grave"     "toggle_scratchpad"
    bind "mod+F1"        "welcome"
    // Custom commands: `spawn` runs anything through sh, so pipes,
    // quotes, and $VARS work. Uncomment and adapt:
    // bind "mod+d" "spawn wofi --show drun"
    bind "mod+f"         "fullscreen"
    bind "mod+v"         "toggle_float"
    bind "mod+equal"     "canvas_zoom_in"
    bind "mod+minus"     "canvas_zoom_out"
    bind "mod+0"         "canvas_zoom_reset"
    bindm "mod+mouse:272" "move_window"
    bindm "mod+mouse:273" "resize_window"
}

scroll {
    speed 400
}

dwindle {
    split_ratio 0.5
    split_direction "auto"
}

gaps {
    inner 10
    outer 15
}

animations {
    enabled #true
    duration 300
    bezier 0.25 0.1 0.25 1.0
    open "scale_up"
}

focus {
    follows_mouse #true
}

// Mouse cursor theme (looked up like any XCURSOR_THEME, e.g. Bibata,
// Adwaita, breeze_cursors). Empty = the built-in drawn arrow.
xcursor-theme "Bibata-Modern-Ice"
xcursor-size 24

// First-run welcome window: a terminal with a tour and a live keybind
// table, shown once per user at startup. Reopen anytime with mod+F1;
// set #false to never show it.
welcome #true

// Keyboard layout, mouse, and touchpad (Niri-style). The keyboard layout
// applies at startup (restart after editing); the libinput settings
// re-apply whenever the config reloads. A bare flag (`tap`) means #true.
input {
    focus-follows-mouse

    keyboard {
        xkb {
            layout "us,il"
            options "grp:alt_shift_toggle"
        }
    }

    touchpad {
        tap
        natural-scroll
        accel-profile "adaptive"
        accel-speed 0.2
        click-method "clickfinger"
        disable-while-typing
    }

    mouse {
        accel-profile "flat"
        accel-speed 0.3
    }
}

decorations {
    rounding 12
    border_width 2
    active_border_color "#ff007f"
    inactive_border_color "#444444"
    border_effect "none"
    border_effect_speed 1.0
    border_gradient_color "#00d9ff"
    blur #true
    opacity 0.85
    focus_opacity 1.0
    grain #true
    grain_intensity 0.05
}

background "wallpaper.png"

startup "foot"
terminal "foot"
"##;
            eprintln!(
                "CONFIG: no config found, writing defaults to {}",
                path.display()
            );
            let _ = fs::write(path, default_kdl);
        }

        let newest = Self::newest_mtime(&self.current_sources());

        if newest == self.last_modified {
            return false;
        }

        self.last_modified = newest;

        match self.load_config() {
            Ok((parsed, sources)) => {
                eprintln!(
                    "CONFIG RELOADED SUCCESSFULLY: modifier={} layout={} files=[{}]",
                    parsed.modifier,
                    parsed.layout,
                    sources
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                self.config = parsed;
                self.loaded_paths = sources;
                true
            }
            Err(e) => {
                eprintln!("FAILED TO PARSE CONFIG {}: {e:?}", self.file_path);
                false
            }
        }
    }

    /// Parse the master file and every `include "file.kdl"` it references
    /// (transitively, cycle-guarded). Include files are applied *after* the
    fn load_config(&self) -> Result<(Config, Vec<std::path::PathBuf>), Box<dyn std::error::Error>> {
        let master_path = std::path::PathBuf::from(&self.file_path);
        let content = fs::read_to_string(&master_path)?;
        let doc: KdlDocument = content.parse()?;

        let mut config = Config::default();
        apply_document(&mut config, &doc);

        let mut sources = vec![master_path.clone()];
        let mut visited = std::collections::HashSet::new();
        visited.insert(canonical_or(&master_path));
        let master_dir = master_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_default();

        apply_includes(&mut config, &master_dir, &doc, &mut sources, &mut visited, 0)?;

        Ok((config, sources))
    }
}

/// Canonicalize `path`, falling back to the raw path if it cannot be resolved.
fn canonical_or(path: &Path) -> std::path::PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Recursively merge `include "file.kdl"` nodes from `doc` into `config`.
///
fn apply_includes(
    config: &mut Config,
    dir: &Path,
    doc: &KdlDocument,
    sources: &mut Vec<std::path::PathBuf>,
    visited: &mut std::collections::HashSet<std::path::PathBuf>,
    depth: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    if depth > 8 {
        return Err("config include depth exceeded (max 8)".into());
    }

    for node in doc.nodes() {
        if node.name().value() != "include" {
            continue;
        }
        for entry in node.entries() {
            let Some(name) = entry.value().as_string() else {
                continue;
            };
            let target = if Path::new(name).is_absolute() {
                std::path::PathBuf::from(name)
            } else {
                dir.join(name)
            };
            if !visited.insert(canonical_or(&target)) {
                continue;
            }
            if !target.exists() {
                tracing::warn!("CONFIG: include not found: {}", target.display());
                continue;
            }
            let content = match fs::read_to_string(&target) {
                Ok(content) => content,
                Err(error) => {
                    tracing::warn!("CONFIG: cannot read {}: {error}", target.display());
                    continue;
                }
            };
            let child_doc: KdlDocument = match content.parse() {
                Ok(doc) => doc,
                Err(error) => {
                    tracing::warn!("CONFIG: cannot parse {}: {error}", target.display());
                    continue;
                }
            };
            apply_document(config, &child_doc);
            sources.push(target.clone());
            let child_dir = target
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_default();
            apply_includes(config, &child_dir, &child_doc, sources, visited, depth + 1)?;
        }
    }
    Ok(())
}

/// Apply every node from `doc` onto an already-initialized config. Values
/// present in the document overwrite the existing ones (last applied wins).
fn apply_document(config: &mut Config, doc: &KdlDocument) {
        if let Some(mod_node) = doc.get("modifier") {
            if let Some(val) = mod_node
                .entries()
                .first()
                .and_then(|e| e.value().as_string())
            {
                config.modifier = val.to_string();
            }
        }

        if let Some(layout_node) = doc.get("layout") {
            if let Some(val) = layout_node
                .entries()
                .first()
                .and_then(|e| e.value().as_string())
            {
                config.layout = match val {
                    "scrolling" | "scroll" => "scroll".to_string(),
                    other => other.to_string(),
                };
            }
        }

        if let Some(scroll) = doc.get("scroll") {
            if let Some(children) = scroll.children() {
                if let Some(speed) = children.get("speed") {
                    if let Some(val) = speed.entries().first().and_then(|e| e.value().as_integer())
                    {
                        config.scroll_speed = val as i32;
                    }
                }
            }
        }

        if let Some(dwindle) = doc.get("dwindle") {
            if let Some(children) = dwindle.children() {
                if let Some(ratio) = children.get("split_ratio") {
                    if let Some(val) = ratio.entries().first() {
                        if let Some(f_val) = val.value().as_float() {
                            config.dwindle_split_ratio = f_val as f32;
                        } else if let Some(i_val) = val.value().as_integer() {
                            config.dwindle_split_ratio = i_val as f32;
                        }
                    }
                }
                if let Some(direction) = children.get("split_direction") {
                    if let Some(val) = direction
                        .entries()
                        .first()
                        .and_then(|e| e.value().as_string())
                    {
                        config.dwindle_split_direction = val.to_string();
                    }
                }
            }
        }

        if let Some(bg) = doc.get("background") {
            if let Some(val) = bg.entries().first().and_then(|e| e.value().as_string()) {
                config.background = val.to_string();
            }
        }

        if let Some(startup) = doc.get("startup") {
            let apps: Vec<String> = startup
                .entries()
                .iter()
                .filter_map(|e| e.value().as_string().map(|s| s.to_owned()))
                .filter(|s| !s.is_empty())
                .collect();
            if !apps.is_empty() {
                config.startup_apps = apps;
            }
        }

        if let Some(term) = doc.get("terminal") {
            if let Some(val) = term.entries().first().and_then(|e| e.value().as_string()) {
                if !val.is_empty() {
                    config.terminal = val.to_string();
                }
            }
        }

        for node in doc.nodes() {
            if node.name().value() == "monitor" {
                let entries: Vec<_> = node.entries().iter().collect();
                let name = entries
                    .first()
                    .and_then(|e| e.value().as_string())
                    .unwrap_or("")
                    .to_string();
                if name.is_empty() {
                    continue;
                }
                if let Some(children) = node.children() {
                    if let Some(pos_node) = children.get("position") {
                        let vals: Vec<i32> = pos_node
                            .entries()
                            .iter()
                            .filter_map(|e| e.value().as_integer().map(|v| v as i32))
                            .collect();
                        if vals.len() >= 2 {
                            config.monitor_configs.push(MonitorConfig {
                                name,
                                x: vals[0],
                                y: vals[1],
                            });
                        }
                    }
                }
            }
        }

        if let Some(gaps) = doc.get("gaps") {
            if let Some(children) = gaps.children() {
                if let Some(inner) = children.get("inner") {
                    if let Some(val) = inner.entries().first().and_then(|e| e.value().as_integer())
                    {
                        config.inner_gap = val as i32;
                    }
                }
                if let Some(outer) = children.get("outer") {
                    if let Some(val) = outer.entries().first().and_then(|e| e.value().as_integer())
                    {
                        config.outer_gap = val as i32;
                    }
                }
            }
        }

        if let Some(animations) = doc.get("animations") {
            if let Some(children) = animations.children() {
                if let Some(enabled) = children.get("enabled") {
                    if let Some(val) = enabled.entries().first().and_then(|e| e.value().as_bool()) {
                        config.animation_enabled = val;
                    }
                }
                if let Some(dur) = children.get("duration") {
                    if let Some(val) = dur.entries().first() {
                        if let Some(f_val) = val.value().as_float() {
                            config.animation_duration = f_val;
                        } else if let Some(i_val) = val.value().as_integer() {
                            config.animation_duration = i_val as f64;
                        }
                    }
                }
                if let Some(bezier) = children.get("bezier") {
                    let vals: Vec<f64> = bezier
                        .entries()
                        .iter()
                        .filter_map(|e| {
                            if let Some(f) = e.value().as_float() {
                                Some(f)
                            } else if let Some(i) = e.value().as_integer() {
                                Some(i as f64)
                            } else {
                                None
                            }
                        })
                        .collect();
                    if vals.len() == 4 {
                        config.animation_bezier = (vals[0], vals[1], vals[2], vals[3]);
                    }
                }
                if let Some(open) = children.get("open") {
                    if let Some(val) = open.entries().first().and_then(|e| e.value().as_string()) {
                        config.animation_open = val.to_string();
                    }
                }
            }
        }

        if let Some(focus) = doc.get("focus") {
            if let Some(children) = focus.children() {
                if let Some(follows) = children.get("follows_mouse") {
                    if let Some(val) = follows.entries().first().and_then(|e| e.value().as_bool()) {
                        config.focus_follows_mouse = val;
                    }
                }
            }
        }

        if let Some(decorations) = doc.get("decorations") {
            if let Some(children) = decorations.children() {
                if let Some(rounding) = children.get("rounding") {
                    if let Some(val) = rounding
                        .entries()
                        .first()
                        .and_then(|e| e.value().as_integer())
                    {
                        config.rounding = val as i32;
                    }
                }
                if let Some(border) = children.get("border_width") {
                    if let Some(val) = border
                        .entries()
                        .first()
                        .and_then(|e| e.value().as_integer())
                    {
                        config.border_width = val as i32;
                    }
                }
                if let Some(blur) = children.get("blur") {
                    if let Some(val) = blur.entries().first().and_then(|e| e.value().as_bool()) {
                        config.blur = val;
                    }
                }
                if let Some(opacity) = children.get("opacity") {
                    if let Some(val) = opacity.entries().first() {
                        if let Some(f_val) = val.value().as_float() {
                            config.opacity = f_val as f32;
                        } else if let Some(i_val) = val.value().as_integer() {
                            config.opacity = i_val as f32;
                        }
                    }
                }
                if let Some(focus_opacity) = children.get("focus_opacity") {
                    if let Some(val) = focus_opacity.entries().first() {
                        if let Some(f_val) = val.value().as_float() {
                            config.focus_opacity = f_val as f32;
                        } else if let Some(i_val) = val.value().as_integer() {
                            config.focus_opacity = i_val as f32;
                        }
                    }
                }
                if let Some(grain) = children.get("grain") {
                    if let Some(val) = grain.entries().first().and_then(|e| e.value().as_bool()) {
                        config.grain = val;
                    }
                }
                if let Some(grain_intensity) = children.get("grain_intensity") {
                    if let Some(val) = grain_intensity.entries().first() {
                        if let Some(f_val) = val.value().as_float() {
                            config.grain_intensity = f_val as f32;
                        } else if let Some(i_val) = val.value().as_integer() {
                            config.grain_intensity = i_val as f32;
                        }
                    }
                }
                if let Some(color) = children.get("active_border_color") {
                    if let Some(hex) = color.entries().first().and_then(|e| e.value().as_string()) {
                        if let Some(rgba) = parse_hex_color(hex) {
                            config.active_border_color = rgba;
                        }
                    }
                }
                if let Some(color) = children.get("inactive_border_color") {
                    if let Some(hex) = color.entries().first().and_then(|e| e.value().as_string()) {
                        if let Some(rgba) = parse_hex_color(hex) {
                            config.inactive_border_color = rgba;
                        }
                    }
                }
                if let Some(effect) = children.get("border_effect") {
                    if let Some(name) =
                        effect.entries().first().and_then(|e| e.value().as_string())
                    {
                        config.border_effect = name.to_string();
                    }
                }
                if let Some(speed) = children.get("border_effect_speed") {
                    if let Some(val) = speed.entries().first() {
                        if let Some(f_val) = val.value().as_float() {
                            config.border_effect_speed = f_val as f32;
                        } else if let Some(i_val) = val.value().as_integer() {
                            config.border_effect_speed = i_val as f32;
                        }
                    }
                }
                if let Some(color) = children.get("border_gradient_color") {
                    if let Some(hex) = color.entries().first().and_then(|e| e.value().as_string()) {
                        if let Some(rgba) = parse_hex_color(hex) {
                            config.border_gradient_color = rgba;
                        }
                    }
                }
            }
        }

        if let Some(theme) = doc
            .get("xcursor-theme")
            .or_else(|| doc.get("xcursor_theme"))
        {
            if let Some(val) = theme
                .entries()
                .first()
                .and_then(|e| e.value().as_string())
            {
                config.xcursor_theme = val.to_string();
            }
        }

        if let Some(size) = doc
            .get("xcursor-size")
            .or_else(|| doc.get("xcursor_size"))
        {
            if let Some(val) = size
                .entries()
                .first()
                .and_then(|e| e.value().as_integer())
            {
                config.xcursor_size = val.max(8) as i32;
            }
        }

        if let Some(welcome) = doc.get("welcome") {
            config.welcome = welcome
                .entries()
                .first()
                .and_then(|e| e.value().as_bool())
                .unwrap_or(true);
        }

        if let Some(input) = doc.get("input") {
            if let Some(children) = input.children() {
                let flag = |names: &[&str]| -> Option<bool> {
                    names.iter().find_map(|name| children.get(name)).map(|node| {
                        node.entries()
                            .first()
                            .and_then(|e| e.value().as_bool())
                            .unwrap_or(true)
                    })
                };

                if let Some(follows) = flag(&["focus-follows-mouse", "focus_follows_mouse"]) {
                    config.focus_follows_mouse = follows;
                }

                if let Some(keyboard) = children.get("keyboard") {
                    if let Some(kids) = keyboard.children() {
                        if let Some(xkb) = kids.get("xkb") {
                            if let Some(xkids) = xkb.children() {
                                let xkb_text = |name: &str| -> Option<String> {
                                    xkids
                                        .get(name)
                                        .and_then(|node| {
                                            node.entries()
                                                .first()
                                                .and_then(|e| e.value().as_string())
                                        })
                                        .map(str::to_string)
                                };
                                if let Some(val) = xkb_text("rules") {
                                    config.xkb_rules = val;
                                }
                                if let Some(val) = xkb_text("model") {
                                    config.xkb_model = val;
                                }
                                if let Some(val) = xkb_text("layout") {
                                    config.xkb_layout = val;
                                }
                                if let Some(val) = xkb_text("variant") {
                                    config.xkb_variant = val;
                                }
                                if let Some(val) = xkb_text("options") {
                                    config.xkb_options = val;
                                }
                            }
                        }
                    }
                }

                if let Some(touchpad) = children.get("touchpad") {
                    if let Some(tkids) = touchpad.children() {
                        let tp_flag = |names: &[&str]| -> Option<bool> {
                            names.iter().find_map(|name| tkids.get(name)).map(|node| {
                                node.entries()
                                    .first()
                                    .and_then(|e| e.value().as_bool())
                                    .unwrap_or(true)
                            })
                        };
                        let tp_text = |names: &[&str]| -> Option<String> {
                            names
                                .iter()
                                .find_map(|name| tkids.get(name))
                                .and_then(|node| {
                                    node.entries()
                                        .first()
                                        .and_then(|e| e.value().as_string())
                                })
                                .map(str::to_string)
                        };
                        let tp_number = |names: &[&str]| -> Option<f64> {
                            names.iter().find_map(|name| tkids.get(name)).and_then(|node| {
                                node.entries().first().and_then(|e| {
                                    e.value()
                                        .as_float()
                                        .or_else(|| e.value().as_integer().map(|i| i as f64))
                                })
                            })
                        };
                        if let Some(val) = tp_flag(&["tap"]) {
                            config.touchpad_tap = val;
                        }
                        if let Some(val) = tp_flag(&["natural-scroll", "natural_scroll"]) {
                            config.touchpad_natural_scroll = val;
                        }
                        if let Some(val) = tp_text(&["accel-profile", "accel_profile"]) {
                            config.touchpad_accel_profile = val;
                        }
                        if let Some(val) = tp_number(&["accel-speed", "accel_speed"]) {
                            config.touchpad_accel_speed = val.clamp(-1.0, 1.0);
                        }
                        if let Some(val) = tp_text(&["click-method", "click_method"]) {
                            config.touchpad_click_method = val;
                        }
                        if let Some(val) = tp_flag(&["disable-while-typing", "disable_while_typing"])
                        {
                            config.touchpad_dwt = val;
                        }
                    }
                }

                if let Some(mouse) = children.get("mouse") {
                    if let Some(mkids) = mouse.children() {
                        let m_flag = |names: &[&str]| -> Option<bool> {
                            names.iter().find_map(|name| mkids.get(name)).map(|node| {
                                node.entries()
                                    .first()
                                    .and_then(|e| e.value().as_bool())
                                    .unwrap_or(true)
                            })
                        };
                        let m_text = |names: &[&str]| -> Option<String> {
                            names
                                .iter()
                                .find_map(|name| mkids.get(name))
                                .and_then(|node| {
                                    node.entries()
                                        .first()
                                        .and_then(|e| e.value().as_string())
                                })
                                .map(str::to_string)
                        };
                        let m_number = |names: &[&str]| -> Option<f64> {
                            names.iter().find_map(|name| mkids.get(name)).and_then(|node| {
                                node.entries().first().and_then(|e| {
                                    e.value()
                                        .as_float()
                                        .or_else(|| e.value().as_integer().map(|i| i as f64))
                                })
                            })
                        };
                        if let Some(val) = m_flag(&["natural-scroll", "natural_scroll"]) {
                            config.mouse_natural_scroll = val;
                        }
                        if let Some(val) = m_text(&["accel-profile", "accel_profile"]) {
                            config.mouse_accel_profile = val;
                        }
                        if let Some(val) = m_number(&["accel-speed", "accel_speed"]) {
                            config.mouse_accel_speed = val.clamp(-1.0, 1.0);
                        }
                    }
                }
            }
        }

        if let Some(keybinds) = doc.get("keybinds") {
            if let Some(children) = keybinds.children() {
                let mut parsed = Vec::new();
                for node in children.nodes() {
                    let tag = node.name().value();
                    if tag != "bind" && tag != "bindm" {
                        continue;
                    }
                    let entries: Vec<String> = node
                        .entries()
                        .iter()
                        .filter_map(|e| e.value().as_string().map(|s| s.to_owned()))
                        .collect();
                    if entries.len() < 2 {
                        continue;
                    }
                    if tag == "bindm" {
                        if let Some(kb) = Keybind::parse(&entries[0], &entries[1]) {
                            if kb.is_mouse() {
                                parsed.push(kb);
                            } else {
                                if let Some(btn) = entries[0]
                                    .split('+')
                                    .next_back()
                                    .and_then(|k| k.strip_prefix("mouse:"))
                                    .and_then(|b| b.parse::<u32>().ok())
                                {
                                    let mut kb = kb;
                                    kb.mouse_button = Some(btn);
                                    parsed.push(kb);
                                }
                            }
                        }
                    } else if let Some(kb) = Keybind::parse(&entries[0], &entries[1]) {
                        parsed.push(kb);
                    }
                }
                if !parsed.is_empty() {
                    if !config.keybindings_from_file {
                        config.keybindings = parsed;
                        config.keybindings_from_file = true;
                    } else {
                        for kb in parsed {
                            config.keybindings.retain(|existing| {
                                existing.modifiers != kb.modifiers
                                    || existing.key != kb.key
                                    || existing.mouse_button != kb.mouse_button
                            });
                            config.keybindings.push(kb);
                        }
                    }
                }
            }
        }

        for node in doc.nodes() {
            let name = node.name().value();
            if name != "windowrule" && name != "window_rule" {
                continue;
            }
            let mut rule = WindowRule::default();
            if let Some(children) = node.children() {
                for child in children.nodes() {
                    let tag = child.name().value();
                    match tag {
                        "app_id" | "appid" | "class" => {
                            if let Some(v) = child
                                .entries()
                                .first()
                                .and_then(|e| e.value().as_string())
                            {
                                rule.app_id = v.to_string();
                            }
                        }
                        "title" => {
                            if let Some(v) = child
                                .entries()
                                .first()
                                .and_then(|e| e.value().as_string())
                            {
                                rule.title = v.to_string();
                            }
                        }
                        "float" | "floating" => {
                            let on = child
                                .entries()
                                .first()
                                .and_then(|e| e.value().as_bool())
                                .unwrap_or(true);
                            rule.float = on;
                        }
                        "opacity" => {
                            if let Some(val) = child.entries().first() {
                                if let Some(f) = val.value().as_float() {
                                    rule.opacity = Some((f as f32).clamp(0.0, 1.0));
                                } else if let Some(i) = val.value().as_integer() {
                                    rule.opacity = Some((i as f32).clamp(0.0, 1.0));
                                }
                            }
                        }
                        "workspace" | "ws" => {
                            if let Some(i) = child
                                .entries()
                                .first()
                                .and_then(|e| e.value().as_integer())
                            {
                                if (1..=9).contains(&i) {
                                    rule.workspace = Some(i as u8);
                                }
                            }
                        }
                        "center" => {
                            let on = child
                                .entries()
                                .first()
                                .and_then(|e| e.value().as_bool())
                                .unwrap_or(true);
                            rule.center = on;
                        }
                        _ => {}
                    }
                }
            }
            for (i, entry) in node.entries().iter().enumerate() {
                if let Some(s) = entry.value().as_string() {
                    if i == 0 && rule.app_id.is_empty() {
                        rule.app_id = s.to_string();
                    } else if s.eq_ignore_ascii_case("float") {
                        rule.float = true;
                    } else if s.eq_ignore_ascii_case("center") {
                        rule.center = true;
                    }
                }
            }
            if !rule.app_id.is_empty()
                || !rule.title.is_empty()
                || rule.float
                || rule.opacity.is_some()
                || rule.workspace.is_some()
                || rule.center
            {
                config.window_rules.push(rule);
            }
        }
    }

fn parse_hex_color(hex: &str) -> Option<[f32; 4]> {
    let clean = hex.trim_start_matches('#');
    if clean.len() == 6 {
        let r = u8::from_str_radix(&clean[0..2], 16).ok()? as f32 / 255.0;
        let g = u8::from_str_radix(&clean[2..4], 16).ok()? as f32 / 255.0;
        let b = u8::from_str_radix(&clean[4..6], 16).ok()? as f32 / 255.0;
        Some([r, g, b, 1.0])
    } else if clean.len() == 8 {
        let r = u8::from_str_radix(&clean[0..2], 16).ok()? as f32 / 255.0;
        let g = u8::from_str_radix(&clean[2..4], 16).ok()? as f32 / 255.0;
        let b = u8::from_str_radix(&clean[4..6], 16).ok()? as f32 / 255.0;
        let a = u8::from_str_radix(&clean[6..8], 16).ok()? as f32 / 255.0;
        Some([r, g, b, a])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The real repo `config.kdl` must parse through the include mechanism and
    /// produce the intended modifier, monitor positions and keybinds.
    #[test]
    fn window_rules_parse() {
        let mut config = Config::default();
        let doc: kdl::KdlDocument = r#"
windowrule {
    app_id "mpv"
    float
    opacity 1.0
}
windowrule {
    title "Picture-in-Picture"
    float
    center
}
windowrule {
    app_id "discord"
    workspace 3
    opacity 0.95
}
"#
        .parse()
        .unwrap();
        super::apply_document(&mut config, &doc);
        assert_eq!(config.window_rules.len(), 3);
        assert_eq!(config.window_rules[0].app_id, "mpv");
        assert!(config.window_rules[0].float);
        assert_eq!(config.window_rules[0].opacity, Some(1.0));
        assert_eq!(config.window_rules[1].title, "Picture-in-Picture");
        assert!(config.window_rules[1].center);
        assert_eq!(config.window_rules[2].workspace, Some(3));
    }

    #[test]
    fn repo_config_loads_with_includes() {
        let watcher = ConfigWatcher::new("config.kdl");

        assert_eq!(watcher.config.modifier, "super");
        assert!(watcher.config.layout == "scroll" || watcher.config.layout == "scrolling");

        let monitors: Vec<(&str, i32, i32)> = watcher
            .config
            .monitor_configs
            .iter()
            .map(|m| (m.name.as_str(), m.x, m.y))
            .collect();
        assert_eq!(
            monitors,
            vec![
                ("eDP-1", 0, 0),
                ("HDMI-A-1", 2560, 0),
                ("DP-1", 4480, 0),
            ]
        );

        let actions: Vec<&str> = watcher
            .config
            .keybindings
            .iter()
            .map(|k| k.action.as_str())
            .collect();
        assert!(actions.contains(&"spawn_terminal"), "keybinds must load from keys.kdl");
        assert!(
            actions.contains(&"canvas_zoom_in"),
            "canvas binds must merge in from canvas_keys.kdl"
        );
        assert!(
            actions.contains(&"close_focused") && actions.contains(&"move_window"),
            "later keybinds blocks must not wipe earlier ones"
        );
        assert_eq!(watcher.config.scroll_speed, 400, "scroll settings must load from scroll.kdl");
    }
}
