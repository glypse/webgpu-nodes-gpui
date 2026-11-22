use gpui::{App, Global, Hsla, Pixels, SharedString, Window, WindowAppearance, px};

use crate::color::{oklch, oklch_a};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
}

impl From<WindowAppearance> for Appearance {
    fn from(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::Light,
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::Dark,
        }
    }
}

/// The neutral palette the components are styled with.
#[derive(Clone, Debug)]
pub struct Colors {
    pub background: Hsla,
    pub foreground: Hsla,
    pub card: Hsla,
    pub card_foreground: Hsla,
    pub popover: Hsla,
    pub popover_foreground: Hsla,
    pub primary: Hsla,
    pub primary_foreground: Hsla,
    pub secondary: Hsla,
    pub secondary_foreground: Hsla,
    pub muted: Hsla,
    pub muted_foreground: Hsla,
    pub accent: Hsla,
    pub accent_foreground: Hsla,
    pub destructive: Hsla,
    pub border: Hsla,
    pub input: Hsla,
    pub ring: Hsla,
    /// Sockets of `f32` values; hue encodes the component count.
    pub value_f32: Hsla,
}

impl Colors {
    pub fn light() -> Self {
        Self {
            background: oklch(1.0, 0.0, 0.0),
            foreground: oklch(0.145, 0.0, 0.0),
            card: oklch(0.95, 0.0, 0.0),
            card_foreground: oklch(0.145, 0.0, 0.0),
            popover: oklch(1.0, 0.0, 0.0),
            popover_foreground: oklch(0.145, 0.0, 0.0),
            primary: oklch(0.205, 0.0, 0.0),
            primary_foreground: oklch(0.985, 0.0, 0.0),
            secondary: oklch(0.97, 0.0, 0.0),
            secondary_foreground: oklch(0.205, 0.0, 0.0),
            muted: oklch(0.97, 0.0, 0.0),
            muted_foreground: oklch(0.556, 0.0, 0.0),
            accent: oklch(0.97, 0.0, 0.0),
            accent_foreground: oklch(0.205, 0.0, 0.0),
            destructive: oklch(0.577, 0.245, 27.325),
            border: oklch(0.922, 0.0, 0.0),
            input: oklch(0.922, 0.0, 0.0),
            ring: oklch(0.708, 0.0, 0.0),
            value_f32: oklch(0.556, 0.0, 0.0),
        }
    }

    pub fn dark() -> Self {
        Self {
            background: oklch(0.145, 0.0, 0.0),
            foreground: oklch(0.985, 0.0, 0.0),
            card: oklch(0.2, 0.0, 0.0),
            card_foreground: oklch(0.985, 0.0, 0.0),
            popover: oklch(0.205, 0.0, 0.0),
            popover_foreground: oklch(0.985, 0.0, 0.0),
            primary: oklch(0.922, 0.0, 0.0),
            primary_foreground: oklch(0.205, 0.0, 0.0),
            secondary: oklch(0.269, 0.0, 0.0),
            secondary_foreground: oklch(0.985, 0.0, 0.0),
            muted: oklch(0.269, 0.0, 0.0),
            muted_foreground: oklch(0.708, 0.0, 0.0),
            accent: oklch(0.269, 0.0, 0.0),
            accent_foreground: oklch(0.985, 0.0, 0.0),
            destructive: oklch(0.704, 0.191, 22.216),
            border: oklch_a(1.0, 0.0, 0.0, 0.1),
            input: oklch_a(1.0, 0.0, 0.0, 0.15),
            ring: oklch(0.556, 0.0, 0.0),
            value_f32: oklch(0.556, 0.0, 0.0),
        }
    }
}

/// Radius scale around a base radius of 0.625rem.
#[derive(Clone, Copy, Debug)]
pub struct Radii {
    pub sm: Pixels,
    pub md: Pixels,
    pub lg: Pixels,
    pub xl: Pixels,
}

/// Spacing is GPUI's rem scale (`p_1` is 0.25rem).
#[derive(Clone, Copy, Debug)]
pub struct TextSizes {
    pub xs: Pixels,
    pub sm: Pixels,
    pub base: Pixels,
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub appearance: Appearance,
    pub colors: Colors,
    pub radius: Radii,
    pub text: TextSizes,
    /// For everything, type names and values included.
    pub font: SharedString,
    /// Only for actual code.
    pub mono_font: SharedString,
}

pub const INTERFACE_FONT: &str = "Atkinson Hyperlegible Next";

impl Theme {
    pub fn new(appearance: Appearance) -> Self {
        let radius = px(10.0);
        Self {
            appearance,
            colors: match appearance {
                Appearance::Light => Colors::light(),
                Appearance::Dark => Colors::dark(),
            },
            radius: Radii {
                sm: radius - px(4.0),
                md: radius - px(2.0),
                lg: radius,
                xl: radius + px(4.0),
            },
            text: TextSizes {
                xs: px(12.0),
                sm: px(14.0),
                base: px(16.0),
            },
            font: INTERFACE_FONT.into(),
            mono_font: "Menlo".into(),
        }
    }

    pub fn is_dark(&self) -> bool {
        self.appearance == Appearance::Dark
    }

    pub fn global(cx: &App) -> &Self {
        cx.global::<Self>()
    }

    /// Switches to the window's appearance, redrawing it when that changes.
    pub fn sync(window: &mut Window, cx: &mut App) {
        let appearance = Appearance::from(window.appearance());
        if cx.try_global::<Self>().map(|t| t.appearance) != Some(appearance) {
            cx.set_global(Self::new(appearance));
            window.refresh();
        }
    }
}

impl Global for Theme {}

/// `cx.theme()` from any view or element.
pub trait ActiveTheme {
    fn theme(&self) -> &Theme;
}

impl ActiveTheme for App {
    fn theme(&self) -> &Theme {
        Theme::global(self)
    }
}
