use std::borrow::Cow;

use gpui::{
    App, AssetSource, Hsla, IntoElement, Pixels, RenderOnce, SharedString, Styled as _, Window, px,
    svg,
};

macro_rules! icons {
    ($($variant:ident => $file:literal,)*) => {
        /// Phosphor icons, in the weight the original uses at each place.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum IconName { $($variant,)* }

        impl IconName {
            pub fn path(self) -> &'static str {
                match self { $(Self::$variant => concat!("icons/", $file),)* }
            }
        }

        const ICON_FILES: &[(&str, &[u8])] = &[
            $((concat!("icons/", $file), include_bytes!(concat!("../assets/icons/", $file))),)*
        ];
    };
}

icons! {
    ArrowCounterClockwise => "arrow-counter-clockwise.svg",
    ArrowDown => "arrow-down.svg",
    ArrowLineDown => "arrow-line-down.svg",
    ArrowLineLeft => "arrow-line-left.svg",
    ArrowLineRight => "arrow-line-right.svg",
    ArrowLineUp => "arrow-line-up.svg",
    ArrowUp => "arrow-up.svg",
    ArrowsDownUp => "arrows-down-up.svg",
    ArrowsIn => "arrows-in.svg",
    ArrowsLeftRight => "arrows-left-right.svg",
    ArrowsOutLineHorizontal => "arrows-out-line-horizontal.svg",
    ArrowsOut => "arrows-out.svg",
    CaretDown => "caret-down.svg",
    CaretLeft => "caret-left.svg",
    CaretLeftBold => "caret-left-bold.svg",
    CaretRight => "caret-right.svg",
    CaretRightBold => "caret-right-bold.svg",
    CaretUpDown => "caret-up-down.svg",
    Check => "check.svg",
    Code => "code.svg",
    Desktop => "desktop.svg",
    DownloadSimple => "download-simple.svg",
    EyeSlash => "eye-slash.svg",
    Faders => "faders.svg",
    FilmStrip => "film-strip.svg",
    FlipHorizontal => "flip-horizontal.svg",
    FlipVertical => "flip-vertical.svg",
    LockSimple => "lock-simple.svg",
    Minus => "minus.svg",
    Moon => "moon.svg",
    MonitorPlay => "monitor-play.svg",
    PaintBrush => "paint-brush.svg",
    Palette => "palette.svg",
    PauseFill => "pause-fill.svg",
    PlayFill => "play-fill.svg",
    Plugs => "plugs.svg",
    Plus => "plus.svg",
    SkipBackFill => "skip-back-fill.svg",
    SquareSplitHorizontal => "square-split-horizontal.svg",
    SquareSplitVertical => "square-split-vertical.svg",
    Sun => "sun.svg",
    Timer => "timer.svg",
    Trash => "trash.svg",
    TreeStructure => "tree-structure.svg",
    TreeView => "tree-view.svg",
    X => "x.svg",
}

/// gpui-component names its built-in icons after Lucide; these paths resolve to the Phosphor
/// icon the original uses for the same job.
const COMPONENT_ICON_ALIASES: &[(&str, &str)] = &[
    ("icons/chevron-down.svg", "icons/caret-down.svg"),
    ("icons/chevron-left.svg", "icons/caret-left.svg"),
    ("icons/chevron-right.svg", "icons/caret-right.svg"),
    ("icons/close.svg", "icons/x.svg"),
];

fn icon_bytes(path: &str) -> Option<&'static [u8]> {
    let path = COMPONENT_ICON_ALIASES
        .iter()
        .find(|(from, _)| *from == path)
        .map_or(path, |(_, to)| to);
    ICON_FILES.iter().find(|(p, _)| *p == path).map(|(_, bytes)| *bytes)
}

/// The theme's embedded assets; pass to `Application::with_assets`.
pub struct Assets;

/// Also serves gpui-component's own icons, which its widgets draw.
impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        match icon_bytes(path) {
            Some(bytes) => Ok(Some(Cow::Borrowed(bytes))),
            None => gpui_kit_assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let mut listed: Vec<SharedString> = ICON_FILES
            .iter()
            .filter(|(p, _)| p.starts_with(path))
            .map(|(p, _)| SharedString::from(*p))
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
