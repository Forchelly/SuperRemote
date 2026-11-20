//! 品牌标的像素回归：标题栏与解锁页都必须画的是**新 logo**（黑底 + 白线），
//! 而不是旧的蓝色 "S" 方块，也不是一块空白。
//!
//! 只断言「有一个 Image 控件」不够：图片句柄没加载、被裁掉、透明度全 0 时
//! 控件树完全正常，画面里却什么都没有。所以这里和右键菜单测试一样，渲染真实
//! 视图后按像素统计。

use iced::{window, Settings, Size, Theme};
use iced_test::Simulator;
use std::path::{Path, PathBuf};
use superremote_app::app::App;
use superremote_app::theme;
use superremote_store::DataPaths;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "superremote-brand-visual-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn paths(&self) -> DataPaths {
        DataPaths::from_dir(&self.0)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn simulator(app: &App) -> Simulator<'_, superremote_app::app::Message, Theme, iced::Renderer> {
    Simulator::with_size(
        Settings {
            default_font: theme::ui_font(),
            ..Settings::default()
        },
        Size::new(1180.0, 760.0),
        app.view(window::Id::unique()),
    )
}

struct Rgba {
    width: u32,
    pixels: Vec<u8>,
}

fn snap(sim: &mut Simulator<'_, superremote_app::app::Message>, dir: &Path, name: &str) -> Rgba {
    let path = dir.join(format!("{name}.png"));
    let snapshot = sim.snapshot(&Theme::Light).unwrap();
    assert!(snapshot.matches_image(&path).unwrap());
    let path = path.with_file_name(format!("{name}-tiny-skia.png"));
    let file = std::fs::File::open(&path).unwrap();
    let decoder = png::Decoder::new(std::io::BufReader::new(file));
    let mut reader = decoder.read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut pixels).unwrap();
    pixels.truncate(info.buffer_size());
    assert_eq!(info.color_type, png::ColorType::Rgba);
    Rgba {
        width: info.width,
        pixels,
    }
}

impl Rgba {
    fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.width + x) * 4) as usize;
        [
            self.pixels[i],
            self.pixels[i + 1],
            self.pixels[i + 2],
            self.pixels[i + 3],
        ]
    }

    /// 一块区域里黑底 / 白线 / 强调蓝各自的像素数。
    fn logo_classes(&self, x0: u32, y0: u32, x1: u32, y1: u32) -> (usize, usize, usize) {
        let (mut dark, mut bright, mut accent) = (0, 0, 0);
        for y in y0..y1 {
            for x in x0..x1 {
                let [r, g, b, _] = self.pixel(x, y);
                if r < 80 && g < 80 && b < 80 {
                    dark += 1;
                }
                if r > 120 && g > 120 && b > 120 {
                    bright += 1;
                }
                // 旧品牌块是 #2B6CE0：蓝色明显高于红/绿。
                if b > 150 && b > r.saturating_add(60) {
                    accent += 1;
                }
            }
        }
        (dark, bright, accent)
    }
}

#[test]
fn the_title_bar_shows_the_real_logo() {
    let temp = TempDir::new("titlebar");
    let app = App::with_paths(temp.paths());
    let mut sim = simulator(&app);
    let shot = snap(&mut sim, temp.path(), "titlebar");

    let scale = shot.width as f32 / 1180.0;
    // 标题栏行高 37、logo 20 → 上下各 8.5；左侧内边距 14。
    // 取内缩 4px 的核心区，排除 logo 自带的透明圆角（那里露出标题栏白底）。
    let x0 = ((14.0 + 4.0) * scale).round() as u32;
    let y0 = ((8.5 + 4.0) * scale).round() as u32;
    let x1 = ((34.0 - 4.0) * scale).round() as u32;
    let y1 = ((28.5 - 4.0) * scale).round() as u32;

    let (dark, bright, accent) = shot.logo_classes(x0, y0, x1, y1);
    eprintln!(
        "titlebar logo: dark={dark} bright={bright} accent={accent} box={x0},{y0}..{x1},{y1}"
    );

    assert!(
        dark > 100,
        "标题栏没有黑底 logo：核心区只有 {dark} 个暗像素"
    );
    assert!(bright > 20, "logo 的白线没有画出来：只有 {bright} 个亮像素");
    assert_eq!(
        accent, 0,
        "标题栏还在画旧的蓝色品牌块：出现 {accent} 个强调蓝像素"
    );
}

#[test]
fn the_unlock_screen_shows_the_real_logo() {
    let temp = TempDir::new("unlock");
    let app = App::with_paths(temp.paths());
    // 直接渲染解锁页本身：外面那层标题栏已有单独的用例，这里只证明页面上
    // 确实有品牌标，不被标题栏和标题文字混进包围盒。
    let mut sim = Simulator::with_size(
        Settings {
            default_font: theme::ui_font(),
            ..Settings::default()
        },
        Size::new(1180.0, 760.0),
        superremote_app::ui::unlock::view(&app),
    );
    let shot = snap(&mut sim, temp.path(), "unlock");

    let scale = shot.width as f32 / 1180.0;
    let x0 = (440.0 * scale) as u32;
    let x1 = (740.0 * scale) as u32;
    let y0 = (120.0 * scale) as u32;
    let y1 = (640.0 * scale) as u32;

    let row_dark: Vec<usize> = (y0..y1)
        .map(|y| (x0..x1).filter(|&x| is_dark(shot.pixel(x, y))).count())
        .collect();

    // 从上往下第一块暗色就是 logo。它的高度是确定的 64 逻辑像素；
    // 下面的标题文字在这个窗口里更窄、更矮，不会影响这条上边界。
    let top_i = row_dark
        .iter()
        .position(|n| *n > 20)
        .expect("解锁页看不到品牌标");
    let top = y0 + top_i as u32;
    let logo_h = (64.0 * scale).round() as u32;
    let bottom = top + logo_h - 1;

    let cols: Vec<u32> = (x0..x1)
        .filter(|&x| {
            (top..=bottom)
                .filter(|&y| is_dark(shot.pixel(x, y)))
                .count()
                > 10
        })
        .collect();
    assert!(!cols.is_empty(), "解锁页看不到品牌标");
    let (left, right) = (cols[0], *cols.last().unwrap());
    let w = right - left + 1;
    eprintln!("unlock logo bbox: {left},{top} {w}x{logo_h}");

    assert!(
        (100..=160).contains(&w),
        "解锁页品牌标的宽度不对：{w}（期望约 128 物理像素）"
    );
    let (dark, bright, accent) = shot.logo_classes(left + 8, top + 8, right - 8, bottom - 8);
    eprintln!("unlock logo: dark={dark} bright={bright} accent={accent}");
    assert!(dark > 5000, "解锁页品牌标没有黑底：暗像素只有 {dark}");
    assert!(bright > 30, "解锁页品牌标没有白线：亮像素只有 {bright}");
    assert_eq!(accent, 0, "解锁页品牌标混进了强调蓝：{accent}");

    // 四个高度带都要有内容，避免只画出了 logo 的一角也通过上面的总数。
    for band in 0..4 {
        let band_top = top + logo_h / 4 * band;
        let band_bottom = band_top + logo_h / 4;
        let dark = (left..right)
            .flat_map(|x| (band_top..band_bottom).map(move |y| (x, y)))
            .filter(|&(x, y)| is_dark(shot.pixel(x, y)))
            .count();
        assert!(dark > 200, "解锁页 logo 第 {band} 段是空的：{dark}");
    }
}

fn is_dark([r, g, b, _]: [u8; 4]) -> bool {
    r < 80 && g < 80 && b < 80
}
