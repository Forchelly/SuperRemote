//! 设置页跨语言版式稳定性。
//!
//! 中英文的文案行数不同（说明、标签都可能从一行变两行），如果高度由文案撑开，
//! 切语言会把后面所有卡片往下推、滚动位置也会跳。这里把同一尺寸下两种语言的
//! 关键文字 y 坐标逐项对比，要求完全一致；同时守住全屏时内容左对齐。

use iced::{window, Settings, Size, Theme};
use iced_test::Simulator;
use std::path::PathBuf;
use superremote_app::app::{App, Message};
use superremote_app::i18n::Lang;
use superremote_app::theme;
use superremote_store::DataPaths;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "superremote-settings-layout-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
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

fn simulator<'a>(
    app: &'a App,
    id: window::Id,
    size: Size,
) -> Simulator<'a, Message, Theme, iced::Renderer> {
    Simulator::with_size(
        Settings {
            default_font: theme::ui_font(),
            ..Settings::default()
        },
        size,
        app.view(id),
    )
}

fn y_of(sim: &mut Simulator<'_, Message>, text: &str) -> f32 {
    sim.find(text)
        .unwrap_or_else(|_| panic!("设置页里找不到文字：{text}"))
        .visible_bounds()
        .unwrap_or_else(|| panic!("文字不可见：{text}"))
        .y
}

fn x_of(sim: &mut Simulator<'_, Message>, text: &str) -> f32 {
    sim.find(text)
        .unwrap_or_else(|_| panic!("设置页里找不到文字：{text}"))
        .visible_bounds()
        .unwrap_or_else(|| panic!("文字不可见：{text}"))
        .x
}

/// 两种语言下都必须存在的关键文字，按 (中文, English) 成对列出。
const TEXTS: &[(&str, &str)] = &[
    ("外观", "Appearance"),
    ("语言", "Language"),
    ("打开方式默认", "Default open mode"),
    ("数据目录", "Data folder"),
    ("凭据加密", "Credential encryption"),
    ("远程桌面默认", "Remote Desktop defaults"),
    ("终端默认", "Terminal defaults"),
    ("新主口令", "New password"),
    ("启用主口令", "Enable master password"),
    ("取消主口令", "Remove password"),
    ("功能键", "Function keys"),
    ("鼠标动作", "Mouse"),
];

fn assert_stable(tag: &str, size: Size) {
    let temp = TempDir::new(tag);
    let mut app = App::with_paths(temp.paths());
    let id = window::Id::unique();
    let _ = app.update(Message::WindowResized(id, size));
    let _ = app.update(Message::ShowSettings);

    let measure = |app: &mut App, lang: Lang| {
        let _ = app.update(Message::SetLang(lang));
        let mut sim = simulator(app, id, size);
        TEXTS
            .iter()
            .map(|(zh, en)| {
                let text = match lang {
                    Lang::Zh => *zh,
                    Lang::En => *en,
                };
                (text, y_of(&mut sim, text), x_of(&mut sim, text))
            })
            .collect::<Vec<_>>()
    };

    let zh = measure(&mut app, Lang::Zh);
    let en = measure(&mut app, Lang::En);

    for ((zh_text, zh_y, _), (en_text, en_y, _)) in zh.iter().zip(&en) {
        assert!(
            (zh_y - en_y).abs() < 0.1,
            "[{tag}] 切语言后「{zh_text} / {en_text}」纵向偏移：{zh_y} vs {en_y}"
        );
    }

    // 页头与第一张卡片左边缘对齐；全屏时内容从侧栏右侧 16px 起排，而不是居中。
    let mut sim = simulator(&app, id, size);
    let header_x = x_of(&mut sim, "Settings");
    let first_card_x = x_of(&mut sim, "Appearance");
    assert!(
        (header_x - first_card_x).abs() < 0.5,
        "[{tag}] 页头与卡片没有左对齐：{header_x} vs {first_card_x}"
    );
    assert!(header_x < 400.0, "[{tag}] 内容没有贴左：页头 x={header_x}");

    let y = |sim: &mut Simulator<'_, Message>, text: &str| y_of(sim, text);
    if size.width > 1400.0 {
        // 宽屏：两列。远程桌面默认与外观同一行，但在右列。
        let appearance_y = y(&mut sim, "Appearance");
        let rdp_y = y(&mut sim, "Remote Desktop defaults");
        let appearance_x = x_of(&mut sim, "Appearance");
        let rdp_x = x_of(&mut sim, "Remote Desktop defaults");
        assert!(
            (appearance_y - rdp_y).abs() < 0.1,
            "[{tag}] 宽屏两列没有顶对齐：{appearance_y} vs {rdp_y}"
        );
        assert!(
            rdp_x > appearance_x + 100.0,
            "[{tag}] 宽屏右列没有落到右侧：{appearance_x} vs {rdp_x}"
        );
    } else {
        // 窄屏：单列，连接默认排在应用级卡片之后。
        let credential_y = y(&mut sim, "Credential encryption");
        let rdp_y = y(&mut sim, "Remote Desktop defaults");
        assert!(
            rdp_y > credential_y + 40.0,
            "[{tag}] 窄屏没有改成单列：{credential_y} vs {rdp_y}"
        );
    }
}

#[test]
fn settings_layout_is_stable_across_languages() {
    // 高度取足够大，保证窄屏单列时下面的卡片也能被量到；列数只由宽度决定。
    assert_stable("narrow", Size::new(1180.0, 1600.0));
    assert_stable("wide", Size::new(1706.0, 1018.0));
    assert_stable("wide-hd", Size::new(2200.0, 1200.0));
}
