use std::ops::Range;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, ElementId, ElementInputHandler, Entity,
    EntityInputHandler, EventEmitter, FocusHandle, Focusable, GlobalElementId, InspectorElementId,
    InteractiveElement as _, IntoElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, ParentElement as _, Pixels, Point, Render, ShapedLine,
    SharedString, Style, Styled as _, TextRun, UTF16Selection, UnderlineStyle, Window, actions,
    div, fill, point, prelude::FluentBuilder as _, px, relative, size,
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
    /// Each line as last drawn, with where its text starts in `content`.
    lines: Vec<(usize, ShapedLine)>,
    last_bounds: Option<Bounds<Pixels>>,
    line_height: Pixels,
    is_selecting: bool,
    /// The x the cursor keeps moving up and down along.
    column_x: Option<Pixels>,
    scroll: Pixels,
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
            scroll: px(0.0),
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

    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus_handle);
    }

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

    fn x_of(&self, offset: usize) -> Pixels {
        let span = self.line_span(offset);
        self.lines
            .iter()
            .find(|(start, _)| *start == span.start)
            .map_or(px(0.0), |(start, line)| line.x_for_index(offset - start))
    }

    /// The offset a line up or down from `offset` on column `x`.
    fn vertical_offset(&self, offset: usize, down: bool, x: Pixels) -> usize {
        let span = self.line_span(offset);
        let target = if down {
            if span.end >= self.content.len() {
                return self.content.len();
            }
            span.end + 1
        } else {
            if span.start == 0 {
                return 0;
            }
            span.start - 1
        };
        let target = self.line_span(target);
        match self.lines.iter().find(|(start, _)| *start == target.start) {
            Some((_, line)) => target.start + line.closest_index_for_x(x).min(target.len()),
            None => target.start,
        }
    }

    fn move_vertically(&mut self, down: bool, select: bool, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        let x = self.column_x.unwrap_or_else(|| self.x_of(cursor));
        self.column_x = Some(x);
        let target = self.vertical_offset(cursor, down, x);
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
        let row = ((position.y - bounds.top()) / self.line_height)
            .floor()
            .max(0.0) as usize;
        let Some((start, line)) = self.lines.get(row.min(self.lines.len().saturating_sub(1)))
        else {
            return 0;
        };
        let end = self.line_span(*start).end;
        start
            + line
                .closest_index_for_x(position.x - bounds.left() + self.scroll)
                .min(end - start)
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
        let row = self.content[..range.start].matches('\n').count();
        let top = bounds.top() + self.line_height * row as f32;
        Some(Bounds::from_corners(
            point(bounds.left() + self.x_of(range.start) - self.scroll, top),
            point(
                bounds.left() + self.x_of(range.end) - self.scroll,
                top + self.line_height,
            ),
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
    lines: Vec<(usize, ShapedLine)>,
    cursor: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
    scroll: Pixels,
}

impl IntoElement for TextareaElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
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
        let rows = self.input.read(cx).rows();
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height = (window.line_height() * rows as f32).into();
        (window.request_layout(style, [], cx), ())
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

        let mut lines = Vec::new();
        let mut start = 0;
        for text in source.split('\n') {
            let run = TextRun {
                len: text.len(),
                font: style.font(),
                color: text_color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let runs = match marked.as_ref().filter(|_| !empty) {
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
            let shaped = window.text_system().shape_line(
                SharedString::from(text.to_owned()),
                font_size,
                &runs,
                None,
            );
            lines.push((start, shaped));
            start += text.len() + 1;
        }

        let line_at = |offset: usize| lines.iter().rposition(|(s, _)| *s <= offset).unwrap_or(0);
        let x_at = |offset: usize| {
            let row = line_at(offset);
            lines
                .get(row)
                .map_or(px(0.0), |(s, l)| l.x_for_index(offset - s))
        };
        let cursor_x = if empty { px(0.0) } else { x_at(cursor) };
        let mut scroll = input.scroll;
        if cursor_x - scroll > bounds.size.width - px(2.0) {
            scroll = cursor_x - bounds.size.width + px(2.0);
        } else if cursor_x < scroll {
            scroll = cursor_x;
        }
        scroll = scroll.max(px(0.0));
        let origin = bounds.left() - scroll;
        let row_top = |row: usize| bounds.top() + line_height * row as f32;

        let mut selection = Vec::new();
        let mut cursor_quad = None;
        if !empty {
            if selected_range.is_empty() {
                let row = line_at(cursor);
                cursor_quad = Some(fill(
                    Bounds::new(
                        point(origin + cursor_x, row_top(row)),
                        size(px(1.0), line_height),
                    ),
                    theme.colors.foreground,
                ));
            } else {
                for (row, (start, line)) in lines.iter().enumerate() {
                    let end = start + line_text_len(&source, *start);
                    let (from, to) = (
                        selected_range.start.max(*start),
                        selected_range.end.min(end),
                    );
                    // A line break inside the selection is shown as a little extra width.
                    let breaks = selected_range.start <= end && selected_range.end > end;
                    if from > to || (from == to && !breaks) {
                        continue;
                    }
                    let extra = if breaks { px(4.0) } else { px(0.0) };
                    selection.push(fill(
                        Bounds::from_corners(
                            point(origin + line.x_for_index(from - start), row_top(row)),
                            point(
                                origin + line.x_for_index(to - start) + extra,
                                row_top(row) + line_height,
                            ),
                        ),
                        theme.colors.ring.opacity(0.4),
                    ));
                }
            }
        } else {
            cursor_quad = Some(fill(
                Bounds::new(point(origin, bounds.top()), size(px(1.0), line_height)),
                theme.colors.foreground,
            ));
        }
        Prepaint {
            lines,
            cursor: cursor_quad,
            selection,
            scroll,
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
            for (row, (_, line)) in prepaint.lines.iter().enumerate() {
                let origin = point(
                    bounds.left() - prepaint.scroll,
                    bounds.top() + line_height * row as f32,
                );
                line.paint(origin, line_height, window, cx).ok();
            }
            if focus_handle.is_focused(window)
                && let Some(cursor) = prepaint.cursor.take()
            {
                window.paint_quad(cursor);
            }
        });
        let lines = std::mem::take(&mut prepaint.lines);
        let scroll = prepaint.scroll;
        self.input.update(cx, |input, _| {
            input.lines = lines;
            input.last_bounds = Some(bounds);
            input.line_height = line_height;
            input.scroll = scroll;
        });
    }
}

/// The length of the line of `text` starting at `start`.
fn line_text_len(text: &str, start: usize) -> usize {
    text[start..].find('\n').unwrap_or(text.len() - start)
}

impl Focusable for Textarea {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Textarea {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{TestAppContext, VisualTestContext};

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
            area.read(cx).focus(window);
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
}
