//! 终端画布：把 [`superremote_term::Frame`] 画成像素。
//!
//! 两遍绘制：先把同色背景合并成矩形（一屏通常只有几十个矩形），再把同样式的
//! 连续字符合并成一段文本。逐格绘制在 200×50 的窗口上会产生上万个图元，
//! 软件渲染下会明显掉帧，所以合并是必须的。

use iced::widget::canvas::{self, Action, Frame as CanvasFrame, Geometry, Text};
use iced::{alignment, mouse, Color, Font, Pixels, Point, Rectangle, Size, Theme};
use superremote_core::session::{TerminalCursor, TerminalMouseMode};
use superremote_core::tab::TabId;
use superremote_term::{Cell, CellAttrs, Frame, Rgb};

use crate::app::Message;
use crate::i18n;
use crate::terminal::{CellPos, Metrics, Selection};

/// 画布状态：记上一次的像素尺寸、字体度量与拖选锚点。
#[derive(Debug, Default)]
pub struct CanvasState {
    last: Size,
    metrics: Option<Metrics>,
    drag: Option<CellPos>,
}

pub struct TerminalCanvas<'a> {
    pub frame: &'a Frame,
    pub instance: TabId,
    /// 光标闪烁相位：为假时只画字符、不画方块。
    pub blink_on: bool,
    /// 只有已连接的会话才显示可输入的光标提示。
    pub interactive: bool,
    pub metrics: Metrics,
    pub font_family: &'static str,
    pub font_size: f32,
    pub cursor: TerminalCursor,
    pub selection: Option<Selection>,
    /// 鼠标动作：右键/中键谁是「粘贴」、谁是「扩展选区」。
    pub mouse_mode: TerminalMouseMode,
}

impl canvas::Program<Message> for TerminalCanvas<'_> {
    type State = CanvasState;

    fn update(
        &self,
        state: &mut Self::State,
        event: &iced::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<Action<Message>> {
        if matches!(event, iced::Event::Window(_) | iced::Event::Mouse(_)) {
            // 布局尺寸或字体度量变了才回报，否则每次输入事件都会发一遍消息。
            if (state.last.width - bounds.width).abs() > 0.5
                || (state.last.height - bounds.height).abs() > 0.5
                || state.metrics != Some(self.metrics)
            {
                state.last = bounds.size();
                state.metrics = Some(self.metrics);
                let (cols, rows) = self.metrics.grid_size(bounds.width, bounds.height);
                return Some(Action::publish(Message::TerminalResized(
                    self.instance,
                    cols,
                    rows,
                )));
            }
        }

        if let iced::Event::Mouse(event) = event {
            match event {
                mouse::Event::ButtonPressed(mouse::Button::Left) => {
                    if let Some(position) = cursor.position_in(bounds) {
                        let cell = cell_at(self, bounds, position);
                        state.drag = Some(cell);
                        return Some(Action::publish(Message::TerminalSelection(Some(
                            Selection {
                                anchor: cell,
                                cursor: cell,
                            },
                        ))));
                    }
                }
                mouse::Event::CursorMoved { .. } => {
                    if let (Some(anchor), Some(position)) = (state.drag, cursor.position_in(bounds))
                    {
                        let cell = cell_at(self, bounds, position);
                        return Some(Action::publish(Message::TerminalSelection(Some(
                            Selection {
                                anchor,
                                cursor: cell,
                            },
                        ))));
                    }
                }
                mouse::Event::ButtonReleased(mouse::Button::Left) => {
                    state.drag = None;
                }
                // 右键/中键的动作由「鼠标动作」设置决定：混合模式与 Windows 是
                // 右键粘贴，xterm 反过来（右键扩展选区、中键粘贴）。
                mouse::Event::ButtonPressed(mouse::Button::Right)
                    if cursor.position_in(bounds).is_some() =>
                {
                    return Some(if self.mouse_mode.right_click_pastes() {
                        Action::publish(Message::PasteClipboard)
                    } else {
                        extend_selection(self, bounds, cursor)
                    });
                }
                mouse::Event::ButtonPressed(mouse::Button::Middle)
                    if cursor.position_in(bounds).is_some() =>
                {
                    return Some(if self.mouse_mode.middle_click_pastes() {
                        Action::publish(Message::PasteClipboard)
                    } else {
                        extend_selection(self, bounds, cursor)
                    });
                }
                mouse::Event::WheelScrolled { delta } if cursor.position_in(bounds).is_some() => {
                    let lines = match delta {
                        mouse::ScrollDelta::Lines { y, .. } => *y,
                        mouse::ScrollDelta::Pixels { y, .. } => *y / self.metrics.cell_height,
                    };
                    let lines = (lines * 3.0) as i32;
                    if lines != 0 {
                        return Some(Action::publish(Message::TerminalScrolled(
                            self.instance,
                            lines,
                        )));
                    }
                }
                _ => {}
            }
        }

        None
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let cell = Size::new(self.metrics.cell_width, self.metrics.cell_height);
        let font = Font::with_name(self.font_family);
        let backdrop = to_color(self.frame.background);
        let mut canvas = CanvasFrame::new(renderer, bounds.size());
        canvas.fill_rectangle(Point::ORIGIN, bounds.size(), backdrop);

        for (row_index, row) in self.frame.rows.iter().enumerate() {
            let y = row_index as f32 * cell.height;
            if y > bounds.height {
                break;
            }
            draw_backgrounds(&mut canvas, row, y, cell, backdrop);
            if let Some(selection) = self.selection {
                draw_selection(&mut canvas, row, row_index, y, cell, selection);
            }
            draw_glyphs(&mut canvas, row, y, cell, font, self.font_size);
        }

        if self.blink_on {
            if let Some(cursor) = self.frame.cursor {
                draw_cursor(
                    &mut canvas,
                    self.frame,
                    cursor,
                    self.cursor,
                    cell,
                    self.font_size,
                    font,
                );
            }
        }

        // 翻历史时给一个位置提示，避免用户以为终端卡死。
        if self.frame.offset > 0 {
            // 浮层画在终端上，配色跟着终端的深底走（同上：不跟界面主题）。
            let dark = crate::theme::Palette::dark();
            let label = i18n::lang().scroll_lines(self.frame.offset);
            let width = cell.width * (label.chars().count() as f32 + 1.0);

            canvas.fill_rectangle(
                Point::new(bounds.width - width - 10.0, 8.0),
                Size::new(width, cell.height + 4.0),
                Color {
                    a: 0.88,
                    ..dark.canvas
                },
            );
            canvas.fill_text(Text {
                content: label,
                position: Point::new(bounds.width - width - 4.0, 10.0),
                color: dark.text_dim,
                size: Pixels(11.0),
                ..Text::default()
            });
        }

        vec![canvas.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        _bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if self.interactive {
            mouse::Interaction::Text
        } else {
            mouse::Interaction::default()
        }
    }
}

/// 「扩展选区」动作：把已有选区的终点拉到点击处；还没有选区时以点击处起选。
///
/// xterm / Windows 模式下右键或中键会走这条路径。没有选区时起选而不是无事可做，
/// 否则用户第一次点右键会以为程序没反应。
fn extend_selection(
    canvas: &TerminalCanvas<'_>,
    bounds: Rectangle,
    cursor: mouse::Cursor,
) -> Action<Message> {
    let Some(position) = cursor.position_in(bounds) else {
        return Action::capture();
    };
    let cell = cell_at(canvas, bounds, position);

    let selection = match canvas.selection {
        Some(selection) if !selection.is_empty() => Selection {
            anchor: selection.anchor,
            cursor: cell,
        },
        _ => Selection {
            anchor: cell,
            cursor: cell,
        },
    };

    // 这里不改 `state.drag`：拖选锚点归左键按住那一路，右键/中键是「点一下扩展一下」，
    // 混进去会让没有按键的鼠标移动也继续拉长选区。
    Action::publish(Message::TerminalSelection(Some(selection)))
}

fn cell_at(canvas: &TerminalCanvas<'_>, bounds: Rectangle, position: Point) -> CellPos {
    let col = ((position.x - bounds.x) / canvas.metrics.cell_width).floor();
    let row = ((position.y - bounds.y) / canvas.metrics.cell_height).floor();
    let max_row = canvas.frame.rows.len().saturating_sub(1);
    let max_col = canvas
        .frame
        .rows
        .get(row.max(0.0) as usize)
        .map(|row| row.len().saturating_sub(1))
        .unwrap_or(0);

    CellPos {
        row: (row.max(0.0) as usize).min(max_row),
        col: (col.max(0.0) as usize).min(max_col),
    }
}

/// 选中区用半透明强调色覆盖在背景上，文字仍然画在最上层。
fn draw_selection(
    canvas: &mut CanvasFrame<iced::Renderer>,
    row: &[Cell],
    row_index: usize,
    y: f32,
    cell: Size,
    selection: Selection,
) {
    let highlight = Color {
        a: 0.28,
        ..crate::theme::Palette::dark().accent
    };
    let mut start: Option<usize> = None;

    for col in 0..=row.len() {
        let selected = col < row.len() && selection.contains(row_index, col);
        match (start, selected) {
            (None, true) => start = Some(col),
            (Some(first), false) => {
                canvas.fill_rectangle(
                    Point::new(first as f32 * cell.width, y),
                    Size::new((col - first) as f32 * cell.width, cell.height),
                    highlight,
                );
                start = None;
            }
            _ => {}
        }
    }
}

/// 同色背景合并成一条矩形；与画布同色的段落直接跳过。
fn draw_backgrounds(
    canvas: &mut CanvasFrame<iced::Renderer>,
    row: &[Cell],
    y: f32,
    cell: Size,
    backdrop: Color,
) {
    let mut run_start = 0usize;
    let mut run_color = row
        .first()
        .map(|cell| cell.bg)
        .unwrap_or(superremote_term::backdrop());

    for col in 1..=row.len() {
        let same = row.get(col).is_some_and(|cell| cell.bg == run_color);
        if same && col < row.len() {
            continue;
        }

        let color = to_color(run_color);
        if color != backdrop {
            let x = run_start as f32 * cell.width;
            let width = (col - run_start) as f32 * cell.width;
            canvas.fill_rectangle(Point::new(x, y), Size::new(width, cell.height), color);
        }

        if let Some(next) = row.get(col) {
            run_start = col;
            run_color = next.bg;
        }
    }
}

/// 一行里可以合并成一段的文本片段。
#[derive(Debug, Clone, PartialEq)]
struct Span {
    /// 起始列号。
    start: usize,
    /// 占几个单元格（宽字符算两格），用来算下划线长度。
    cells: usize,
    style: Style,
    text: String,
}

/// 把一行切成语义完整的文本段：同样式、连续、跳过空白与占位格。
///
/// 分段规则单独抽出来，是因为它直接决定「每个词画在哪个 x 上」，
/// 一旦起点算错就是整屏叠字——这类问题必须能被单测抓住。
fn row_spans(row: &[Cell]) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();

    for (col, current) in row.iter().enumerate() {
        if is_skipped(current) {
            continue;
        }

        let style = Style::from(current);
        let wide = current.attrs.wide;

        match spans.last_mut() {
            // 只有「样式相同」且「列号紧接上一段末尾」才能续写，
            // 中间隔了空格就是新的一段，否则词会被画到上一段的起点上。
            Some(span) if span.style == style && span.start + span.cells == col => {
                span.text.push(current.c);
                span.cells += if wide { 2 } else { 1 };
            }
            _ => spans.push(Span {
                start: col,
                cells: if wide { 2 } else { 1 },
                style,
                text: current.c.to_string(),
            }),
        }
    }

    spans
}

fn is_skipped(cell: &Cell) -> bool {
    cell.attrs.spacer
        || cell.attrs.hidden
        || (cell.c == ' ' && !cell.attrs.underline && !cell.attrs.strike)
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Style {
    fg: Rgb,
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
}

impl From<&Cell> for Style {
    fn from(cell: &Cell) -> Self {
        Self {
            fg: cell.fg,
            bold: cell.attrs.bold,
            italic: cell.attrs.italic,
            underline: cell.attrs.underline,
            strike: cell.attrs.strike,
        }
    }
}

/// 同样式的连续字符合并成一段文本。
fn draw_glyphs(
    canvas: &mut CanvasFrame<iced::Renderer>,
    row: &[Cell],
    y: f32,
    cell: Size,
    font: Font,
    font_size: f32,
) {
    for span in row_spans(row) {
        draw_span(canvas, &span, y, cell, font, font_size);
    }
}

fn draw_span(
    canvas: &mut CanvasFrame<iced::Renderer>,
    span: &Span,
    y: f32,
    cell: Size,
    font: Font,
    font_size: f32,
) {
    let x = span.start as f32 * cell.width;
    let width = span.cells as f32 * cell.width;
    let color = to_color(span.style.fg);

    canvas.fill_text(Text {
        content: span.text.clone(),
        position: Point::new(x, y),
        color,
        size: Pixels(font_size),
        font: Font {
            weight: if span.style.bold {
                iced::font::Weight::Bold
            } else {
                iced::font::Weight::Normal
            },
            style: if span.style.italic {
                iced::font::Style::Italic
            } else {
                iced::font::Style::Normal
            },
            ..font
        },
        align_y: alignment::Vertical::Top,
        ..Text::default()
    });

    if span.style.underline {
        canvas.fill_rectangle(
            Point::new(x, y + cell.height - 2.0),
            Size::new(width, 1.0),
            color,
        );
    }
    if span.style.strike {
        canvas.fill_rectangle(
            Point::new(x, y + cell.height * 0.55),
            Size::new(width, 1.0),
            color,
        );
    }
}

fn draw_cursor(
    canvas: &mut CanvasFrame<iced::Renderer>,
    frame: &Frame,
    cursor: superremote_term::Cursor,
    shape: TerminalCursor,
    cell: Size,
    font_size: f32,
    font: Font,
) {
    let occupant = frame
        .rows
        .get(cursor.row)
        .and_then(|row| row.get(cursor.col));
    let width = if occupant.is_some_and(|cell| cell.attrs.wide) {
        cell.width * 2.0
    } else {
        cell.width
    };
    let corner = Point::new(
        cursor.col as f32 * cell.width,
        cursor.row as f32 * cell.height,
    );
    let area = Size::new(width, cell.height);
    let color = to_color(frame.cursor_color);

    match shape {
        TerminalCursor::Underline => canvas.fill_rectangle(
            Point::new(corner.x, corner.y + cell.height - 2.0),
            Size::new(width, 2.0),
            color,
        ),
        TerminalCursor::Beam => canvas.fill_rectangle(corner, Size::new(2.0, cell.height), color),
        TerminalCursor::Block => canvas.fill_rectangle(corner, area, color),
    }

    // 方块光标压住字符时，用背景色把该字重画一遍：文本层在图形层之上，
    // 所以这一笔会盖住原来的前景色，得到经典的反色效果。
    if shape == TerminalCursor::Block {
        if let Some(occupant) = occupant {
            if !occupant.attrs.spacer && occupant.c != ' ' {
                canvas.fill_text(Text {
                    content: occupant.c.to_string(),
                    position: Point::new(corner.x, corner.y),
                    color: to_color(occupant.bg),
                    size: Pixels(font_size),
                    font,
                    align_y: alignment::Vertical::Top,
                    ..Text::default()
                });
            }
        }
    }
}

/// 终端调色板 → 界面颜色。会话页画布四周的底色也用它填，
/// 让 10/12 的文字内边距和终端自己的背景连成一片（白底方案下也成立）。
pub(crate) fn to_color(rgb: Rgb) -> Color {
    Color::from_rgb8(rgb.r, rgb.g, rgb.b)
}

/// 单元格属性在渲染侧的唯一用途就是上面这些开关，这里留一个编译期断言，
/// 防止 `superremote_term::CellAttrs` 加字段后这里悄悄漏掉。
const _: fn() = || {
    let attrs = CellAttrs::default();
    let CellAttrs {
        bold: _,
        dim: _,
        italic: _,
        underline: _,
        strike: _,
        inverse: _,
        hidden: _,
        wide: _,
        spacer: _,
    } = attrs;
};

#[cfg(test)]
mod tests {
    use super::*;
    use superremote_term::CellAttrs;

    fn cell(c: char) -> Cell {
        Cell {
            c,
            fg: Rgb::new(0xBB, 0xC2, 0xCF),
            bg: Rgb::new(0x14, 0x18, 0x1F),
            attrs: CellAttrs::default(),
        }
    }

    fn row(text: &str) -> Vec<Cell> {
        text.chars().map(cell).collect()
    }

    #[test]
    fn words_after_a_space_restart_at_their_own_column() {
        // 这条用例守的是「叠字」：第二个词必须从第 6 列开始，而不是接着第一个词。
        let spans = row_spans(&row("hello world"));

        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].start, 0);
        assert_eq!(spans[0].text, "hello");
        assert_eq!(spans[1].start, 6, "空格后的词必须重新取起始列");
        assert_eq!(spans[1].text, "world");
    }

    #[test]
    fn a_style_change_splits_the_run_but_keeps_columns_contiguous() {
        let mut cells = row("abc");
        cells[2].attrs.bold = true;

        let spans = row_spans(&cells);
        assert_eq!(spans.len(), 2);
        assert_eq!((spans[0].start, spans[0].text.as_str()), (0, "ab"));
        assert_eq!((spans[1].start, spans[1].text.as_str()), (2, "c"));
        assert!(spans[1].style.bold);
    }

    #[test]
    fn a_wide_character_occupies_two_cells_and_its_spacer_is_skipped() {
        let mut cells = row("会话");
        // 模拟网格：'会' 占 0/1 两列，'话' 从第 2 列开始。
        cells[0].attrs.wide = true;
        let mut spacer = cell(' ');
        spacer.attrs.spacer = true;
        cells.insert(1, spacer);
        cells[2].attrs.wide = true;

        let spans = row_spans(&cells);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].text, "会话");
        assert_eq!(spans[0].start, 0);
        assert_eq!(spans[0].cells, 4, "两个宽字符共占四列");
    }

    #[test]
    fn trailing_blanks_do_not_create_spans() {
        assert!(row_spans(&row("      ")).is_empty());
    }
}
