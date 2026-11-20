use crate::core::{Color, Rectangle, Size};
use crate::graphics::compositor::{self, Information};
use crate::graphics::damage;
use crate::graphics::error::{self, Error};
use crate::graphics::{self, Shell, Viewport};
use crate::{Layer, Renderer, Settings};

use std::collections::VecDeque;
use std::num::NonZeroU32;

pub struct Compositor {
    context: softbuffer::Context<Box<dyn compositor::Display>>,
    settings: Settings,
}

pub struct Surface {
    window: softbuffer::Surface<Box<dyn compositor::Display>, Box<dyn compositor::Window>>,
    clip_mask: tiny_skia::Mask,
    layer_stack: VecDeque<Vec<Layer>>,
    background_color: Color,
    max_age: u8,
    /// 临时诊断：present 帧号 / 上一次 present 的物理尺寸 / 重新配置次数。
    /// 定位完拖拽明暗闪烁后整块删除（连同 `diag` 模块）。
    frames: u64,
    last_size: (u32, u32),
    configures: u32,
}

impl crate::graphics::Compositor for Compositor {
    type Renderer = Renderer;
    type Surface = Surface;

    async fn with_backend(
        settings: graphics::Settings,
        display: impl compositor::Display,
        _compatible_window: impl compositor::Window,
        _shell: Shell,
        backend: Option<&str>,
    ) -> Result<Self, Error> {
        match backend {
            None | Some("tiny-skia") | Some("tiny_skia") => Ok(new(settings.into(), display)),
            Some(backend) => Err(Error::GraphicsAdapterNotFound {
                backend: "tiny-skia",
                reason: error::Reason::DidNotMatch {
                    preferred_backend: backend.to_owned(),
                },
            }),
        }
    }

    fn create_renderer(&self) -> Self::Renderer {
        Renderer::new(self.settings.default_font, self.settings.default_text_size)
    }

    fn create_surface<W: compositor::Window + Clone>(
        &mut self,
        window: W,
        width: u32,
        height: u32,
    ) -> Self::Surface {
        let window = softbuffer::Surface::new(&self.context, Box::new(window.clone()) as _)
            .expect("Create softbuffer surface for window");

        let mut surface = Surface {
            window,
            clip_mask: tiny_skia::Mask::new(1, 1).expect("Create clip mask"),
            layer_stack: VecDeque::new(),
            background_color: Color::BLACK,
            max_age: 0,
            frames: 0,
            last_size: (0, 0),
            configures: 0,
        };

        if width > 0 && height > 0 {
            self.configure_surface(&mut surface, width, height);
        }

        surface
    }

    fn configure_surface(&mut self, surface: &mut Self::Surface, width: u32, height: u32) {
        surface
            .window
            .resize(
                NonZeroU32::new(width).expect("Non-zero width"),
                NonZeroU32::new(height).expect("Non-zero height"),
            )
            .expect("Resize surface");

        surface.clip_mask = tiny_skia::Mask::new(width, height).expect("Create clip mask");
        surface.layer_stack.clear();
        surface.configures += 1;
        diag::line(format!(
            "t={} event=configure size={width}x{height} configures={}",
            diag::stamp(),
            surface.configures
        ));
    }

    fn information(&self) -> Information {
        Information {
            adapter: String::from("CPU"),
            backend: String::from("tiny-skia"),
        }
    }

    fn present(
        &mut self,
        renderer: &mut Self::Renderer,
        surface: &mut Self::Surface,
        viewport: &Viewport,
        background_color: Color,
        on_pre_present: impl FnOnce(),
    ) -> Result<(), compositor::SurfaceError> {
        present(
            renderer,
            surface,
            viewport,
            background_color,
            on_pre_present,
        )
    }

    fn screenshot(
        &mut self,
        renderer: &mut Self::Renderer,
        viewport: &Viewport,
        background_color: Color,
    ) -> Vec<u8> {
        screenshot(renderer, viewport, background_color)
    }
}

pub fn new(settings: Settings, display: impl compositor::Display) -> Compositor {
    #[allow(unsafe_code)]
    let context =
        softbuffer::Context::new(Box::new(display) as _).expect("Create softbuffer context");

    Compositor { context, settings }
}

pub fn present(
    renderer: &mut Renderer,
    surface: &mut Surface,
    viewport: &Viewport,
    background_color: Color,
    on_pre_present: impl FnOnce(),
) -> Result<(), compositor::SurfaceError> {
    let physical_size = viewport.physical_size();

    let frame = surface.frames;
    surface.frames += 1;
    let resized = surface.last_size != (physical_size.width, physical_size.height);
    surface.last_size = (physical_size.width, physical_size.height);
    let started = std::time::Instant::now();

    let mut buffer = surface
        .window
        .buffer_mut()
        .map_err(|_| compositor::SurfaceError::Lost)?;

    let age = buffer.age();
    let stack_before = surface.layer_stack.len();
    // 绘制前的缓冲内容：age==0（surface 重建）时这里是未定义/清零值，
    // age>0 时是上一帧。两种情况的对比就是「暗帧」的直接证据。
    let pre = diag::sample(&buffer, physical_size.width, physical_size.height);
    let sampled_at = std::time::Instant::now();

    let last_layers = {
        surface.max_age = surface.max_age.max(age);
        surface.layer_stack.truncate(surface.max_age as usize);

        if age > 0 {
            surface.layer_stack.get(age as usize - 1)
        } else {
            None
        }
    };

    let forced = diag::force_full();
    let damage = if forced {
        vec![Rectangle::with_size(viewport.logical_size())]
    } else {
        last_layers
            .and_then(|last_layers| {
                (surface.background_color == background_color).then(|| {
                    // 兜底判据：先把整层与当前帧做一次相等比较。内容一模一样才允许
                    // 跳过绘制；只要有任何差异，就至少走脏区路径，脏区算空则整窗重绘。
                    // iced 0.14.1 原来直接信 `Layer::damage` 的结果，漏算的图元
                    // （阴影扩散、文本墨迹）会被永久留在屏幕上。
                    if last_layers.as_slice() == renderer.layers() {
                        Vec::new()
                    } else {
                        let damage = damage::diff(
                            last_layers,
                            renderer.layers(),
                            |layer| vec![layer.bounds],
                            Layer::damage,
                        );

                        if damage.is_empty() {
                            vec![Rectangle::with_size(viewport.logical_size())]
                        } else {
                            damage
                        }
                    }
                })
            })
            .unwrap_or_else(|| vec![Rectangle::with_size(viewport.logical_size())])
    };

    let damage_area: f32 = damage
        .iter()
        .map(|rect| rect.width * rect.height)
        .sum::<f32>();
    let logical = viewport.logical_size();
    let area_percent = if logical.width * logical.height > 0.0 {
        damage_area / (logical.width * logical.height) * 100.0
    } else {
        0.0
    };
    let drew = !damage.is_empty();
    let damage_rects = damage.len();

    if damage.is_empty() {
        if let Some(last_layers) = last_layers {
            surface.layer_stack.push_front(last_layers.clone());
        }
    } else {
        surface.layer_stack.push_front(renderer.layers().to_vec());
        surface.background_color = background_color;

        let damage = coalesce(damage::group(
            damage,
            Rectangle::with_size(viewport.logical_size()),
        ));

        let mut pixels = tiny_skia::PixmapMut::from_bytes(
            bytemuck::cast_slice_mut(&mut buffer),
            physical_size.width,
            physical_size.height,
        )
        .expect("Create pixel map");

        renderer.draw(
            &mut pixels,
            &mut surface.clip_mask,
            viewport,
            &damage,
            background_color,
        );
    }

    let draw_ended = std::time::Instant::now();
    let post = diag::sample(&buffer, physical_size.width, physical_size.height);

    on_pre_present();
    let result = buffer.present();
    let presented = std::time::Instant::now();

    if diag::enabled() {
        diag::line(format!(
            "t={} event=present frame={frame} size={}x{} age={age} resized={} cfg={} stack={}>{} damage={} area={area_percent:.1}% drew={} force={} sample_us={} draw_us={} present_us={} total_us={} pre=[{pre}] post=[{post}]",
            diag::stamp(),
            physical_size.width,
            physical_size.height,
            u8::from(resized),
            surface.configures,
            stack_before,
            surface.layer_stack.len(),
            damage_rects,
            u8::from(drew),
            u8::from(forced),
            (sampled_at - started).as_micros(),
            (draw_ended - sampled_at).as_micros(),
            (presented - draw_ended).as_micros(),
            (presented - started).as_micros(),
        ));
    }

    result.map_err(|_| compositor::SurfaceError::Lost)
}

/// 把大量零碎脏区收敛成一个包围盒。
///
/// `Renderer::draw` 会对**每个**脏区重新遍历全部图层和图元。切语言时整页
/// 逐条文本变更会产生近百个小区块（实测 96 块），单帧从 ~30ms 涨到 450ms；
/// 这些文本块本身已经覆盖了大部分窗口，合成一个包围盒只多画极少量像素，
/// 却把图元遍历从 96 次降到 1 次。
///
/// 稀疏脏区（包围盒远大于脏区总面积，例如终端里两处零散光标变化）不合并，
/// 避免为了几行文字重画整个终端。
const MAX_DAMAGE_RECTS: usize = 4;
const MAX_DAMAGE_OVERDRAW: f32 = 3.0;

fn coalesce(damage: Vec<Rectangle>) -> Vec<Rectangle> {
    if damage.len() <= MAX_DAMAGE_RECTS {
        return damage;
    }

    let area: f32 = damage
        .iter()
        .map(|rect| rect.width.max(0.0) * rect.height.max(0.0))
        .sum();

    let mut union = damage[0];
    for rect in &damage[1..] {
        union = union.union(rect);
    }

    let overdraw = union.width.max(0.0) * union.height.max(0.0) / area.max(1.0);

    if overdraw <= MAX_DAMAGE_OVERDRAW {
        vec![union]
    } else {
        damage
    }
}

/// 临时诊断：把每一帧的 present 判据与缓冲像素写进日志文件。
///
/// 目的：定位拖拽缩放时「亮一下、暗一下」交替闪烁发生在
/// 「自己画的那一帧」还是「没画、直接 present 的那一帧」。
/// 路径：`SR2_FRAME_LOG` 指定；未指定时写到 `%TEMP%\sr2-frame.log`。
/// 对照实验：设置 `SR2_FULL_REPAINT=1` 后每帧强制整窗重绘。
/// 定位完成后整块删除（连同 `Surface` 里的三个诊断字段）。
mod diag {
    use std::fs::{File, OpenOptions};
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::{Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn sink() -> Option<&'static Mutex<File>> {
        static SINK: OnceLock<Option<Mutex<File>>> = OnceLock::new();

        SINK.get_or_init(|| {
            // 本轮取证默认打开：写到 `%TEMP%\sr2-frame.log`，
            // `SR2_FRAME_LOG` 可以改路径。
            let path = std::env::var_os("SR2_FRAME_LOG")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("TEMP").map(|dir| PathBuf::from(dir).join("sr2-frame.log"))
                })?;

            OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .ok()
                .map(Mutex::new)
        })
        .as_ref()
    }

    pub fn enabled() -> bool {
        sink().is_some()
    }

    pub fn line(text: String) {
        let Some(sink) = sink() else {
            return;
        };

        if let Ok(mut file) = sink.lock() {
            let _ = writeln!(file, "{text}");
        }
    }

    /// 毫秒级时间戳：应用侧与合成器侧的日志按它对齐。
    pub fn stamp() -> u128 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis())
            .unwrap_or(0)
    }

    pub fn force_full() -> bool {
        static VALUE: OnceLock<bool> = OnceLock::new();

        *VALUE.get_or_init(|| std::env::var_os("SR2_FULL_REPAINT").is_some())
    }

    /// 取四个代表点：标题栏、窗口中心、右边缘、下边缘。
    /// 缓冲像素在 Win32 上是 `0x00RRGGBB`，这里输出成 `#RRGGBB`。
    pub fn sample(buffer: &[u32], width: u32, height: u32) -> String {
        if !enabled() {
            return String::new();
        }
        if width == 0 || height == 0 {
            return "----,----,----,----".to_owned();
        }

        let pick = |x: u32, y: u32| -> String {
            let index = (y as usize) * (width as usize) + (x as usize);

            match buffer.get(index) {
                Some(pixel) => format!("#{:06x}", pixel & 0x00FF_FFFF),
                None => "----".to_owned(),
            }
        };

        [
            pick(width / 2, 8.min(height - 1)),
            pick(width / 2, height / 2),
            pick(width.saturating_sub(2), height / 2),
            pick(width / 2, height.saturating_sub(2)),
        ]
        .join(",")
    }
}

pub fn screenshot(
    renderer: &mut Renderer,
    viewport: &Viewport,
    background_color: Color,
) -> Vec<u8> {
    let size = viewport.physical_size();

    let mut offscreen_buffer: Vec<u32> = vec![0; size.width as usize * size.height as usize];

    let mut clip_mask = tiny_skia::Mask::new(size.width, size.height).expect("Create clip mask");

    renderer.draw(
        &mut tiny_skia::PixmapMut::from_bytes(
            bytemuck::cast_slice_mut(&mut offscreen_buffer),
            size.width,
            size.height,
        )
        .expect("Create offscreen pixel map"),
        &mut clip_mask,
        viewport,
        &[Rectangle::with_size(Size::new(
            size.width as f32,
            size.height as f32,
        ))],
        background_color,
    );

    offscreen_buffer.iter().fold(
        Vec::with_capacity(offscreen_buffer.len() * 4),
        |mut acc, pixel| {
            const A_MASK: u32 = 0xFF_00_00_00;
            const R_MASK: u32 = 0x00_FF_00_00;
            const G_MASK: u32 = 0x00_00_FF_00;
            const B_MASK: u32 = 0x00_00_00_FF;

            let a = ((A_MASK & pixel) >> 24) as u8;
            let r = ((R_MASK & pixel) >> 16) as u8;
            let g = ((G_MASK & pixel) >> 8) as u8;
            let b = (B_MASK & pixel) as u8;

            acc.extend([r, g, b, a]);
            acc
        },
    )
}

#[cfg(test)]
mod tests {
    use super::coalesce;
    use crate::core::{Rectangle, Size};

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rectangle {
        Rectangle::new(crate::core::Point::new(x, y), Size::new(w, h))
    }

    /// 整页文本逐条变更会产生大量相邻小块；必须合成一块，否则渲染器会
    /// 按块数重复遍历全部图元（真实回归：96 块、单帧 450ms）。
    #[test]
    fn many_adjacent_damage_rects_are_coalesced() {
        let damage: Vec<_> = (0..96)
            .map(|index| {
                let row = index % 12;
                let column = index / 12;
                rect(
                    20.0 + column as f32 * 200.0,
                    30.0 + row as f32 * 40.0,
                    180.0,
                    20.0,
                )
            })
            .collect();

        assert_eq!(coalesce(damage).len(), 1);
    }

    /// 只有几块时保持原样：光标闪烁、按钮悬停这类小范围重绘不该被放大。
    #[test]
    fn few_damage_rects_are_left_alone() {
        let damage = vec![rect(10.0, 10.0, 20.0, 20.0), rect(300.0, 300.0, 20.0, 20.0)];

        assert_eq!(coalesce(damage).len(), 2);
    }

    /// 两处相距很远、面积很小的脏区不合并，避免重画整个窗口。
    #[test]
    fn sparse_damage_rects_are_not_coalesced() {
        let mut damage = Vec::new();
        for index in 0..6 {
            damage.push(rect(10.0 + index as f32, 10.0, 4.0, 4.0));
        }
        damage.push(rect(1700.0, 1000.0, 4.0, 4.0));

        assert_eq!(coalesce(damage).len(), 7);
    }
}
