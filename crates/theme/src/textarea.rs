use std::ops::Range;

use gpui::{
    App, AvailableSpace, Bounds, ClipboardItem, Context, CursorStyle, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    GlobalElementId, InspectorElementId, InteractiveElement as _, IntoElement, KeyBinding,
    LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad,
    ParentElement as _, Pixels, Point, Render, SharedString, Style, Styled as _, TextAlign,
    TextRun, TextStyle, UTF16Selection, UnderlineStyle, Window, WrappedLine, actions, div, fill,
    point, prelude::FluentBuilder as _, px, relative, size,
};
use guard::SafeListener as _;
use unicode_segmentation::UnicodeSegmentation as _;

use crate::input::{
    Backspace, Cancel, Copy, Cut, Delete, End, Home, Left, Paste, Right, SelectAll, SelectLeft,
    SelectRight,
};
use crate::{ActiveTheme as _, InputEvent};

actions!(
    theme_textarea,
    [Newline, Up, Down, SelectUp, SelectDown, Submit]
);

const CONTEXT: &str = "ThemeTextarea";

pub(crate) fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some(CONTEXT)),
        KeyBinding::new("delete", Delete, Some(CONTEXT)),
        KeyBinding::new("left", Left, Some(CONTEXT)),
        KeyBinding::new("right", Right, Some(CONTEXT)),
        KeyBinding::new("up", Up, Some(CONTEXT)),
        KeyBinding::new("down", Down, Some(CONTEXT)),
        KeyBinding::new("shift-left", SelectLeft, Some(CONTEXT)),
        KeyBinding::new("shift-right", SelectRight, Some(CONTEXT)),
        KeyBinding::new("shift-up", SelectUp, Some(CONTEXT)),
        KeyBinding::new("shift-down", SelectDown, Some(CONTEXT)),
        KeyBinding::new("cmd-a", SelectAll, Some(CONTEXT)),
        KeyBinding::new("cmd-v", Paste, Some(CONTEXT)),
        KeyBinding::new("cmd-c", Copy, Some(CONTEXT)),
        KeyBinding::new("cmd-x", Cut, Some(CONTEXT)),
        KeyBinding::new("home", Home, Some(CONTEXT)),
        KeyBinding::new("end", End, Some(CONTEXT)),
        KeyBinding::new("enter", Newline, Some(CONTEXT)),
        KeyBinding::new("cmd-enter", Submit, Some(CONTEXT)),
        KeyBinding::new("escape", Cancel, Some(CONTEXT)),
    ]);
}

/// Rows of the smallest textarea.
const MIN_ROWS: usize = 3;
const PADDING: f32 = 8.0;

/// A multi-line text field, growing with its lines: Enter starts a new one,
/// Cmd-Enter submits.
pub struct Textarea {
    focus_handle: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    /// Each line as last drawn, wrapped to the width, with where its text starts in `content`.
    lines: Vec<(usize, WrappedLine)>,
    last_bounds: Option<Bounds<Pixels>>,
    line_height: Pixels,
    is_selecting: bool,
    /// The x the cursor keeps moving up and down along.
    column_x: Option<Pixels>,
    was_focused: bool,
}

impl EventEmitter<InputEvent> for Textarea {}

impl Textarea {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: SharedString::default(),
            placeholder: SharedString::default(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            lines: Vec::new(),
            last_bounds: None,
            line_height: px(20.0),
            is_selecting: false,
            column_x: None,
            was_focused: false,
        }
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub fn value(&self) -> &SharedString {
        &self.content
    }

    /// Shows `value` without emitting a change.
    pub fn set_value(&mut self, value: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = value.into();
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.focus_handle, cx);
    }

    #[cfg(test)]
    fn rows(&self) -> usize {
        self.content.split('\n').count().max(MIN_ROWS)
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify();
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }

    /// The start of the line holding `offset`, and where it ends.
    fn line_span(&self, offset: usize) -> Range<usize> {
        let start = self.content[..offset].rfind('\n').map_or(0, |i| i + 1);
        let end = self.content[offset..]
            .find('\n')
            .map_or(self.content.len(), |i| offset + i);
        start..end
    }

    /// Where `offset` is drawn, from the top left of the text.
    fn point_of(&self, offset: usize) -> Point<Pixels> {
        locate(&self.lines, offset, self.line_height)
    }

    /// The offset nearest to `at`, a position from the top left of the text.
    fn offset_at(&self, at: Point<Pixels>) -> usize {
        if at.y < px(0.0) {
            return 0;
        }
        let mut top = px(0.0);
        for (start, line) in &self.lines {
            let height = self.line_height * visual_rows(line) as f32;
            if at.y < top + height {
                let index = line
                    .closest_index_for_position(point(at.x, at.y - top), self.line_height)
                    .unwrap_or_else(|index| index);
                return start + index.min(line.len());
            }
            top += height;
        }
        self.content.len()
    }

    fn move_vertically(&mut self, down: bool, select: bool, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        let from = self.point_of(cursor);
        let x = self.column_x.unwrap_or(from.x);
        self.column_x = Some(x);
        let step = if down {
            self.line_height
        } else {
            -self.line_height
        };
        let target = self.offset_at(point(x, from.y + step + self.line_height / 2.0));
        if select {
            self.select_to(target, cx);
        } else {
            self.move_to(target, cx);
        }
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        self.column_x = None;
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx);
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        self.column_x = None;
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx);
        }
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertically(false, false, cx);
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertically(true, false, cx);
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertically(false, true, cx);
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_vertically(true, true, cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.column_x = None;
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.column_x = None;
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        cx.notify();
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.column_x = None;
        self.move_to(self.line_span(self.cursor_offset()).start, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.column_x = None;
        self.move_to(self.line_span(self.cursor_offset()).end, cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx);
        }
        self.replace_text_in_range(None, "", window, cx);
    }

    fn newline(&mut self, _: &Newline, window: &mut Window, cx: &mut Context<Self>) {
        self.replace_text_in_range(None, "\n", window, cx);
    }

    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Submit);
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Cancel);
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text.replace("\r\n", "\n"), window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected_range.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx);
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        let Some(bounds) = self.last_bounds else {
            return 0;
        };
        self.offset_at(position - bounds.origin)
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.is_selecting = true;
        self.column_x = None;
        let index = self.index_for_mouse_position(event.position);
        if event.modifiers.shift {
            self.select_to(index, cx);
        } else {
            self.move_to(index, cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8 = 0;
        let mut utf16 = 0;
        for ch in self.content.chars() {
            if utf16 >= offset {
                break;
            }
            utf16 += ch.len_utf16();
            utf8 += ch.len_utf8();
        }
        utf8
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16 = 0;
        let mut utf8 = 0;
        for ch in self.content.chars() {
            if utf8 >= offset {
                break;
            }
            utf8 += ch.len_utf8();
            utf16 += ch.len_utf16();
        }
        utf16
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }
}

impl EntityInputHandler for Textarea {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        let end = range.start + new_text.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        self.column_x = None;
        cx.emit(InputEvent::Change);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.content =
            (self.content[0..range.start].to_owned() + new_text + &self.content[range.end..])
                .into();
        self.marked_range =
            (!new_text.is_empty()).then(|| range.start..range.start + new_text.len());
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .map(|new| new.start + range.start..new.end + range.start)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        cx.emit(InputEvent::Change);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let (start, end) = (self.point_of(range.start), self.point_of(range.end));
        Some(Bounds::from_corners(
            bounds.origin + start,
            bounds.origin + point(end.x, end.y + self.line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.offset_to_utf16(self.index_for_mouse_position(position)))
    }
}

struct TextareaElement {
    input: Entity<Textarea>,
}

struct Prepaint {
    lines: Vec<(usize, WrappedLine)>,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
}

/// Where `offset` is drawn in `lines`, from the top left of the text.
fn locate(lines: &[(usize, WrappedLine)], offset: usize, line_height: Pixels) -> Point<Pixels> {
    let mut rows = 0;
    for (start, line) in lines {
        if offset <= start + line.len() {
            let at = line
                .position_for_index(offset - start, line_height)
                .unwrap_or_default();
            return point(at.x, at.y + line_height * rows as f32);
        }
        rows += visual_rows(line);
    }
    point(px(0.0), line_height * rows.saturating_sub(1) as f32)
}

/// How many rows `line` takes once wrapped.
fn visual_rows(line: &WrappedLine) -> usize {
    line.wrap_boundaries().len() + 1
}

/// The offsets at which each row of `line` starts, then its length.
fn row_starts(line: &WrappedLine) -> Vec<usize> {
    let boundaries = line.wrap_boundaries().iter().filter_map(|b| {
        line.unwrapped_layout
            .runs
            .get(b.run_ix)?
            .glyphs
            .get(b.glyph_ix)
            .map(|glyph| glyph.index)
    });
    std::iter::once(0)
        .chain(boundaries)
        .chain([line.len()])
        .collect()
}

/// Shapes each line of `source`, wrapped at `wrap_width`, with the lines' starts in `source`.
fn shape_lines(
    source: &str,
    marked: Option<&Range<usize>>,
    style: &TextStyle,
    color: gpui::Hsla,
    font_size: Pixels,
    wrap_width: Option<Pixels>,
    window: &mut Window,
) -> Vec<(usize, WrappedLine)> {
    let mut lines = Vec::new();
    let mut start = 0;
    for text in source.split('\n') {
        let run = TextRun {
            len: text.len(),
            font: style.font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let runs = match marked {
            Some(m) if m.start < start + text.len() && m.end > start => {
                let from = m.start.max(start) - start;
                let to = m.end.min(start + text.len()) - start;
                [
                    TextRun {
                        len: from,
                        ..run.clone()
                    },
                    TextRun {
                        len: to - from,
                        underline: Some(UnderlineStyle {
                            color: Some(run.color),
                            thickness: px(1.0),
                            wavy: false,
                        }),
                        ..run.clone()
                    },
                    TextRun {
                        len: text.len() - to,
                        ..run
                    },
                ]
                .into_iter()
                .filter(|r| r.len > 0)
                .collect()
            }
            _ => vec![run],
        };
        let shaped = window
            .text_system()
            .shape_text(
                SharedString::from(text.to_owned()),
                font_size,
                &runs,
                wrap_width,
                None,
            )
            .ok()
            .and_then(|lines| lines.into_iter().next())
            .unwrap_or_default();
        lines.push((start, shaped));
        start += text.len() + 1;
    }
    lines
}

impl gpui::Element for TextareaElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let input = self.input.read(cx);
        let source = if input.content.is_empty() {
            input.placeholder.clone()
        } else {
            input.content.clone()
        };
        let style = window.text_style();
        let line_height = window.line_height();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let mut layout = Style::default();
        layout.size.width = relative(1.0).into();
        // The height follows the width the text wraps at, which only layout knows.
        let layout = window.request_measured_layout(layout, move |known, available, window, _| {
            let width = known.width.or(match available.width {
                AvailableSpace::Definite(width) => Some(width),
                _ => None,
            });
            let lines = guard::guard("textarea measure", || {
                shape_lines(
                    &source,
                    None,
                    &style,
                    gpui::black(),
                    font_size,
                    width,
                    window,
                )
            })
            .unwrap_or_default();
            let rows: usize = lines.iter().map(|(_, line)| visual_rows(line)).sum();
            size(
                width.unwrap_or_default(),
                line_height * rows.max(MIN_ROWS) as f32,
            )
        });
        (layout, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let theme = cx.theme();
        let input = self.input.read(cx);
        let content = input.content.clone();
        let selected_range = input.selected_range.clone();
        let cursor = input.cursor_offset();
        let marked = input.marked_range.clone();
        let style = window.text_style();
        let line_height = window.line_height();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let empty = content.is_empty();
        let (text_color, source) = if empty {
            (theme.colors.muted_foreground, input.placeholder.clone())
        } else {
            (style.color, content)
        };

        let lines = shape_lines(
            &source,
            marked.as_ref().filter(|_| !empty),
            &style,
            text_color,
            font_size,
            Some(bounds.size.width),
            window,
        );

        let drawn_at = |offset: usize| locate(&lines, offset, line_height);

        let mut selection = Vec::new();
        let mut cursor_quad = None;
        if empty || selected_range.is_empty() {
            let at = if empty {
                point(px(0.0), px(0.0))
            } else {
                drawn_at(cursor)
            };
            cursor_quad = Some(fill(
                Bounds::new(bounds.origin + at, size(px(1.0), line_height)),
                theme.colors.foreground,
            ));
        } else {
            for (start, line) in &lines {
                let end = start + line.len();
                // A line break inside the selection is shown as a little extra width.
                let breaks = selected_range.start <= end && selected_range.end > end;
                let starts = row_starts(line);
                for (row, span) in starts.windows(2).enumerate() {
                    let last = row + 2 == starts.len();
                    let (from, to) = (
                        selected_range.start.max(start + span[0]),
                        selected_range.end.min(start + span[1]),
                    );
                    let extra = if last && breaks { px(4.0) } else { px(0.0) };
                    if from > to || (from == to && extra == px(0.0)) {
                        continue;
                    }
                    let (left, right) = (drawn_at(from), drawn_at(to));
                    let top = drawn_at(*start).y + line_height * row as f32;
                    selection.push(fill(
                        Bounds::from_corners(
                            bounds.origin + point(left.x, top),
                            bounds.origin + point(right.x + extra, top + line_height),
                        ),
                        theme.colors.ring.opacity(0.4),
                    ));
                }
            }
        }
        Prepaint {
            lines,
            cursor: cursor_quad,
            selection,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        let line_height = window.line_height();
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for quad in prepaint.selection.drain(..) {
                window.paint_quad(quad);
            }
            let mut rows = 0;
            for (_, line) in &prepaint.lines {
                let origin = point(bounds.left(), bounds.top() + line_height * rows as f32);
                line.paint(origin, line_height, TextAlign::Left, None, window, cx)
                    .ok();
                rows += visual_rows(line);
            }
            if focus_handle.is_focused(window)
                && let Some(cursor) = prepaint.cursor.take()
            {
                window.paint_quad(cursor);
            }
        });
        let lines = std::mem::take(&mut prepaint.lines);
        self.input.update(cx, |input, _| {
            input.lines = lines;
            input.last_bounds = Some(bounds);
            input.line_height = line_height;
        });
    }
}

impl Focusable for Textarea {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Textarea {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        guard::render("Textarea", || {
            let focused = self.focus_handle.is_focused(window);
            if self.was_focused && !focused {
                cx.emit(InputEvent::Blur);
            }
            self.was_focused = focused;
            let theme = cx.theme();
            let c = &theme.colors;
            div()
                .w_full()
                .min_w_0()
                .min_h(px(64.0))
                .px(px(10.0))
                .py(px(PADDING))
                .rounded(theme.radius.lg)
                .border_1()
                .border_color(if focused { c.ring } else { c.input })
                .when(theme.is_dark(), |d| d.bg(c.input.opacity(0.3)))
                .text_color(c.foreground)
                .font_family(theme.font.clone())
                .text_size(theme.text.sm)
                .key_context(CONTEXT)
                .track_focus(&self.focus_handle)
                .cursor(CursorStyle::IBeam)
                .on_action(cx.safe_listener(Self::backspace))
                .on_action(cx.safe_listener(Self::delete))
                .on_action(cx.safe_listener(Self::left))
                .on_action(cx.safe_listener(Self::right))
                .on_action(cx.safe_listener(Self::up))
                .on_action(cx.safe_listener(Self::down))
                .on_action(cx.safe_listener(Self::select_left))
                .on_action(cx.safe_listener(Self::select_right))
                .on_action(cx.safe_listener(Self::select_up))
                .on_action(cx.safe_listener(Self::select_down))
                .on_action(cx.safe_listener(Self::select_all))
                .on_action(cx.safe_listener(Self::home))
                .on_action(cx.safe_listener(Self::end))
                .on_action(cx.safe_listener(Self::paste))
                .on_action(cx.safe_listener(Self::cut))
                .on_action(cx.safe_listener(Self::copy))
                .on_action(cx.safe_listener(Self::newline))
                .on_action(cx.safe_listener(Self::submit))
                .on_action(cx.safe_listener(Self::cancel))
                .on_mouse_down(MouseButton::Left, cx.safe_listener(Self::on_mouse_down))
                .on_mouse_up(MouseButton::Left, cx.safe_listener(Self::on_mouse_up))
                .on_mouse_up_out(MouseButton::Left, cx.safe_listener(Self::on_mouse_up))
                .on_mouse_move(cx.safe_listener(Self::on_mouse_move))
                .child(TextareaElement { input: cx.entity() })
        })
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{AppContext as _, TestAppContext, VisualTestContext};

    use super::*;

    fn setup(
        cx: &mut TestAppContext,
    ) -> (
        Entity<Textarea>,
        Rc<RefCell<Vec<InputEvent>>>,
        &mut VisualTestContext,
    ) {
        cx.update(crate::init);
        let (area, cx) = cx.add_window_view(|_, cx| Textarea::new(cx));
        let events = Rc::new(RefCell::new(Vec::new()));
        let log = events.clone();
        cx.update(|window, cx| {
            cx.subscribe(&area, move |_, event: &InputEvent, _| {
                log.borrow_mut().push(event.clone())
            })
            .detach();
            area.update(cx, |a, cx| a.focus(window, cx));
        });
        cx.run_until_parked();
        (area, events, cx)
    }

    fn value(area: &Entity<Textarea>, cx: &mut VisualTestContext) -> String {
        cx.read(|cx| area.read(cx).value().to_string())
    }

    #[gpui::test]
    fn enter_starts_a_new_line(cx: &mut TestAppContext) {
        let (area, events, cx) = setup(cx);
        cx.simulate_input("one");
        cx.simulate_keystrokes("enter");
        cx.simulate_input("two");
        assert_eq!(value(&area, cx), "one\ntwo");
        assert!(events.borrow().iter().all(|e| *e == InputEvent::Change));
    }

    #[gpui::test]
    fn cmd_enter_submits_and_escape_cancels(cx: &mut TestAppContext) {
        let (_, events, cx) = setup(cx);
        cx.simulate_input("a");
        cx.simulate_keystrokes("cmd-enter escape");
        assert_eq!(
            *events.borrow(),
            [InputEvent::Change, InputEvent::Submit, InputEvent::Cancel]
        );
    }

    fn on_fourth_column_of_the_first_line(area: &Entity<Textarea>, cx: &mut VisualTestContext) {
        cx.update(|_, cx| area.update(cx, |a, cx| a.set_value("abcdef\nab\nabcdef", cx)));
        cx.run_until_parked();
        cx.simulate_keystrokes("up up home right right right right");
    }

    #[gpui::test]
    fn a_shorter_line_stops_the_cursor_at_its_end(cx: &mut TestAppContext) {
        let (area, _, cx) = setup(cx);
        on_fourth_column_of_the_first_line(&area, cx);
        cx.simulate_keystrokes("down");
        cx.simulate_input("X");
        assert_eq!(value(&area, cx), "abcdef\nabX\nabcdef");
    }

    #[gpui::test]
    fn up_and_down_keep_the_column_through_a_shorter_line(cx: &mut TestAppContext) {
        let (area, _, cx) = setup(cx);
        on_fourth_column_of_the_first_line(&area, cx);
        cx.simulate_keystrokes("down down");
        cx.simulate_input("Y");
        assert_eq!(value(&area, cx), "abcdef\nab\nabcdYef");
    }

    #[gpui::test]
    fn backspace_joins_lines(cx: &mut TestAppContext) {
        let (area, _, cx) = setup(cx);
        cx.update(|_, cx| area.update(cx, |a, cx| a.set_value("a\nb", cx)));
        cx.run_until_parked();
        cx.simulate_keystrokes("left backspace");
        assert_eq!(value(&area, cx), "ab");
    }

    #[gpui::test]
    fn grows_with_its_lines_from_three_rows(cx: &mut TestAppContext) {
        let (area, _, cx) = setup(cx);
        assert_eq!(cx.read(|cx| area.read(cx).rows()), 3);
        cx.update(|_, cx| area.update(cx, |a, cx| a.set_value("1\n2\n3\n4\n5", cx)));
        assert_eq!(cx.read(|cx| area.read(cx).rows()), 5);
    }

    #[gpui::test]
    fn a_click_places_the_cursor_on_that_line(cx: &mut TestAppContext) {
        let (area, _, cx) = setup(cx);
        cx.update(|_, cx| area.update(cx, |a, cx| a.set_value("abc\ndef", cx)));
        cx.run_until_parked();
        let (bounds, height) = cx.read(|cx| {
            (
                area.read(cx).last_bounds.expect("drawn"),
                area.read(cx).line_height,
            )
        });
        let on_second_line = point(bounds.left() + px(1.0), bounds.top() + height * 1.5);
        cx.simulate_click(on_second_line, gpui::Modifiers::default());
        cx.simulate_input("X");
        assert_eq!(value(&area, cx), "abc\nXdef");
    }

    struct Narrow(Entity<Textarea>);

    impl Render for Narrow {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            guard::render("Narrow", || div().w(px(90.0)).child(self.0.clone()))
        }
    }

    #[gpui::test]
    fn long_lines_wrap_and_the_arrows_move_between_their_rows(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (narrow, cx) = cx.add_window_view(|_, cx| Narrow(cx.new(Textarea::new)));
        let area = cx.read(|cx| narrow.read(cx).0.clone());
        cx.update(|window, cx| {
            area.update(cx, |a, cx| {
                a.set_value("alpha beta gamma delta epsilon zeta eta theta", cx);
                a.focus(window, cx);
            })
        });
        cx.run_until_parked();
        let rows = cx.read(|cx| visual_rows(&area.read(cx).lines[0].1));
        assert!(rows > 3, "{rows} rows");
        let height = cx.read(|cx| area.read(cx).last_bounds.expect("drawn").size.height);
        let line_height = cx.read(|cx| area.read(cx).line_height);
        assert_eq!(height, line_height * rows as f32);

        let starts = cx.read(|cx| row_starts(&area.read(cx).lines[0].1));
        cx.simulate_keystrokes("up");
        cx.simulate_input("X");
        let text = value(&area, cx);
        let at = text.find('X').expect("inserted");
        assert!(
            (starts[rows - 2]..=starts[rows - 1]).contains(&at),
            "{at} outside {starts:?}"
        );
    }
}
