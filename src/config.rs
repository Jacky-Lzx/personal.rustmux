use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::SCROLLBACK_LINES;

const CONFIG_RELOAD_INTERVAL: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, Eq, PartialEq)]
enum ConfigSnapshot {
    Missing,
    Contents(Vec<u8>),
    ReadError(String),
}

pub(super) struct ConfigReloader {
    path: PathBuf,
    snapshot: ConfigSnapshot,
    next_check: Instant,
}

impl ConfigReloader {
    pub(super) fn new(path: PathBuf) -> Self {
        Self {
            snapshot: config_snapshot(&path),
            path,
            next_check: Instant::now() + CONFIG_RELOAD_INTERVAL,
        }
    }

    pub(super) fn poll(&mut self, now: Instant) -> Option<Result<Config, String>> {
        if now < self.next_check {
            return None;
        }
        self.next_check = now + CONFIG_RELOAD_INTERVAL;
        let snapshot = config_snapshot(&self.path);
        if snapshot == self.snapshot {
            return None;
        }
        self.snapshot = snapshot.clone();
        Some(Config::from_snapshot(&self.path, snapshot))
    }
}

fn config_snapshot(path: &Path) -> ConfigSnapshot {
    match fs::read(path) {
        Ok(contents) => ConfigSnapshot::Contents(contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ConfigSnapshot::Missing,
        Err(error) => ConfigSnapshot::ReadError(error.to_string()),
    }
}

pub const DEFAULT_CONFIG_TOML: &str = r#"
default_mode = "locked"
clear_defaults = false
compact = false
scrollback_lines = 1000
autosave_interval_seconds = 30 # 0 disables automatic saving.
# shell = "/bin/zsh" # Defaults to $SHELL, then /bin/sh.

[notifications]
enabled = true
command_duration_seconds = 10
exclude_applications = ["yazi", "nvim"]

# Each action replaces its default keys; [] disables it.
[session_manager]
up = ["k", "up"]
down = ["j", "down"]
search = ["/"]
complete = ["tab"]
open = ["enter"]
rename = ["Ctrl r"]
save = ["Ctrl a"]
delete = ["delete"]
disconnect = ["Ctrl x"]
cancel = ["esc"]
backspace = ["backspace"]

# A binding can also use { actions = [...], display = "always" }.
# display: "always" = status + help, "help" = help only, "hidden" = not listed.
[keybinds.locked]
"Ctrl b" = [{ action = "switch-mode", mode = "normal" }]

[keybinds.normal]
"Ctrl b" = ["send-prefix", { action = "switch-mode", mode = "locked" }]
c = ["new-window", { action = "switch-mode", mode = "locked" }]
"," = ["rename-window"]
n = ["next-window", { action = "switch-mode", mode = "locked" }]
p = ["previous-window", { action = "switch-mode", mode = "locked" }]
s = ["switch-session"]
1 = [{ action = "go-to-window", index = 1 }, { action = "switch-mode", mode = "locked" }]
2 = [{ action = "go-to-window", index = 2 }, { action = "switch-mode", mode = "locked" }]
3 = [{ action = "go-to-window", index = 3 }, { action = "switch-mode", mode = "locked" }]
4 = [{ action = "go-to-window", index = 4 }, { action = "switch-mode", mode = "locked" }]
5 = [{ action = "go-to-window", index = 5 }, { action = "switch-mode", mode = "locked" }]
6 = [{ action = "go-to-window", index = 6 }, { action = "switch-mode", mode = "locked" }]
7 = [{ action = "go-to-window", index = 7 }, { action = "switch-mode", mode = "locked" }]
8 = [{ action = "go-to-window", index = 8 }, { action = "switch-mode", mode = "locked" }]
9 = [{ action = "go-to-window", index = 9 }, { action = "switch-mode", mode = "locked" }]
"[" = [{ action = "switch-mode", mode = "scroll" }]
enter = [{ action = "switch-mode", mode = "scroll" }]
h = ["edit-history", { action = "switch-mode", mode = "locked" }]
e = ["edit-last-output", { action = "switch-mode", mode = "locked" }]
y = ["copy-last-output", { action = "switch-mode", mode = "locked" }]
i = [{ action = "switch-mode", mode = "locked" }, "toggle-floating-terminal"]
"Ctrl p" = [{ action = "switch-mode", mode = "pane" }]
"&" = ["close-window", { action = "switch-mode", mode = "locked" }]
x = ["close-window", { action = "switch-mode", mode = "locked" }]
d = ["detach"]
"?" = ["show-help", { action = "switch-mode", mode = "locked" }]
"<" = { actions = ["move-window-left", { action = "switch-mode", mode = "locked" }], display = "help" }
">" = { actions = ["move-window-right", { action = "switch-mode", mode = "locked" }], display = "help" }

[keybinds.pane]
r = ["new-pane-right", { action = "switch-mode", mode = "locked" }]
d = ["new-pane-down", { action = "switch-mode", mode = "locked" }]
n = ["new-pane-right", { action = "switch-mode", mode = "locked" }]
h = ["focus-left", { action = "switch-mode", mode = "locked" }]
j = ["focus-down", { action = "switch-mode", mode = "locked" }]
k = ["focus-up", { action = "switch-mode", mode = "locked" }]
l = ["focus-right", { action = "switch-mode", mode = "locked" }]
left = ["focus-left", { action = "switch-mode", mode = "locked" }]
down = ["focus-down", { action = "switch-mode", mode = "locked" }]
up = ["focus-up", { action = "switch-mode", mode = "locked" }]
right = ["focus-right", { action = "switch-mode", mode = "locked" }]
tab = ["focus-next-pane", { action = "switch-mode", mode = "locked" }]
H = ["resize-pane-left"]
J = ["resize-pane-down"]
K = ["resize-pane-up"]
L = ["resize-pane-right"]
z = ["toggle-pane-zoom"]
x = ["close-pane", { action = "switch-mode", mode = "locked" }]
q = [{ action = "switch-mode", mode = "locked" }]
esc = [{ action = "switch-mode", mode = "locked" }]
"Alt h" = { actions = ["move-pane-left"], display = "help" }
"Alt j" = { actions = ["move-pane-down"], display = "help" }
"Alt k" = { actions = ["move-pane-up"], display = "help" }
"Alt l" = { actions = ["move-pane-right"], display = "help" }

[keybinds.scroll]
"/" = ["search-history"]
n = ["next-search-match"]
N = ["previous-search-match"]
v = ["toggle-history-selection"]
left = ["selection-left"]
h = ["selection-left"]
right = ["selection-right"]
l = ["selection-right"]
up = ["scroll-up"]
k = ["scroll-up"]
down = ["scroll-down"]
j = ["scroll-down"]
pageup = ["page-up"]
u = ["page-up"]
"Ctrl b" = ["page-up"]
"Ctrl u" = ["page-up"]
pagedown = ["page-down"]
d = ["page-down"]
"Ctrl f" = ["page-down"]
"Ctrl d" = ["page-down"]
g = ["scroll-top"]
G = ["scroll-bottom"]
E = ["scroll-bottom", { action = "switch-mode", mode = "locked" }, "edit-history"]
e = ["scroll-bottom", { action = "switch-mode", mode = "locked" }, "edit-last-output"]
y = ["copy-selection"]
q = ["scroll-bottom", { action = "switch-mode", mode = "locked" }]
esc = ["scroll-bottom", { action = "switch-mode", mode = "locked" }]
"#;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    SwitchMode(String),
    SendPrefix,
    SendKey(Vec<u8>),
    NewWindow,
    RenameWindow,
    NextWindow,
    PreviousWindow,
    MoveWindowLeft,
    MoveWindowRight,
    SwitchSession,
    GoToWindow(usize),
    CloseWindow,
    Detach,
    ShowHelp,
    ScrollUp,
    ScrollDown,
    PageUp,
    PageDown,
    ScrollTop,
    ScrollBottom,
    EditHistory,
    EditLastOutput,
    CopyLastOutput,
    ToggleFloatingTerminal,
    NewPaneRight,
    NewPaneDown,
    FocusLeft,
    FocusRight,
    FocusUp,
    FocusDown,
    FocusNextPane,
    MovePaneLeft,
    MovePaneRight,
    MovePaneUp,
    MovePaneDown,
    ClosePane,
    ResizePaneLeft,
    ResizePaneRight,
    ResizePaneUp,
    ResizePaneDown,
    TogglePaneZoom,
    SearchHistory,
    NextSearchMatch,
    PreviousSearchMatch,
    ToggleHistorySelection,
    SelectionLeft,
    SelectionRight,
    CopySelection,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub(super) shell: Option<String>,
    pub(super) autosave_interval_seconds: u64,
    pub default_mode: String,
    compact: bool,
    scrollback_lines: usize,
    notifications_enabled: bool,
    command_duration_seconds: u64,
    notification_excluded_applications: Vec<String>,
    bindings: HashMap<String, HashMap<String, Binding>>,
    session_manager: HashMap<String, Vec<String>>,
}

#[derive(Clone, Debug)]
struct Binding {
    actions: Vec<Action>,
    display: BindingDisplay,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
enum BindingDisplay {
    #[default]
    Always,
    #[serde(alias = "help-menu")]
    Help,
    #[serde(alias = "never")]
    Hidden,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    shell: Option<String>,
    autosave_interval_seconds: Option<u64>,
    default_mode: Option<String>,
    #[serde(default)]
    clear_defaults: bool,
    compact: Option<bool>,
    scrollback_lines: Option<usize>,
    notifications: Option<NotificationsFile>,
    #[serde(default)]
    session_manager: HashMap<String, Vec<String>>,
    #[serde(default)]
    keybinds: HashMap<String, HashMap<String, BindingSpec>>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct NotificationsFile {
    enabled: Option<bool>,
    command_duration_seconds: Option<u64>,
    exclude_applications: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ActionSpec {
    Name(String),
    Detailed {
        action: String,
        mode: Option<String>,
        index: Option<usize>,
        key: Option<String>,
    },
}

#[derive(Deserialize)]
#[serde(untagged)]
enum BindingSpec {
    Actions(Vec<ActionSpec>),
    Detailed(BindingFile),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BindingFile {
    actions: Option<Vec<ActionSpec>>,
    display: Option<BindingDisplay>,
}

impl Config {
    #[cfg(test)]
    pub(super) fn test_defaults() -> Self {
        Self::from_file(toml::from_str(DEFAULT_CONFIG_TOML).unwrap(), None).unwrap()
    }

    pub fn load() -> Result<Self, String> {
        Self::load_from_path(&config_path())
    }

    fn load_from_path(path: &Path) -> Result<Self, String> {
        Self::from_snapshot(path, config_snapshot(path))
    }

    fn from_snapshot(path: &Path, snapshot: ConfigSnapshot) -> Result<Self, String> {
        let defaults: ConfigFile = toml::from_str(DEFAULT_CONFIG_TOML)
            .map_err(|error| format!("invalid built-in config: {error}"))?;
        let mut config = Self::from_file(defaults, None)?;
        let source = match snapshot {
            ConfigSnapshot::Missing => return Ok(config),
            ConfigSnapshot::Contents(contents) => String::from_utf8(contents)
                .map_err(|error| format!("invalid UTF-8 in {}: {error}", path.display()))?,
            ConfigSnapshot::ReadError(error) => {
                return Err(format!("could not read {}: {error}", path.display()));
            }
        };
        let user: ConfigFile = toml::from_str(&source)
            .map_err(|error| format!("invalid {}: {error}", path.display()))?;
        config.apply_user(user)?;
        Ok(config)
    }

    fn apply_user(&mut self, user: ConfigFile) -> Result<(), String> {
        if let Some(seconds) = user.autosave_interval_seconds {
            self.autosave_interval_seconds = seconds;
        }
        if let Some(shell) = user.shell {
            self.shell = Some(validate_shell(shell)?);
        }
        if user.clear_defaults {
            self.bindings.clear();
        }
        if let Some(mode) = user.default_mode {
            self.default_mode = normalize_mode(&mode)?;
        }
        if let Some(compact) = user.compact {
            self.compact = compact;
        }
        if let Some(lines) = user.scrollback_lines {
            self.scrollback_lines = validate_scrollback_lines(lines)?;
        }
        if let Some(notifications) = user.notifications {
            if let Some(enabled) = notifications.enabled {
                self.notifications_enabled = enabled;
            }
            if let Some(seconds) = notifications.command_duration_seconds {
                self.command_duration_seconds = seconds;
            }
            if let Some(applications) = notifications.exclude_applications {
                self.notification_excluded_applications = normalize_applications(applications)?;
            }
        }
        self.merge_session_manager(user.session_manager)?;
        self.merge_bindings(user.keybinds)?;
        self.validate()
    }

    fn from_file(file: ConfigFile, fallback_mode: Option<&str>) -> Result<Self, String> {
        let default_mode = normalize_mode(
            file.default_mode
                .as_deref()
                .or(fallback_mode)
                .unwrap_or("locked"),
        )?;
        let notifications = file.notifications.unwrap_or_default();
        let mut config = Self {
            shell: file.shell.map(validate_shell).transpose()?,
            autosave_interval_seconds: file.autosave_interval_seconds.unwrap_or(30),
            default_mode,
            compact: file.compact.unwrap_or(false),
            scrollback_lines: validate_scrollback_lines(
                file.scrollback_lines.unwrap_or(SCROLLBACK_LINES),
            )?,
            notifications_enabled: notifications.enabled.unwrap_or(true),
            command_duration_seconds: notifications.command_duration_seconds.unwrap_or(10),
            notification_excluded_applications: normalize_applications(
                notifications.exclude_applications.unwrap_or_default(),
            )?,
            bindings: HashMap::new(),
            session_manager: HashMap::new(),
        };
        config.merge_session_manager(file.session_manager)?;
        config.merge_bindings(file.keybinds)?;
        config.validate()?;
        Ok(config)
    }

    fn merge_session_manager(
        &mut self,
        bindings: HashMap<String, Vec<String>>,
    ) -> Result<(), String> {
        for (action, keys) in bindings {
            if !matches!(
                action.as_str(),
                "up" | "down"
                    | "search"
                    | "complete"
                    | "open"
                    | "rename"
                    | "save"
                    | "delete"
                    | "disconnect"
                    | "cancel"
                    | "backspace"
            ) {
                return Err(format!("unknown session manager action '{action}'"));
            }
            let keys = keys
                .iter()
                .map(|key| canonical_key_name(key))
                .collect::<Result<Vec<_>, _>>()?;
            self.session_manager.insert(action, keys);
        }
        let mut seen = HashMap::new();
        for (action, keys) in &self.session_manager {
            for key in keys {
                if let Some(previous) = seen.insert(key, action)
                    && previous != action
                {
                    return Err(format!(
                        "session manager key '{key}' is bound to both '{previous}' and '{action}'"
                    ));
                }
            }
        }
        Ok(())
    }

    pub(super) fn session_manager_action(&self, key: &str, editing: bool) -> Option<&str> {
        self.session_manager.iter().find_map(|(action, keys)| {
            // Printable navigation/action keys remain text while editing a name.
            let accepts_text = matches!(action.as_str(), "cancel" | "open" | "backspace");
            (keys.iter().any(|candidate| candidate == key)
                && (!editing || key.chars().count() != 1 || accepts_text))
                .then_some(action.as_str())
        })
    }

    pub(super) fn session_manager_keys(&self) -> HashMap<String, Vec<String>> {
        self.session_manager.clone()
    }

    fn merge_bindings(
        &mut self,
        modes: HashMap<String, HashMap<String, BindingSpec>>,
    ) -> Result<(), String> {
        for (mode, bindings) in modes {
            let mode = normalize_mode(&mode)?;
            let target = self.bindings.entry(mode.clone()).or_default();
            for (key, binding) in bindings {
                let key = canonical_key_name(&key)?;
                match binding {
                    BindingSpec::Actions(actions) => {
                        let actions = parse_actions(actions)?;
                        if actions.is_empty() {
                            target.remove(&key);
                        } else {
                            target.insert(
                                key,
                                Binding {
                                    actions,
                                    display: BindingDisplay::Always,
                                },
                            );
                        }
                    }
                    BindingSpec::Detailed(binding) => {
                        let actions = binding.actions.map(parse_actions).transpose()?;
                        if actions.as_ref().is_some_and(Vec::is_empty) {
                            target.remove(&key);
                            continue;
                        }
                        if let Some(existing) = target.get_mut(&key) {
                            if let Some(actions) = actions {
                                existing.actions = actions;
                            }
                            if let Some(display) = binding.display {
                                existing.display = display;
                            }
                        } else {
                            let actions = actions.ok_or_else(|| {
                                format!(
                                    "binding '{key}' in mode '{mode}' needs actions because it does not already exist"
                                )
                            })?;
                            target.insert(
                                key,
                                Binding {
                                    actions,
                                    display: binding.display.unwrap_or_default(),
                                },
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), String> {
        if !self.bindings.contains_key(&self.default_mode) {
            return Err(format!(
                "default_mode '{}' has no keybinds table",
                self.default_mode
            ));
        }
        for bindings in self.bindings.values() {
            for binding in bindings.values() {
                for action in &binding.actions {
                    if let Action::SwitchMode(mode) = action
                        && !self.bindings.contains_key(mode)
                    {
                        return Err(format!("switch-mode target '{mode}' has no keybinds table"));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn actions(&self, mode: &str, key: &str) -> Option<&[Action]> {
        self.bindings
            .get(mode)
            .and_then(|bindings| bindings.get(key))
            .map(|binding| binding.actions.as_slice())
    }

    pub fn has_mode(&self, mode: &str) -> bool {
        self.bindings.contains_key(mode)
    }

    pub fn command_notification_seconds(&self) -> Option<u64> {
        self.notifications_enabled
            .then_some(self.command_duration_seconds)
    }

    pub fn notification_excludes_application(&self, application: Option<&str>) -> bool {
        let Some(application) = application else {
            return false;
        };
        let application = Path::new(application)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(application);
        self.notification_excluded_applications
            .iter()
            .any(|excluded| excluded.eq_ignore_ascii_case(application))
    }

    pub fn notification_excludes_any_application<'a>(
        &self,
        applications: impl IntoIterator<Item = &'a str>,
    ) -> bool {
        applications
            .into_iter()
            .any(|application| self.notification_excludes_application(Some(application)))
    }

    pub fn compact(&self) -> bool {
        self.compact
    }

    pub fn scrollback_lines(&self) -> usize {
        self.scrollback_lines
    }

    pub fn describe_status_mode(&self, mode: &str) -> Vec<String> {
        self.describe_bindings(mode, |display| display == BindingDisplay::Always)
    }

    pub fn describe_help_mode(&self, mode: &str) -> Vec<String> {
        self.describe_bindings(mode, |display| display != BindingDisplay::Hidden)
    }

    fn describe_bindings(
        &self,
        mode: &str,
        include: impl Fn(BindingDisplay) -> bool,
    ) -> Vec<String> {
        let Some(bindings) = self.bindings.get(mode) else {
            return Vec::new();
        };
        let mut descriptions = bindings
            .iter()
            .filter(|(_, binding)| include(binding.display))
            .map(|(key, binding)| {
                let actions = binding
                    .actions
                    .iter()
                    .map(Action::label)
                    .collect::<Vec<_>>()
                    .join(" + ");
                format!("{key}={actions}")
            })
            .collect::<Vec<_>>();
        descriptions.sort();
        descriptions
    }
}

impl Action {
    fn label(&self) -> String {
        match self {
            Self::SwitchMode(mode) => format!("mode:{mode}"),
            Self::SendPrefix => "send-prefix".to_owned(),
            Self::SendKey(_) => "send-key".to_owned(),
            Self::NewWindow => "new-window".to_owned(),
            Self::RenameWindow => "rename-window".to_owned(),
            Self::NextWindow => "next-window".to_owned(),
            Self::PreviousWindow => "previous-window".to_owned(),
            Self::MoveWindowLeft => "move-window-left".to_owned(),
            Self::MoveWindowRight => "move-window-right".to_owned(),
            Self::SwitchSession => "switch-session".to_owned(),
            Self::GoToWindow(index) => format!("window:{index}"),
            Self::CloseWindow => "close-window".to_owned(),
            Self::Detach => "detach".to_owned(),
            Self::ShowHelp => "show-help".to_owned(),
            Self::ScrollUp => "scroll-up".to_owned(),
            Self::ScrollDown => "scroll-down".to_owned(),
            Self::PageUp => "page-up".to_owned(),
            Self::PageDown => "page-down".to_owned(),
            Self::ScrollTop => "scroll-top".to_owned(),
            Self::ScrollBottom => "scroll-bottom".to_owned(),
            Self::EditHistory => "edit-history".to_owned(),
            Self::EditLastOutput => "edit-last-output".to_owned(),
            Self::CopyLastOutput => "copy-last-output".to_owned(),
            Self::ToggleFloatingTerminal => "toggle-floating-terminal".to_owned(),
            Self::NewPaneRight => "new-pane-right".to_owned(),
            Self::NewPaneDown => "new-pane-down".to_owned(),
            Self::FocusLeft => "focus-left".to_owned(),
            Self::FocusRight => "focus-right".to_owned(),
            Self::FocusUp => "focus-up".to_owned(),
            Self::FocusDown => "focus-down".to_owned(),
            Self::FocusNextPane => "focus-next-pane".to_owned(),
            Self::MovePaneLeft => "move-pane-left".to_owned(),
            Self::MovePaneRight => "move-pane-right".to_owned(),
            Self::MovePaneUp => "move-pane-up".to_owned(),
            Self::MovePaneDown => "move-pane-down".to_owned(),
            Self::ClosePane => "close-pane".to_owned(),
            Self::ResizePaneLeft => "resize-left".to_owned(),
            Self::ResizePaneRight => "resize-right".to_owned(),
            Self::ResizePaneUp => "resize-up".to_owned(),
            Self::ResizePaneDown => "resize-down".to_owned(),
            Self::TogglePaneZoom => "zoom-pane".to_owned(),
            Self::SearchHistory => "search".to_owned(),
            Self::NextSearchMatch => "next-match".to_owned(),
            Self::PreviousSearchMatch => "previous-match".to_owned(),
            Self::ToggleHistorySelection => "select".to_owned(),
            Self::SelectionLeft => "select-left".to_owned(),
            Self::SelectionRight => "select-right".to_owned(),
            Self::CopySelection => "copy-selection".to_owned(),
        }
    }
}

fn parse_actions(actions: Vec<ActionSpec>) -> Result<Vec<Action>, String> {
    actions
        .into_iter()
        .map(Action::try_from)
        .collect::<Result<Vec<_>, _>>()
}

impl TryFrom<ActionSpec> for Action {
    type Error = String;

    fn try_from(value: ActionSpec) -> Result<Self, Self::Error> {
        match value {
            ActionSpec::Name(name) => parse_action(&name, None, None, None),
            ActionSpec::Detailed {
                action,
                mode,
                index,
                key,
            } => parse_action(&action, mode, index, key),
        }
    }
}

fn parse_action(
    name: &str,
    mode: Option<String>,
    index: Option<usize>,
    key: Option<String>,
) -> Result<Action, String> {
    let normalized = name.trim().to_ascii_lowercase().replace('_', "-");
    let no_arguments = || {
        if mode.is_some() || index.is_some() || key.is_some() {
            Err(format!("action '{name}' does not accept arguments"))
        } else {
            Ok(())
        }
    };
    let action = match normalized.as_str() {
        "switch-mode" => {
            if index.is_some() || key.is_some() {
                return Err("switch-mode accepts only the 'mode' argument".to_owned());
            }
            Action::SwitchMode(normalize_mode(
                mode.as_deref()
                    .ok_or_else(|| "switch-mode requires 'mode'".to_owned())?,
            )?)
        }
        "send-key" => {
            if mode.is_some() || index.is_some() {
                return Err("send-key accepts only the 'key' argument".to_owned());
            }
            Action::SendKey(parse_send_key(
                key.as_deref()
                    .ok_or_else(|| "send-key requires 'key'".to_owned())?,
            )?)
        }
        "go-to-window" => {
            if mode.is_some() || key.is_some() {
                return Err("go-to-window accepts only the 'index' argument".to_owned());
            }
            let index = index.ok_or_else(|| "go-to-window requires 'index'".to_owned())?;
            if index == 0 {
                return Err("go-to-window index starts at 1".to_owned());
            }
            Action::GoToWindow(index)
        }
        "send-prefix" => {
            no_arguments()?;
            Action::SendPrefix
        }
        "new-window" => {
            no_arguments()?;
            Action::NewWindow
        }
        "rename-window" => {
            no_arguments()?;
            Action::RenameWindow
        }
        "next-window" => {
            no_arguments()?;
            Action::NextWindow
        }
        "previous-window" => {
            no_arguments()?;
            Action::PreviousWindow
        }
        "move-window-left" => {
            no_arguments()?;
            Action::MoveWindowLeft
        }
        "move-window-right" => {
            no_arguments()?;
            Action::MoveWindowRight
        }
        "switch-session" => {
            no_arguments()?;
            Action::SwitchSession
        }
        "close-window" => {
            no_arguments()?;
            Action::CloseWindow
        }
        "detach" => {
            no_arguments()?;
            Action::Detach
        }
        "show-help" => {
            no_arguments()?;
            Action::ShowHelp
        }
        "scroll-up" => {
            no_arguments()?;
            Action::ScrollUp
        }
        "scroll-down" => {
            no_arguments()?;
            Action::ScrollDown
        }
        "page-up" => {
            no_arguments()?;
            Action::PageUp
        }
        "page-down" => {
            no_arguments()?;
            Action::PageDown
        }
        "scroll-top" => {
            no_arguments()?;
            Action::ScrollTop
        }
        "scroll-bottom" => {
            no_arguments()?;
            Action::ScrollBottom
        }
        "edit-history" => {
            no_arguments()?;
            Action::EditHistory
        }
        "edit-last-output" => {
            no_arguments()?;
            Action::EditLastOutput
        }
        "copy-last-output" => {
            no_arguments()?;
            Action::CopyLastOutput
        }
        "toggle-floating-terminal" => {
            no_arguments()?;
            Action::ToggleFloatingTerminal
        }
        "new-pane-right" => {
            no_arguments()?;
            Action::NewPaneRight
        }
        "new-pane-down" => {
            no_arguments()?;
            Action::NewPaneDown
        }
        "focus-left" => {
            no_arguments()?;
            Action::FocusLeft
        }
        "focus-right" => {
            no_arguments()?;
            Action::FocusRight
        }
        "focus-up" => {
            no_arguments()?;
            Action::FocusUp
        }
        "focus-down" => {
            no_arguments()?;
            Action::FocusDown
        }
        "focus-next-pane" => {
            no_arguments()?;
            Action::FocusNextPane
        }
        "move-pane-left" => {
            no_arguments()?;
            Action::MovePaneLeft
        }
        "move-pane-right" => {
            no_arguments()?;
            Action::MovePaneRight
        }
        "move-pane-up" => {
            no_arguments()?;
            Action::MovePaneUp
        }
        "move-pane-down" => {
            no_arguments()?;
            Action::MovePaneDown
        }
        "close-pane" => {
            no_arguments()?;
            Action::ClosePane
        }
        "resize-pane-left" => {
            no_arguments()?;
            Action::ResizePaneLeft
        }
        "resize-pane-right" => {
            no_arguments()?;
            Action::ResizePaneRight
        }
        "resize-pane-up" => {
            no_arguments()?;
            Action::ResizePaneUp
        }
        "resize-pane-down" => {
            no_arguments()?;
            Action::ResizePaneDown
        }
        "toggle-pane-zoom" => {
            no_arguments()?;
            Action::TogglePaneZoom
        }
        "search-history" => {
            no_arguments()?;
            Action::SearchHistory
        }
        "next-search-match" => {
            no_arguments()?;
            Action::NextSearchMatch
        }
        "previous-search-match" => {
            no_arguments()?;
            Action::PreviousSearchMatch
        }
        "toggle-history-selection" => {
            no_arguments()?;
            Action::ToggleHistorySelection
        }
        "selection-left" => {
            no_arguments()?;
            Action::SelectionLeft
        }
        "selection-right" => {
            no_arguments()?;
            Action::SelectionRight
        }
        "copy-selection" => {
            no_arguments()?;
            Action::CopySelection
        }
        _ => return Err(format!("unknown action '{name}'")),
    };
    Ok(action)
}

fn validate_shell(shell: String) -> Result<String, String> {
    if shell.trim().is_empty() || shell.contains('\0') {
        return Err("shell must be a nonempty executable name or path".to_owned());
    }
    Ok(shell)
}

fn normalize_mode(mode: &str) -> Result<String, String> {
    let mode = mode.trim().to_ascii_lowercase();
    if mode.is_empty()
        || !mode
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(format!("invalid mode name '{mode}'"));
    }
    Ok(mode)
}

fn validate_scrollback_lines(lines: usize) -> Result<usize, String> {
    (1..=1_000_000)
        .contains(&lines)
        .then_some(lines)
        .ok_or_else(|| "scrollback_lines must be between 1 and 1000000".to_owned())
}

fn normalize_applications(applications: Vec<String>) -> Result<Vec<String>, String> {
    let mut normalized = Vec::new();
    for application in applications {
        let application = application.trim();
        if application.is_empty() {
            return Err("notification application names cannot be empty".to_owned());
        }
        let name = Path::new(application)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(application)
            .to_owned();
        if !normalized
            .iter()
            .any(|existing: &String| existing.eq_ignore_ascii_case(&name))
        {
            normalized.push(name);
        }
    }
    Ok(normalized)
}

pub fn canonical_key_name(key: &str) -> Result<String, String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err("key name cannot be empty".to_owned());
    }
    let parts = trimmed.split_whitespace().collect::<Vec<_>>();
    if parts.len() == 2
        && (parts[0].eq_ignore_ascii_case("ctrl") || parts[0].eq_ignore_ascii_case("alt"))
    {
        let modifier = parts[0].to_ascii_lowercase();
        let key = parts[1].to_ascii_lowercase();
        if key.chars().count() == 1
            && (modifier == "alt" || key.as_bytes()[0].is_ascii_alphabetic())
        {
            return Ok(format!("{modifier} {key}"));
        }
        return Err(format!("unsupported control key '{trimmed}'"));
    }
    if parts.len() != 1 {
        return Err(format!("unsupported key '{trimmed}'"));
    }
    let canonical = match trimmed.to_ascii_lowercase().as_str() {
        "escape" => "esc".to_owned(),
        "del" => "delete".to_owned(),
        "return" => "enter".to_owned(),
        "pgup" | "page-up" => "pageup".to_owned(),
        "pgdn" | "page-down" => "pagedown".to_owned(),
        "up" | "down" | "left" | "right" | "enter" | "tab" | "backspace" | "esc" | "pageup"
        | "pagedown" | "delete" => trimmed.to_ascii_lowercase(),
        _ if trimmed.chars().count() == 1 => trimmed.to_owned(),
        _ => return Err(format!("unsupported key '{trimmed}'")),
    };
    Ok(canonical)
}

fn parse_send_key(key: &str) -> Result<Vec<u8>, String> {
    let canonical = canonical_key_name(key)?;
    if let Some(letter) = canonical.strip_prefix("ctrl ") {
        return Ok(vec![letter.as_bytes()[0] - b'a' + 1]);
    }
    if let Some(character) = canonical.strip_prefix("alt ") {
        let mut bytes = vec![0x1b];
        bytes.extend_from_slice(character.as_bytes());
        return Ok(bytes);
    }
    match canonical.as_str() {
        "enter" => Ok(vec![b'\r']),
        "tab" => Ok(vec![b'\t']),
        "backspace" => Ok(vec![0x7f]),
        "esc" => Ok(vec![0x1b]),
        "up" => Ok(b"\x1b[A".to_vec()),
        "down" => Ok(b"\x1b[B".to_vec()),
        "right" => Ok(b"\x1b[C".to_vec()),
        "left" => Ok(b"\x1b[D".to_vec()),
        "pageup" => Ok(b"\x1b[5~".to_vec()),
        "pagedown" => Ok(b"\x1b[6~".to_vec()),
        "delete" => Ok(b"\x1b[3~".to_vec()),
        _ => Ok(canonical.into_bytes()),
    }
}

pub fn config_path() -> PathBuf {
    if let Some(directory) = env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(directory).join("rustmux/config.toml")
    } else if let Some(home) = env::var_os("HOME") {
        PathBuf::from(home).join(".config/rustmux/config.toml")
    } else {
        PathBuf::from(".config/rustmux/config.toml")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_manager_bindings_distinguish_navigation_from_text() {
        let config = Config::test_defaults();
        assert_eq!(config.session_manager_action("j", false), Some("down"));
        assert_eq!(config.session_manager_action("k", false), Some("up"));
        assert_eq!(config.session_manager_action("/", false), Some("search"));
        assert_eq!(config.session_manager_action("j", true), None);
        assert_eq!(config.session_manager_action("k", true), None);
        assert_eq!(config.session_manager_action("down", true), Some("down"));
        assert_eq!(config.session_manager_action("esc", true), Some("cancel"));
        assert_eq!(canonical_key_name("Del").unwrap(), "delete");
        assert_eq!(parse_send_key("delete").unwrap(), b"\x1b[3~");
    }

    #[test]
    fn session_manager_bindings_can_be_replaced_disabled_and_validated() {
        let mut config = Config::test_defaults();
        config
            .apply_user(
                toml::from_str(
                    r#"
[session_manager]
down = ["n"]
up = ["p"]
search = ["Ctrl f"]
delete = []
"#,
                )
                .unwrap(),
            )
            .unwrap();
        assert_eq!(config.session_manager_action("j", false), None);
        assert_eq!(config.session_manager_action("n", false), Some("down"));
        assert_eq!(config.session_manager_action("n", true), None);
        assert_eq!(
            config.session_manager_action("ctrl f", false),
            Some("search")
        );
        assert_eq!(config.session_manager_action("delete", false), None);
        assert!(
            config
                .apply_user(toml::from_str("[session_manager]\nunknown = []").unwrap())
                .is_err()
        );
        assert!(
            config
                .apply_user(toml::from_str("[session_manager]\nup = [\"n\"]").unwrap())
                .is_err()
        );
    }

    #[test]
    fn default_config_has_zellij_style_modes() {
        let file: ConfigFile = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();
        let config = Config::from_file(file, None).unwrap();

        assert_eq!(config.default_mode, "locked");
        assert!(!config.compact());
        assert_eq!(
            config.actions("locked", "ctrl b"),
            Some(&[Action::SwitchMode("normal".to_owned())][..])
        );
        assert!(config.actions("scroll", "E").is_some());
        assert_eq!(
            config.actions("normal", ","),
            Some(&[Action::RenameWindow][..])
        );
        assert_eq!(
            config.actions("normal", "s"),
            Some(&[Action::SwitchSession][..])
        );
        assert_eq!(
            config.actions("normal", "i"),
            Some(
                &[
                    Action::SwitchMode("locked".to_owned()),
                    Action::ToggleFloatingTerminal
                ][..]
            )
        );
        assert_eq!(
            config.actions("normal", "<"),
            Some(
                &[
                    Action::MoveWindowLeft,
                    Action::SwitchMode("locked".to_owned())
                ][..]
            )
        );
        assert_eq!(config.command_notification_seconds(), Some(10));
        assert!(config.notification_excludes_application(Some("yazi")));
        assert!(config.notification_excludes_application(Some("nvim")));
        assert_eq!(
            config.actions("pane", "r"),
            Some(
                &[
                    Action::NewPaneRight,
                    Action::SwitchMode("locked".to_owned())
                ][..]
            )
        );
        assert_eq!(
            config.actions("pane", "alt h"),
            Some(&[Action::MovePaneLeft][..])
        );
    }

    #[test]
    fn key_names_are_canonicalized_without_losing_shift() {
        assert_eq!(canonical_key_name("Ctrl B").unwrap(), "ctrl b");
        assert_eq!(canonical_key_name("Escape").unwrap(), "esc");
        assert_eq!(canonical_key_name("G").unwrap(), "G");
    }

    #[test]
    fn parameterized_actions_reject_unrelated_arguments() {
        assert_eq!(
            parse_action("switch-mode", Some("normal".to_owned()), Some(1), None).unwrap_err(),
            "switch-mode accepts only the 'mode' argument"
        );
        assert_eq!(
            parse_action(
                "send-key",
                Some("normal".to_owned()),
                None,
                Some("x".to_owned())
            )
            .unwrap_err(),
            "send-key accepts only the 'key' argument"
        );
        assert_eq!(
            parse_action("go-to-window", None, Some(1), Some("x".to_owned())).unwrap_err(),
            "go-to-window accepts only the 'index' argument"
        );
    }

    #[test]
    fn user_config_overlays_and_unbinds_defaults() {
        let defaults: ConfigFile = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();
        let mut config = Config::from_file(defaults, None).unwrap();
        let user: ConfigFile = toml::from_str(
            r#"
[keybinds.normal]
c = []
z = ["new-window"]
"#,
        )
        .unwrap();

        config.apply_user(user).unwrap();

        assert!(config.actions("normal", "c").is_none());
        assert_eq!(
            config.actions("normal", "z"),
            Some(&[Action::NewWindow][..])
        );
        assert!(config.actions("locked", "ctrl b").is_some());
    }

    #[test]
    fn binding_display_controls_status_and_help_visibility() {
        let defaults: ConfigFile = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();
        let mut config = Config::from_file(defaults, None).unwrap();
        let user: ConfigFile = toml::from_str(
            r#"
[keybinds.normal]
c = { display = "help" }
n = { display = "hidden" }
z = { actions = ["new-window"], display = "help" }
"#,
        )
        .unwrap();

        config.apply_user(user).unwrap();

        assert_eq!(config.actions("normal", "c").unwrap()[0], Action::NewWindow);
        assert_eq!(
            config.actions("normal", "z"),
            Some(&[Action::NewWindow][..])
        );
        let status = config.describe_status_mode("normal");
        assert!(!status.iter().any(|hint| hint.starts_with("c=")));
        assert!(!status.iter().any(|hint| hint.starts_with("n=")));
        assert!(!status.iter().any(|hint| hint.starts_with("z=")));
        let help = config.describe_help_mode("normal");
        assert!(help.iter().any(|hint| hint.starts_with("c=")));
        assert!(!help.iter().any(|hint| hint.starts_with("n=")));
        assert!(help.iter().any(|hint| hint.starts_with("z=")));
    }

    #[test]
    fn binding_display_rejects_unknown_values_and_display_only_new_bindings() {
        assert!(
            toml::from_str::<ConfigFile>(
                r#"
[keybinds.normal]
c = { display = "sometimes" }
"#
            )
            .is_err()
        );

        let defaults: ConfigFile = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();
        let mut config = Config::from_file(defaults, None).unwrap();
        let user: ConfigFile = toml::from_str(
            r#"
[keybinds.normal]
z = { display = "help" }
"#,
        )
        .unwrap();
        assert_eq!(
            config.apply_user(user).unwrap_err(),
            "binding 'z' in mode 'normal' needs actions because it does not already exist"
        );
    }

    #[test]
    fn clear_defaults_supports_entirely_custom_modes() {
        let defaults: ConfigFile = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();
        let mut config = Config::from_file(defaults, None).unwrap();
        let user: ConfigFile = toml::from_str(
            r#"
default_mode = "base"
clear_defaults = true

[keybinds.base]
"Ctrl a" = [{ action = "switch-mode", mode = "command" }]

[keybinds.command]
q = [{ action = "switch-mode", mode = "base" }]
"#,
        )
        .unwrap();

        config.apply_user(user).unwrap();

        assert_eq!(config.default_mode, "base");
        assert!(!config.has_mode("locked"));
        assert!(config.actions("command", "q").is_some());
    }

    #[test]
    fn user_can_change_or_disable_command_notifications() {
        let defaults: ConfigFile = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();
        let mut config = Config::from_file(defaults, None).unwrap();
        let user: ConfigFile = toml::from_str(
            r#"
[notifications]
command_duration_seconds = 25
"#,
        )
        .unwrap();
        config.apply_user(user).unwrap();
        assert_eq!(config.command_notification_seconds(), Some(25));

        let disabled: ConfigFile = toml::from_str(
            r#"
[notifications]
enabled = false
"#,
        )
        .unwrap();
        config.apply_user(disabled).unwrap();
        assert_eq!(config.command_notification_seconds(), None);
    }

    #[test]
    fn user_can_exclude_applications_from_command_notifications() {
        let defaults: ConfigFile = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();
        let mut config = Config::from_file(defaults, None).unwrap();
        let user: ConfigFile = toml::from_str(
            r#"
[notifications]
exclude_applications = ["yazi", "/usr/bin/NVIM", "yazi"]
"#,
        )
        .unwrap();

        config.apply_user(user).unwrap();

        assert!(config.notification_excludes_application(Some("yazi")));
        assert!(config.notification_excludes_application(Some("/opt/homebrew/bin/nvim")));
        assert!(!config.notification_excludes_application(Some("cargo")));
        assert!(!config.notification_excludes_application(None));
        assert!(config.notification_excludes_any_application(["yazi", "cat", "rm"]));
        assert!(!config.notification_excludes_any_application(["cat", "rm"]));

        let clear: ConfigFile = toml::from_str(
            r#"
[notifications]
exclude_applications = []
"#,
        )
        .unwrap();
        config.apply_user(clear).unwrap();
        assert!(!config.notification_excludes_application(Some("yazi")));
    }

    #[test]
    fn empty_notification_application_name_is_rejected() {
        let defaults: ConfigFile = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();
        let mut config = Config::from_file(defaults, None).unwrap();
        let user: ConfigFile = toml::from_str(
            r#"
[notifications]
exclude_applications = ["  "]
"#,
        )
        .unwrap();

        assert_eq!(
            config.apply_user(user).unwrap_err(),
            "notification application names cannot be empty"
        );
    }

    #[test]
    fn user_can_enable_compact_layout() {
        let defaults: ConfigFile = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();
        let mut config = Config::from_file(defaults, None).unwrap();
        let user: ConfigFile = toml::from_str("compact = true").unwrap();

        config.apply_user(user).unwrap();

        assert!(config.compact());
    }

    #[test]
    fn user_can_configure_scrollback_capacity() {
        let defaults: ConfigFile = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();
        let mut config = Config::from_file(defaults, None).unwrap();
        let user: ConfigFile = toml::from_str("scrollback_lines = 25000").unwrap();
        config.apply_user(user).unwrap();
        assert_eq!(config.scrollback_lines(), 25_000);

        let invalid: ConfigFile = toml::from_str("scrollback_lines = 0").unwrap();
        assert!(config.apply_user(invalid).is_err());
    }

    #[test]
    fn config_reloader_applies_changes_recovers_from_errors_and_handles_deletion() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "rustmux-config-reload-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        let path = directory.join("config.toml");
        let mut reloader = ConfigReloader::new(path.clone());
        let start = Instant::now();

        fs::write(&path, "compact = true\n").unwrap();
        let config = reloader
            .poll(start + Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert!(config.compact());

        fs::write(&path, "compact = [invalid\n").unwrap();
        assert!(
            reloader
                .poll(start + Duration::from_secs(2))
                .unwrap()
                .is_err()
        );
        assert!(reloader.poll(start + Duration::from_secs(3)).is_none());

        fs::remove_file(&path).unwrap();
        let config = reloader
            .poll(start + Duration::from_secs(4))
            .unwrap()
            .unwrap();
        assert!(!config.compact());
        fs::remove_dir(&directory).unwrap();
    }
}
