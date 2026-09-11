use serde::Deserialize;
use std::collections::BTreeMap;

pub(super) type Rgb = (u8, u8, u8);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Theme {
    pub badge_text: Rgb,
    pub background: Rgb,
    pub surface: Rgb,
    pub surface_highlight: Rgb,
    pub border: Rgb,
    pub muted: Rgb,
    pub foreground: Rgb,
    pub error: Rgb,
    pub accent: Rgb,
    pub orange: Rgb,
    pub warning: Rgb,
    pub blue: Rgb,
    pub secondary: Rgb,
    pub key: Rgb,
    pub purple: Rgb,
    pub teal: Rgb,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            badge_text: (17, 17, 27),
            background: (30, 30, 46),
            surface: (49, 50, 68),
            surface_highlight: (69, 71, 90),
            border: (108, 112, 134),
            muted: (166, 173, 200),
            foreground: (205, 214, 244),
            error: (243, 139, 168),
            accent: (166, 227, 161),
            orange: (250, 179, 135),
            warning: (249, 226, 175),
            blue: (137, 180, 250),
            secondary: (180, 190, 254),
            key: (245, 194, 231),
            purple: (203, 166, 247),
            teal: (148, 226, 213),
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ThemeConfig {
    #[serde(default)]
    preset: Preset,
    #[serde(default)]
    colors: BTreeMap<String, String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Preset {
    #[default]
    Mocha,
    Light,
}

impl ThemeConfig {
    pub(super) fn resolve(self) -> Result<Theme, String> {
        let mut theme = match self.preset {
            Preset::Mocha => Theme::default(),
            Preset::Light => Theme {
                badge_text: (255, 255, 255),
                background: (245, 246, 250),
                surface: (226, 230, 239),
                surface_highlight: (195, 202, 216),
                border: (103, 112, 132),
                muted: (85, 94, 114),
                foreground: (36, 43, 59),
                error: (174, 37, 58),
                accent: (24, 110, 74),
                orange: (165, 72, 17),
                warning: (135, 94, 0),
                blue: (38, 88, 174),
                secondary: (86, 73, 170),
                key: (163, 46, 119),
                purple: (113, 55, 170),
                teal: (16, 108, 117),
            },
        };
        for (name, value) in self.colors {
            let field = match name.as_str() {
                "badge_text" => &mut theme.badge_text,
                "background" => &mut theme.background,
                "surface" => &mut theme.surface,
                "surface_highlight" => &mut theme.surface_highlight,
                "border" => &mut theme.border,
                "muted" => &mut theme.muted,
                "foreground" => &mut theme.foreground,
                "error" => &mut theme.error,
                "accent" => &mut theme.accent,
                "orange" => &mut theme.orange,
                "warning" => &mut theme.warning,
                "blue" => &mut theme.blue,
                "secondary" => &mut theme.secondary,
                "key" => &mut theme.key,
                "purple" => &mut theme.purple,
                "teal" => &mut theme.teal,
                _ => return Err(format!("unknown theme color '{name}'")),
            };
            *field = parse_color(&value)
                .ok_or_else(|| format!("theme color '{name}' must be #RRGGBB, got '{value}'"))?;
        }
        Ok(theme)
    }
}

fn parse_color(value: &str) -> Option<Rgb> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 6 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some((
        u8::from_str_radix(&hex[..2], 16).ok()?,
        u8::from_str_radix(&hex[2..4], 16).ok()?,
        u8::from_str_radix(&hex[4..], 16).ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_accept_partial_overrides_without_changing_other_colors() {
        let config: ThemeConfig =
            toml::from_str("preset = 'light'\n[colors]\naccent = '#01Abef'\n").unwrap();
        let theme = config.resolve().unwrap();
        assert_eq!(theme.accent, (1, 171, 239));
        assert_eq!(theme.background, (245, 246, 250));
        assert_eq!(ThemeConfig::default().resolve().unwrap(), Theme::default());
    }

    #[test]
    fn theme_rejects_unknown_options_and_malformed_colors() {
        for source in [
            "preset = 'unknown'",
            "presett = 'light'",
            "[colors]\naccent = 123",
        ] {
            assert!(toml::from_str::<ThemeConfig>(source).is_err());
        }
        for source in [
            "[colors]\nacent = '#112233'",
            "[colors]\naccent = '#abc'",
            "[colors]\naccent = '112233'",
            "[colors]\naccent = '#gg0000'",
            "[colors]\naccent = '#é1234'",
        ] {
            assert!(
                toml::from_str::<ThemeConfig>(source)
                    .unwrap()
                    .resolve()
                    .is_err()
            );
        }
    }
}
