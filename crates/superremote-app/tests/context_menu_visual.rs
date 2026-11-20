//! 右键菜单/悬停提示的**像素级**回归。
//!
//! 只验证「消息发出去了」不够：菜单以 overlay 形式挂在控件树上，任何一层包装
//! （滚动区、Stack、窗口描边、自定义 `ContextMenu` 的转发）漏掉 overlay，菜单
//! 就会静默消失——状态位和消息全对，窗口里什么都没有。更隐蔽的是 `menu::Menu`
//! 的宽度默认是 0，不给宽度就画出一条零宽的边，同样一个像素都不变。
//!
//! 所以这里用 `iced_test::Simulator` 把真实 `App` 视图渲染两遍（操作前/后），
//! 逐像素比对，证明弹层真的出现在鼠标位置附近。

use iced::{mouse, window, Event, Point, Settings, Size, Theme};
use iced_test::Simulator;
use std::path::{Path, PathBuf};
use superremote_app::app::{App, DraftField, Message};
use superremote_app::theme;
use superremote_core::session::SessionKind;
use superremote_store::DataPaths;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "superremote-menu-visual-{tag}-{}-{}",
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

fn send(app: &mut App, message: Message) {
    let _ = app.update(message);
}

/// 建一条 SSH 连接并选中它，让主页树里有行可右键。
fn app_with_one_row(temp: &TempDir) -> App {
    let mut app = App::with_paths(temp.paths());
    send(&mut app, Message::NewSession(SessionKind::Ssh));
    for (field, value) in [
        (DraftField::Name, "跳板机"),
        (DraftField::Host, "10.0.0.1"),
        (DraftField::Port, "22"),
        (DraftField::Username, "root"),
    ] {
        send(&mut app, Message::DraftChanged(field, value.to_owned()));
    }
    send(&mut app, Message::ApplyDraft);
    app
}

fn simulator(app: &App) -> Simulator<'_, Message, Theme, iced::Renderer> {
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
    height: u32,
    pixels: Vec<u8>,
}

/// 拍一张快照并解码成像素。`iced_test` 只会给「整图是否相等」，定位问题需要原图。
fn snap(sim: &mut Simulator<'_, Message>, dir: &Path, name: &str) -> Rgba {
    let path = dir.join(format!("{name}.png"));
    let snapshot = sim.snapshot(&Theme::Light).unwrap();
    assert!(snapshot.matches_image(&path).unwrap());
    // 渲染后端名字会被拼进文件名；这里只跑 tiny-skia。
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
        height: info.height,
        pixels,
    }
}

struct Diff {
    count: usize,
    min: (u32, u32),
    max: (u32, u32),
}

fn diff(before: &Rgba, after: &Rgba) -> Diff {
    assert_eq!((before.width, before.height), (after.width, after.height));

    let mut out = Diff {
        count: 0,
        min: (u32::MAX, u32::MAX),
        max: (0, 0),
    };
    for y in 0..before.height {
        for x in 0..before.width {
            let i = ((y * before.width + x) * 4) as usize;
            if before.pixels[i..i + 4] != after.pixels[i..i + 4] {
                out.count += 1;
                out.min = (out.min.0.min(x), out.min.1.min(y));
                out.max = (out.max.0.max(x), out.max.1.max(y));
            }
        }
    }
    out
}

/// 像素坐标是物理像素（快照按 2 倍缩放），断言前先换算回逻辑坐标。
#[track_caller]
fn assert_menu_near(anchor: Point, diff: &Diff, snapshot_width: u32, min_changed: usize) {
    let scale = snapshot_width as f32 / 1180.0;
    eprintln!(
        "anchor={anchor:?} changed={} bbox={:?}..{:?} scale={scale}",
        diff.count, diff.min, diff.max
    );

    assert!(
        diff.count > min_changed,
        "操作后几乎没有像素变化（{}）：弹层没有画出来",
        diff.count
    );

    let ax = (anchor.x * scale) as u32;
    let ay = (anchor.y * scale) as u32;
    let max_x = (400.0 * scale) as u32;
    let max_y = (560.0 * scale) as u32;

    // 菜单可能向上展开（点击点靠近窗口底部时），所以纵向允许在锚点两侧。
    assert!(
        diff.min.0 >= ax.saturating_sub(16)
            && diff.min.1 >= ay.saturating_sub(max_y)
            && diff.max.0 <= ax + max_x
            && diff.max.1 <= ay + max_y,
        "弹层像素不在鼠标位置附近：bbox={:?}..{:?}，锚点=({ax},{ay})",
        diff.min,
        diff.max
    );
}

fn right_press(sim: &mut Simulator<'_, Message>, at: Point) {
    sim.point_at(at);
    let statuses = sim.simulate([Event::Mouse(mouse::Event::ButtonPressed(
        mouse::Button::Right,
    ))]);
    assert!(
        statuses
            .iter()
            .any(|status| status == &iced::event::Status::Captured),
        "右键应被菜单控件捕获，实际 {statuses:?}"
    );
}

/// 树行：右键后菜单必须从鼠标位置向下展开。
#[test]
fn right_clicking_a_tree_row_draws_a_visible_menu() {
    let temp = TempDir::new("row");
    let app = app_with_one_row(&temp);
    let mut sim = simulator(&app);

    let target = sim.find("10.0.0.1:22").expect("树行里的地址文字");
    let anchor = target.visible_bounds().expect("行在可视区内").center();
    assert!(
        anchor.x < 400.0,
        "地址应落在左侧栏里，实际 {anchor:?}——选择器抓到了别的页面"
    );

    let before = snap(&mut sim, temp.path(), "row-before");
    right_press(&mut sim, anchor);
    let after = snap(&mut sim, temp.path(), "row-after");

    assert_menu_near(anchor, &diff(&before, &after), before.width, 20_000);
}

/// 树空白处：根层菜单（新建 / 粘贴）同样要出现。
#[test]
fn right_clicking_blank_tree_space_draws_the_root_menu() {
    let temp = TempDir::new("blank");
    let app = app_with_one_row(&temp);
    let mut sim = simulator(&app);

    let anchor = Point::new(150.0, 620.0);
    let before = snap(&mut sim, temp.path(), "blank-before");
    right_press(&mut sim, anchor);
    let after = snap(&mut sim, temp.path(), "blank-after");

    // 空白区菜单的底色与侧栏同为 surface，只有描边与文字会变，故阈值低一些。
    assert_menu_near(anchor, &diff(&before, &after), before.width, 2_000);
}

/// 连接列表页是另一套视图（`Screen::List`），行菜单也要单独证明。
#[test]
fn right_clicking_a_list_row_draws_a_visible_menu() {
    let temp = TempDir::new("list");
    let mut app = app_with_one_row(&temp);
    send(&mut app, Message::ShowList);
    let mut sim = simulator(&app);

    let target = sim.find("10.0.0.1:22").expect("列表行里的地址文字");
    let anchor = target.visible_bounds().expect("行在可视区内").center();

    // 先把光标放到行上，让悬停底色进入基线；否则右键后的差异里会混进
    // 整行的悬停底色（列表行没有选中态，底色只由 hover 决定）。
    sim.point_at(anchor);
    let _ = sim.simulate([Event::Mouse(mouse::Event::CursorMoved { position: anchor })]);
    let before = snap(&mut sim, temp.path(), "list-before");
    right_press(&mut sim, anchor);
    let after = snap(&mut sim, temp.path(), "list-after");

    assert_menu_near(anchor, &diff(&before, &after), before.width, 20_000);
}

/// 图标栏的悬停提示：延迟结束后必须画出气泡。
#[test]
fn hovering_the_settings_icon_shows_its_tooltip() {
    let temp = TempDir::new("tooltip");
    let app = app_with_one_row(&temp);
    let mut sim = simulator(&app);

    // 图标栏宽 56；标题栏 38 + 10 内边距 + 两枚 34px 图标与两段 6px 间距后，
    // 第三枚（设置）的中心在 (28, 145)。图标是自绘路径，选择器找不到文字。
    let anchor = Point::new(28.0, 145.0);
    let before = snap(&mut sim, temp.path(), "tip-before");

    sim.point_at(anchor);
    let _ = sim.simulate([Event::Mouse(mouse::Event::CursorMoved { position: anchor })]);
    // tooltip 延迟 350ms 后才打开；overlay 在下一帧开头才重新计算，
    // 所以第一张快照只是推进状态机，第二张才是真正画出来的画面。
    std::thread::sleep(std::time::Duration::from_millis(450));
    let _ = snap(&mut sim, temp.path(), "tip-warmup");
    let after = snap(&mut sim, temp.path(), "tip-after");

    let diff = diff(&before, &after);
    eprintln!(
        "tooltip changed={} bbox={:?}..{:?}",
        diff.count, diff.min, diff.max
    );

    // 悬停态本身只改图标底色（56 宽的一列）；气泡在图标右侧，必须有点亮像素。
    let bubble_edge = (60.0 * after.width as f32 / 1180.0) as u32;
    let bubble_pixels = (diff.min.0..=diff.max.0)
        .filter(|x| *x > bubble_edge)
        .count();
    assert!(
        bubble_pixels > 0 && diff.count > 300,
        "设置图标的悬停提示没有出现：changed={}，右侧像素={bubble_pixels}",
        diff.count
    );
}

/// 窗口四周的 1px 描边必须真的画出来。
///
/// 它从「一块整窗大小的带边框容器」拆成了四条细线（后者每帧多铺一遍全屏
/// 透明底，实测 +15ms @2x）。拆完如果漏了某条边，窗口在浅色桌面上就没有边界。
#[test]
fn window_frame_has_a_visible_line_on_every_edge() {
    let temp = TempDir::new("frame");
    let app = App::with_paths(temp.paths());
    let mut sim = simulator(&app);
    let shot = snap(&mut sim, temp.path(), "frame");

    let line = superremote_app::theme::palette().line_strong;
    let expected = [
        (line.r * 255.0).round() as u8,
        (line.g * 255.0).round() as u8,
        (line.b * 255.0).round() as u8,
    ];

    let pixel = |x: u32, y: u32| {
        let i = ((y * shot.width + x) * 4) as usize;
        [shot.pixels[i], shot.pixels[i + 1], shot.pixels[i + 2]]
    };
    let close = |color: [u8; 3]| {
        color
            .iter()
            .zip(expected.iter())
            .all(|(actual, want)| actual.abs_diff(*want) <= 4)
    };

    let (w, h) = (shot.width, shot.height);
    let mut sides = [false; 4];
    for step in 0..3 {
        sides[0] |= (0..w).any(|x| close(pixel(x, step)));
        sides[1] |= (0..w).any(|x| close(pixel(x, h - 1 - step)));
        sides[2] |= (0..h).any(|y| close(pixel(step, y)));
        sides[3] |= (0..h).any(|y| close(pixel(w - 1 - step, y)));
    }

    assert!(
        sides.iter().all(|visible| *visible),
        "窗口描边缺边：上/下/左/右 = {sides:?}"
    );
}
