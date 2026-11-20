//! 自绘图标：界面里不再借用符号字体（Segoe UI Symbol）的字形。
//!
//! 字体只负责文字。图标用几何路径画，线宽、尺寸、颜色都在代码里，
//! 不会因为某台机器缺字形而回退到另一款字体、变成另一个粗细。

use iced::widget::canvas as canvas_widget;
use iced::widget::canvas::{self, Frame as CanvasFrame, Geometry, Path};
use iced::{mouse, Color, Element, Fill, Point, Rectangle, Renderer, Theme};

use crate::app::Message;

#[derive(Clone, Copy)]
pub enum RailGlyph {
    Home,
    List,
    Settings,
}

struct RailIcon {
    glyph: RailGlyph,
    color: Color,
}

impl canvas::Program<Message> for RailIcon {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = CanvasFrame::new(renderer, bounds.size());
        let (w, h) = (bounds.width, bounds.height);
        let stroke = canvas::Stroke {
            line_cap: canvas::LineCap::Round,
            line_join: canvas::LineJoin::Round,
            ..canvas::Stroke::default()
        }
        .with_color(self.color)
        .with_width(1.25);

        match self.glyph {
            RailGlyph::Home => {
                let roof = Path::new(|p| {
                    p.move_to(Point::new(w * 0.13, h * 0.48));
                    p.line_to(Point::new(w * 0.50, h * 0.13));
                    p.line_to(Point::new(w * 0.87, h * 0.48));
                });
                frame.stroke(&roof, stroke);

                let walls = Path::new(|p| {
                    p.move_to(Point::new(w * 0.24, h * 0.43));
                    p.line_to(Point::new(w * 0.24, h * 0.87));
                    p.line_to(Point::new(w * 0.76, h * 0.87));
                    p.line_to(Point::new(w * 0.76, h * 0.43));
                });
                frame.stroke(&walls, stroke);
            }
            RailGlyph::List => {
                for row in [0.24_f32, 0.50, 0.76] {
                    frame.fill(
                        &Path::circle(Point::new(w * 0.16, h * row), 0.9),
                        self.color,
                    );
                    frame.stroke(
                        &Path::line(Point::new(w * 0.31, h * row), Point::new(w * 0.88, h * row)),
                        stroke,
                    );
                }
            }
            RailGlyph::Settings => {
                // 齿轮：厚圆环 + 8 根短粗齿。齿不能细长——15px 下会看成太阳。
                let center = Point::new(w / 2.0, h / 2.0);
                let thick = stroke.with_width(2.0);
                frame.stroke(&Path::circle(center, w * 0.23), thick);

                let teeth = 8;
                for index in 0..teeth {
                    let angle = std::f32::consts::TAU * index as f32 / teeth as f32;
                    let inner = w * 0.28;
                    let outer = w * 0.40;
                    frame.stroke(
                        &Path::line(
                            Point::new(
                                center.x + angle.cos() * inner,
                                center.y + angle.sin() * inner,
                            ),
                            Point::new(
                                center.x + angle.cos() * outer,
                                center.y + angle.sin() * outer,
                            ),
                        ),
                        thick,
                    );
                }
            }
        }

        vec![frame.into_geometry()]
    }
}

/// 图标栏的一项。`color` 由调用方按选中态给（选中是 on_accent，否则 text_dim）。
pub fn rail(glyph: RailGlyph, color: Color) -> Element<'static, Message> {
    canvas_widget(RailIcon { glyph, color })
        .width(15.0)
        .height(15.0)
        .into()
}

struct Caret {
    open: bool,
    color: Color,
}

impl canvas::Program<Message> for Caret {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = CanvasFrame::new(renderer, bounds.size());
        let cx = bounds.width / 2.0;
        let cy = bounds.height / 2.0;
        let half = 2.4;
        let depth = 1.8;

        let path = Path::new(|builder| {
            if self.open {
                builder.move_to(Point::new(cx - half, cy - depth));
                builder.line_to(Point::new(cx + half, cy - depth));
                builder.line_to(Point::new(cx, cy + depth));
            } else {
                builder.move_to(Point::new(cx - depth, cy - half));
                builder.line_to(Point::new(cx - depth, cy + half));
                builder.line_to(Point::new(cx + depth, cy));
            }
            builder.close();
        });
        frame.fill(&path, self.color);

        vec![frame.into_geometry()]
    }
}

/// 目录行的展开箭头。固定 10×14，与旧的 `▾/▸` 占位一致。
pub fn caret(open: bool, color: Color) -> Element<'static, Message> {
    canvas_widget(Caret { open, color })
        .width(10.0)
        .height(14.0)
        .into()
}

/// 标题栏与全屏栏的窗口控件图形。
///
/// 与侧栏 [`rail`] 共用同一套视觉语言：同一线宽（1.25）、圆头圆角、同一墨迹
/// 尺寸（约 10 逻辑像素）。上一版照 Lucide 在 10×10 网格里画，实际墨迹只有
/// 6.5px，线宽 1.5 且用坐标 1.75 这类四分之一数值，在 150% DPI 下每条边都
/// 摊成 2~3 个灰像素——所以比 15px 的侧栏图标既小又发灰，形状本身倒是没画错。
#[derive(Clone, Copy)]
pub enum WindowGlyph {
    Minimize,
    Maximize,
    /// 已最大化：后框右下角被前框压住的标准「还原」。
    Restore,
    /// 进入全屏：四角外扩。
    Expand,
    /// 退出全屏：四角内收，与最大化的方块区分。
    Collapse,
    Close,
}

struct WindowControl {
    glyph: WindowGlyph,
    color: Color,
    /// 悬停/按下色。`button` 不会把状态传给内容，所以画布自己按光标是否在
    /// 格子里判断——格子被画布填满，命中区与按钮完全一致。
    hot: Color,
}

impl canvas::Program<Message> for WindowControl {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = CanvasFrame::new(renderer, bounds.size());
        let color = if cursor.is_over(bounds) {
            self.hot
        } else {
            self.color
        };
        // 与 `RailIcon` 完全同一组笔画参数：窗口控件和侧栏图标放在同一屏里，
        // 线宽差 0.25 或端帽一方一圆都会立刻露馅。
        let stroke = canvas::Stroke {
            line_cap: canvas::LineCap::Round,
            line_join: canvas::LineJoin::Round,
            ..canvas::Stroke::default()
        }
        .with_color(color)
        .with_width(1.25);

        // 14×14 网格、整数原点：三种标题条高度下图形大小恒定，墨迹 10~11.25px，
        // 与侧栏 15px 画布的墨迹（约 11px）同量级；不再用 1.75/8.25 这种四分之一
        // 坐标，避免 150% 缩放下边缘摊成灰边。
        let x0 = ((bounds.width - 14.0) / 2.0).round();
        let y0 = ((bounds.height - 14.0) / 2.0).round();
        let at = |x: f32, y: f32| Point::new(x0 + x, y0 + y);

        match self.glyph {
            WindowGlyph::Minimize => {
                frame.stroke(&Path::line(at(2.5, 7.0), at(11.5, 7.0)), stroke);
            }
            WindowGlyph::Maximize => {
                frame.stroke(&square(at, 2.0, 12.0), stroke);
            }
            WindowGlyph::Restore => {
                // 后框的右下角被前框压住，只画露出来的 C 形（与 Lucide `copy`
                // 同一解法）：两层描边不会在交叠处叠成双线。
                let back = Path::new(|path| {
                    path.move_to(at(5.0, 9.0));
                    path.line_to(at(2.0, 9.0));
                    path.line_to(at(2.0, 2.0));
                    path.line_to(at(9.0, 2.0));
                    path.line_to(at(9.0, 5.0));
                });
                frame.stroke(&back, stroke);
                frame.stroke(&square(at, 5.0, 12.0), stroke);
            }
            WindowGlyph::Expand => {
                // 外角括号，四臂都朝内。
                for (x, y, arm_x, arm_y) in [
                    (5.5, 2.0, 2.0, 5.5),
                    (8.5, 2.0, 12.0, 5.5),
                    (5.5, 12.0, 2.0, 8.5),
                    (8.5, 12.0, 12.0, 8.5),
                ] {
                    frame.stroke(&bracket(at, x, y, arm_x, arm_y), stroke);
                }
            }
            WindowGlyph::Collapse => {
                // 折点收到 (5.5,5.5)/(8.5,8.5)，两臂朝外，读作「向内收」。
                for (x, y, arm_x, arm_y) in [
                    (2.0, 5.5, 5.5, 2.0),
                    (12.0, 5.5, 8.5, 2.0),
                    (2.0, 8.5, 5.5, 12.0),
                    (12.0, 8.5, 8.5, 12.0),
                ] {
                    frame.stroke(&bracket(at, x, y, arm_x, arm_y), stroke);
                }
            }
            WindowGlyph::Close => {
                frame.stroke(&Path::line(at(2.0, 2.0), at(12.0, 12.0)), stroke);
                frame.stroke(&Path::line(at(12.0, 2.0), at(2.0, 12.0)), stroke);
            }
        }

        vec![frame.into_geometry()]
    }
}

/// 网格里的一个正方形描边路径（坐标由 `at` 映射到画布）。
fn square(at: impl Fn(f32, f32) -> Point, from: f32, to: f32) -> Path {
    Path::new(|path| {
        path.move_to(at(from, from));
        path.line_to(at(to, from));
        path.line_to(at(to, to));
        path.line_to(at(from, to));
        path.close();
    })
}

/// 两段折线组成的角括号：(`x,y`) 出发到拐点，再拐向 (`arm_x`,`arm_y`)。
fn bracket(at: impl Fn(f32, f32) -> Point, x: f32, y: f32, arm_x: f32, arm_y: f32) -> Path {
    Path::new(|path| {
        path.move_to(at(x, y));
        path.line_to(at(arm_x, y));
        path.line_to(at(arm_x, arm_y));
    })
}

/// 窗口控件图标。常态色与悬停色由调用方从主题里取，不在这里写死色值。
pub fn window_control(glyph: WindowGlyph, color: Color, hot: Color) -> Element<'static, Message> {
    canvas_widget(WindowControl { glyph, color, hot })
        .width(Fill)
        .height(Fill)
        .into()
}
