use std::borrow::Cow;

use gpui::{
    App, AssetSource, Hsla, IntoElement, Pixels, RenderOnce, SharedString, Styled as _, Window, px,
    svg,
};

/// Phosphor icons, each in the weight its place calls for. Each variant names its
/// `phosphor_svgs` weight and const, and the Phosphor file stem its asset path is built from.
macro_rules! icons {
    ($($variant:ident => $weight:ident::$konst:ident, $stem:literal,)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum IconName { $($variant,)* }

        impl IconName {
            pub const ALL: &'static [IconName] = &[$(IconName::$variant,)*];

            pub fn path(self) -> &'static str {
                match self { $(Self::$variant => concat!("icons/", $stem, ".svg"),)* }
            }

            fn svg(self) -> &'static str {
                match self { $(Self::$variant => phosphor_svgs::style::$weight::$konst,)* }
            }
        }
    };
}

icons! {
    ArrowCounterClockwise => regular::ARROW_COUNTER_CLOCKWISE, "arrow-counter-clockwise",
    ArrowDown => regular::ARROW_DOWN, "arrow-down",
    ArrowLineDown => regular::ARROW_LINE_DOWN, "arrow-line-down",
    ArrowLineLeft => regular::ARROW_LINE_LEFT, "arrow-line-left",
    ArrowLineRight => regular::ARROW_LINE_RIGHT, "arrow-line-right",
    ArrowLineUp => regular::ARROW_LINE_UP, "arrow-line-up",
    ArrowUp => regular::ARROW_UP, "arrow-up",
    ArrowsDownUp => regular::ARROWS_DOWN_UP, "arrows-down-up",
    ArrowsLeftRight => regular::ARROWS_LEFT_RIGHT, "arrows-left-right",
    ArrowsOutLineHorizontal => regular::ARROWS_OUT_LINE_HORIZONTAL, "arrows-out-line-horizontal",
    ArrowsOut => regular::ARROWS_OUT, "arrows-out",
    CaretDown => regular::CARET_DOWN, "caret-down",
    CaretLeft => regular::CARET_LEFT, "caret-left",
    CaretLeftBold => bold::CARET_LEFT, "caret-left-bold",
    CaretRight => regular::CARET_RIGHT, "caret-right",
    CaretRightBold => bold::CARET_RIGHT, "caret-right-bold",
    CaretUpDown => regular::CARET_UP_DOWN, "caret-up-down",
    Check => regular::CHECK, "check",
    Code => regular::CODE, "code",
    Desktop => regular::DESKTOP, "desktop",
    DownloadSimple => regular::DOWNLOAD_SIMPLE, "download-simple",
    EyeSlash => regular::EYE_SLASH, "eye-slash",
    Faders => regular::FADERS, "faders",
    FilmStrip => regular::FILM_STRIP, "film-strip",
    FlipHorizontal => regular::FLIP_HORIZONTAL, "flip-horizontal",
    FlipVertical => regular::FLIP_VERTICAL, "flip-vertical",
    Image => regular::IMAGE, "image",
    ImageBroken => regular::IMAGE_BROKEN, "image-broken",
    LockSimple => regular::LOCK_SIMPLE, "lock-simple",
    Minus => regular::MINUS, "minus",
    Moon => regular::MOON, "moon",
    MonitorPlay => regular::MONITOR_PLAY, "monitor-play",
    PaintBrush => regular::PAINT_BRUSH, "paint-brush",
    Palette => regular::PALETTE, "palette",
    PauseFill => fill::PAUSE, "pause-fill",
    PlayFill => fill::PLAY, "play-fill",
    Plugs => regular::PLUGS, "plugs",
    Plus => regular::PLUS, "plus",
    SkipBackFill => fill::SKIP_BACK, "skip-back-fill",
    SquareSplitHorizontal => regular::SQUARE_SPLIT_HORIZONTAL, "square-split-horizontal",
    SquareSplitVertical => regular::SQUARE_SPLIT_VERTICAL, "square-split-vertical",
    Sun => regular::SUN, "sun",
    Timer => regular::TIMER, "timer",
    Trash => regular::TRASH, "trash",
    TreeStructure => regular::TREE_STRUCTURE, "tree-structure",
    TreeView => regular::TREE_VIEW, "tree-view",
    Warning => fill::WARNING, "warning-fill",
    X => regular::X, "x",
}

/// gpui-component names its built-in icons after Lucide; these paths resolve to the Phosphor
/// icon for the same job.
const COMPONENT_ICON_ALIASES: &[(&str, &str)] = &[
    ("icons/chevron-down.svg", "icons/caret-down.svg"),
    ("icons/chevron-left.svg", "icons/caret-left.svg"),
    ("icons/chevron-right.svg", "icons/caret-right.svg"),
    ("icons/close.svg", "icons/x.svg"),
];

fn icon_svg(path: &str) -> Option<&'static str> {
    let path = COMPONENT_ICON_ALIASES
        .iter()
        .find(|(from, _)| *from == path)
        .map_or(path, |(_, to)| to);
    IconName::ALL
        .iter()
        .find(|name| name.path() == path)
        .map(|name| name.svg())
}

/// The theme's embedded assets; pass to `Application::with_assets`.
pub struct Assets;

/// Also serves gpui-component's own icons, which its widgets draw.
impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        match icon_svg(path) {
            Some(svg) => Ok(Some(Cow::Borrowed(svg.as_bytes()))),
            None => gpui_kit_assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let mut listed: Vec<SharedString> = IconName::ALL
            .iter()
            .map(|name| name.path())
            .filter(|p| p.starts_with(path))
            .map(SharedString::from)
            .collect();
        listed.extend(gpui_kit_assets::Assets.list(path)?);
        Ok(listed)
    }
}

impl From<IconName> for gpui_component::Icon {
    fn from(name: IconName) -> Self {
        gpui_component::Icon::empty().path(name.path())
    }
}

#[derive(IntoElement)]
pub struct Icon {
    name: IconName,
    size: Pixels,
    color: Option<Hsla>,
}

impl Icon {
    pub fn new(name: IconName) -> Self {
        Self {
            name,
            size: px(16.0),
            color: None,
        }
    }

    pub fn size(mut self, size: Pixels) -> Self {
        self.size = size;
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl RenderOnce for Icon {
    fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
        guard::render("Icon", || {
            // SVGs are drawn as masks: like `currentColor`, the text colour by default.
            let color = self.color.unwrap_or_else(|| window.text_style().color);
            svg()
                .path(self.name.path())
                .size(self.size)
                .flex_none()
                .text_color(color)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_resolves_to_svg_through_asset_source() {
        for name in IconName::ALL {
            let loaded = Assets.load(name.path()).expect("asset source failed");
            let bytes = loaded.unwrap_or_else(|| panic!("{} is not served", name.path()));
            assert!(bytes.starts_with(b"<svg"), "{} is not an SVG", name.path());
        }
    }
}
