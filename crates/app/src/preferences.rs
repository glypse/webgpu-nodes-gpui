//! The user's own settings, kept in a file under the platform's config
//! directory rather than in projects.

use std::fs;
use std::path::PathBuf;

use gpui::{App, Global, Window, actions};
use serde::{Deserialize, Serialize};
use theme::{Appearance, Theme};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemePreference {
    Light,
    Dark,
    #[default]
    System,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub theme: ThemePreference,
    /// A font-family list for the interface; empty for the default.
    pub font: String,
    /// Unset, developer tools start on in debug builds only.
    pub developer_tools: Option<bool>,
}

actions!(preferences, [ToggleDeveloperTools]);

impl Global for Preferences {}

#[cfg(not(test))]
fn file() -> Option<PathBuf> {
    Some(
        dirs::config_dir()?
            .join("webgpu-nodes")
            .join("preferences.json"),
    )
}

/// Tests neither read nor write the user's preferences.
#[cfg(test)]
fn file() -> Option<PathBuf> {
    None
}

impl Preferences {
    pub fn developer_tools_enabled(&self) -> bool {
        self.developer_tools.unwrap_or(cfg!(debug_assertions))
    }

    /// What was saved, or the defaults when there is nothing readable.
    pub fn load() -> Self {
        let Some(file) = file() else {
            return Self::default();
        };
        match fs::read_to_string(&file) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
                log::warn!("ignoring unreadable preferences {}: {e}", file.display());
                Self::default()
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => {
                log::warn!("couldn't read preferences {}: {e}", file.display());
                Self::default()
            }
        }
    }

    pub fn save(&self) {
        let Some(file) = file() else { return };
        let result = (|| -> std::io::Result<()> {
            if let Some(dir) = file.parent() {
                fs::create_dir_all(dir)?;
            }
            let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
            let temporary = file.with_extension("json.tmp");
            fs::write(&temporary, json)?;
            fs::rename(&temporary, &file)
        })();
        if let Err(e) = result {
            log::warn!("couldn't save preferences: {e}");
        }
    }

    /// The first family of the list: GPUI takes one family name, not a comma-separated list.
    pub fn font_family(&self) -> Option<String> {
        let first = self
            .font
            .split(',')
            .next()?
            .trim()
            .trim_matches(['"', '\''])
            .trim();
        (!first.is_empty()).then(|| first.to_owned())
    }

    fn appearance(&self, window: &Window) -> Appearance {
        match self.theme {
            ThemePreference::Light => Appearance::Light,
            ThemePreference::Dark => Appearance::Dark,
            ThemePreference::System => window.appearance().into(),
        }
    }
}

pub fn init(cx: &mut App) {
    cx.set_global(Preferences::load());
    cx.on_action(|_: &ToggleDeveloperTools, cx| {
        guard::guard("developer tools", || toggle_developer_tools(cx));
    });
}

/// Turns developer tools on or off, saving the choice and redrawing the windows
/// so the add menu lists or drops the developer nodes.
pub fn toggle_developer_tools(cx: &mut App) {
    let mut preferences = cx.global::<Preferences>().clone();
    preferences.developer_tools = Some(!preferences.developer_tools_enabled());
    preferences.save();
    cx.set_global(preferences);
    cx.refresh_windows();
}

/// Whether developer tools are on, or the build's default before preferences are set up.
pub fn developer_tools_on(cx: &App) -> bool {
    cx.try_global::<Preferences>().map_or_else(
        || Preferences::default().developer_tools_enabled(),
        Preferences::developer_tools_enabled,
    )
}

/// Logs a shader's generated code while developer tools are on. Callers pass
/// only code that changed since their last call.
pub fn log_generated_code(cx: &App, code: &str) {
    if developer_tools_on(cx) {
        log::info!("[shader debug] compiled passes:\n{code}");
    }
}

/// Makes the theme follow the preferences (and the system's appearance for "system"),
/// redrawing the window when that changes it.
pub fn apply(window: &mut Window, cx: &mut App) {
    let preferences = cx.global::<Preferences>().clone();
    let appearance = preferences.appearance(window);
    let mut wanted = Theme::new(appearance);
    if let Some(family) = preferences.font_family() {
        wanted.font = family.into();
    }
    let current = cx.try_global::<Theme>();
    if current.is_none_or(|t| t.appearance != appearance || t.font != wanted.font) {
        cx.set_global(wanted);
        theme::sync_component_theme(appearance, cx);
        // Every window shows the theme, and this may be a change made in another one.
        cx.refresh_windows();
    }
}

/// Switches to the opposite of the appearance on screen.
pub fn toggle_mode(window: &mut Window, cx: &mut App) {
    let on_screen = cx.global::<Preferences>().appearance(window);
    let theme = match on_screen {
        Appearance::Dark => ThemePreference::Light,
        Appearance::Light => ThemePreference::Dark,
    };
    update(window, cx, |p| p.theme = theme);
}

/// Changes the preferences with `f`, saving and applying them.
pub fn update(window: &mut Window, cx: &mut App, f: impl FnOnce(&mut Preferences)) {
    let mut preferences = cx.global::<Preferences>().clone();
    f(&mut preferences);
    if preferences != *cx.global::<Preferences>() {
        preferences.save();
        cx.set_global(preferences);
    }
    apply(window, cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_font_is_the_first_family_of_the_list() {
        let p = |font: &str| Preferences {
            font: font.to_owned(),
            ..Preferences::default()
        };
        assert_eq!(
            p("Inter, system-ui").font_family().as_deref(),
            Some("Inter")
        );
        assert_eq!(
            p("\"Fira Sans\", sans-serif").font_family().as_deref(),
            Some("Fira Sans")
        );
        assert_eq!(p("  ").font_family(), None);
        assert_eq!(Preferences::default().font_family(), None);
    }

    #[test]
    fn preferences_round_trip_and_tolerate_missing_fields() {
        let p = Preferences {
            theme: ThemePreference::Dark,
            font: "Inter".into(),
            developer_tools: None,
        };
        let json = serde_json::to_string(&p).expect("serialises");
        assert_eq!(
            serde_json::from_str::<Preferences>(&json).expect("reads"),
            p
        );
        assert_eq!(
            serde_json::from_str::<Preferences>("{}").expect("reads"),
            Preferences::default()
        );
        assert_eq!(
            serde_json::from_str::<Preferences>(r#"{"theme":"light"}"#)
                .expect("reads")
                .theme,
            ThemePreference::Light
        );
    }

    #[test]
    fn developer_tools_follow_the_saved_choice_then_the_build() {
        let on = Preferences {
            developer_tools: Some(true),
            ..Preferences::default()
        };
        let off = Preferences {
            developer_tools: Some(false),
            ..Preferences::default()
        };
        assert!(on.developer_tools_enabled());
        assert!(!off.developer_tools_enabled());
    }
}
