use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;

use serde::Deserialize;

pub const DEFAULT_CONFIG_TOML: &str = r#"
default_mode = "locked"
clear_defaults = false

[notifications]
enabled = true
command_duration_seconds = 10

[keybinds.locked]
"Ctrl b" = [{ action = "switch-mode", mode = "normal" }]

[keybinds.normal]
"Ctrl b" = ["send-prefix", { action = "switch-mode", mode = "locked" }]
c = ["new-window", { action = "switch-mode", mode = "locked" }]
n = ["next-window", { action = "switch-mode", mode = "locked" }]
p = ["previous-window", { action = "switch-mode", mode = "locked" }]
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
x = ["close-pane", { action = "switch-mode", mode = "locked" }]
q = [{ action = "switch-mode", mode = "locked" }]
esc = [{ action = "switch-mode", mode = "locked" }]

[keybinds.scroll]
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
y = ["copy-last-output", "scroll-bottom", { action = "switch-mode", mode = "locked" }]
q = ["scroll-bottom", { action = "switch-mode", mode = "locked" }]
esc = ["scroll-bottom", { action = "switch-mode", mode = "locked" }]
"#;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Action {
    SwitchMode(String),
    SendPrefix,
    SendKey(Vec<u8>),
    NewWindow,
    NextWindow,
    PreviousWindow,
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
    ClosePane,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub default_mode: String,
    notifications_enabled: bool,
    command_duration_seconds: u64,
    bindings: HashMap<String, HashMap<String, Vec<Action>>>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigFile {
    default_mode: Option<String>,
    #[serde(default)]
    clear_defaults: bool,
    notifications: Option<NotificationsFile>,
    #[serde(default)]
    keybinds: HashMap<String, HashMap<String, Vec<ActionSpec>>>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct NotificationsFile {
    enabled: Option<bool>,
    command_duration_seconds: Option<u64>,
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

impl Config {
    pub fn load() -> Result<Self, String> {
        let defaults: ConfigFile = toml::from_str(DEFAULT_CONFIG_TOML)
            .map_err(|error| format!("invalid built-in config: {error}"))?;
        let mut config = Self::from_file(defaults, None)?;
        let path = config_path();
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(config),
            Err(error) => return Err(format!("could not read {}: {error}", path.display())),
        };
        let user: ConfigFile = toml::from_str(&source)
            .map_err(|error| format!("invalid {}: {error}", path.display()))?;
        config.apply_user(user)?;
        Ok(config)
    }

    fn apply_user(&mut self, user: ConfigFile) -> Result<(), String> {
        if user.clear_defaults {
            self.bindings.clear();
        }
        if let Some(mode) = user.default_mode {
            self.default_mode = normalize_mode(&mode)?;
        }
        if let Some(notifications) = user.notifications {
            if let Some(enabled) = notifications.enabled {
                self.notifications_enabled = enabled;
            }
            if let Some(seconds) = notifications.command_duration_seconds {
                self.command_duration_seconds = seconds;
            }
        }
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
            default_mode,
            notifications_enabled: notifications.enabled.unwrap_or(true),
            command_duration_seconds: notifications.command_duration_seconds.unwrap_or(10),
            bindings: HashMap::new(),
        };
        config.merge_bindings(file.keybinds)?;
        config.validate()?;
        Ok(config)
    }

    fn merge_bindings(
        &mut self,
        modes: HashMap<String, HashMap<String, Vec<ActionSpec>>>,
    ) -> Result<(), String> {
        for (mode, bindings) in modes {
            let mode = normalize_mode(&mode)?;
            let target = self.bindings.entry(mode).or_default();
            for (key, actions) in bindings {
                let key = canonical_key_name(&key)?;
                let actions = actions
                    .into_iter()
                    .map(Action::try_from)
                    .collect::<Result<Vec<_>, _>>()?;
                if actions.is_empty() {
                    target.remove(&key);
                } else {
                    target.insert(key, actions);
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
            for actions in bindings.values() {
                for action in actions {
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
            .map(Vec::as_slice)
    }

    pub fn has_mode(&self, mode: &str) -> bool {
        self.bindings.contains_key(mode)
    }

    pub fn command_notification_seconds(&self) -> Option<u64> {
        self.notifications_enabled
            .then_some(self.command_duration_seconds)
    }

    pub fn describe_mode(&self, mode: &str) -> Vec<String> {
        let Some(bindings) = self.bindings.get(mode) else {
            return Vec::new();
        };
        let mut descriptions = bindings
            .iter()
            .map(|(key, actions)| {
                let actions = actions
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
            Self::NextWindow => "next-window".to_owned(),
            Self::PreviousWindow => "previous-window".to_owned(),
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
            Self::ClosePane => "close-pane".to_owned(),
        }
    }
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
        "next-window" => {
            no_arguments()?;
            Action::NextWindow
        }
        "previous-window" => {
            no_arguments()?;
            Action::PreviousWindow
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
        "close-pane" => {
            no_arguments()?;
            Action::ClosePane
        }
        _ => return Err(format!("unknown action '{name}'")),
    };
    Ok(action)
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
        "return" => "enter".to_owned(),
        "pgup" | "page-up" => "pageup".to_owned(),
        "pgdn" | "page-down" => "pagedown".to_owned(),
        "up" | "down" | "left" | "right" | "enter" | "tab" | "backspace" | "esc" | "pageup"
        | "pagedown" => trimmed.to_ascii_lowercase(),
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
    fn default_config_has_zellij_style_modes() {
        let file: ConfigFile = toml::from_str(DEFAULT_CONFIG_TOML).unwrap();
        let config = Config::from_file(file, None).unwrap();

        assert_eq!(config.default_mode, "locked");
        assert_eq!(
            config.actions("locked", "ctrl b"),
            Some(&[Action::SwitchMode("normal".to_owned())][..])
        );
        assert!(config.actions("scroll", "E").is_some());
        assert_eq!(
            config.actions("normal", "i"),
            Some(
                &[
                    Action::SwitchMode("locked".to_owned()),
                    Action::ToggleFloatingTerminal
                ][..]
            )
        );
        assert_eq!(config.command_notification_seconds(), Some(10));
        assert_eq!(
            config.actions("pane", "r"),
            Some(
                &[
                    Action::NewPaneRight,
                    Action::SwitchMode("locked".to_owned())
                ][..]
            )
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
}
