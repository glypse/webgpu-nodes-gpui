use std::borrow::Cow;

use gpui::{
    App, AssetSource, Hsla, IntoElement, Pixels, RenderOnce, SharedString, Styled as _, Window, px,
    svg,
};

macro_rules! icons {
    ($($variant:ident => $file:literal,)*) => {
        /// Phosphor's regular icons, as the original uses.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum IconName { $($variant,)* }

        impl IconName {
            pub fn path(self) -> &'static str {
                match self { $(Self::$variant => concat!("icons/", $file),)* }
            }
        }

        fn icon_bytes(path: &str) -> Option<&'static [u8]> {
            match path {
                $(concat!("icons/", $file) => Some(include_bytes!(concat!("../assets/icons/", $file))),)*
                _ => None,
            }
        }

        const ICON_PATHS: &[&str] = &[$(concat!("icons/", $file),)*];
    };
}

icons! {
    ArrowLineDown => "arrow-line-down.svg",
    ArrowLineLeft => "arrow-line-left.svg",
    ArrowLineRight => "arrow-line-right.svg",
    ArrowLineUp => "arrow-line-up.svg",
    ArrowsDownUp => "arrows-down-up.svg",
    ArrowsIn => "arrows-in.svg",
    ArrowsLeftRight => "arrows-left-right.svg",
    ArrowsOut => "arrows-out.svg",
    CaretDown => "caret-down.svg",
    CaretLeft => "caret-left.svg",
    CaretRight => "caret-right.svg",
    CaretUpDown => "caret-up-down.svg",
    Check => "check.svg",
    Code => "code.svg",
    DownloadSimple => "download-simple.svg",
    MonitorPlay => "monitor-play.svg",
    Pause => "pause.svg",
    Play => "play.svg",
    SkipBack => "skip-back.svg",
    SquareSplitHorizontal => "square-split-horizontal.svg",
    SquareSplitVertical => "square-split-vertical.svg",
    TreeStructure => "tree-structure.svg",
    X => "x.svg",
}

/// The theme's embedded assets; pass to `Application::with_assets`.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(icon_bytes(path).map(Cow::Borrowed))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(ICON_PATHS
            .iter()
            .filter(|p| p.starts_with(path))
            .map(|p| SharedString::from(*p))
            .collect())
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
        // SVGs are drawn as masks: like `currentColor`, the text colour by default.
        let color = self.color.unwrap_or_else(|| window.text_style().color);
        svg()
            .path(self.name.path())
            .size(self.size)
            .flex_none()
            .text_color(color)
    }
}
