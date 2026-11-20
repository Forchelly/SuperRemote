# 旧版功能对照与差距清单

> 对照对象：`legacy/SimoleRemote`（C#/WPF + RDP ActiveX）与当前 Rust 工作区（`crates/`）。
> 起因：用户指出旧版可调分辨率、可控声音，要求逐项核对当前缺失项。
> 结论先行：**声音/录音当前已实现，分辨率缺「全屏档」与「系统动态分辨率」；另有 5 项设置类缺口与 1 项 UI 缺口。**

---

## 一、RDP 设置（重点：分辨率与声音）

旧版数据模型 `DbItemSettingRdp` + 设置界面 `SettingRdp.xaml`/`SettingUtility.xaml`；
新版数据模型 `RdpSettings`（`superremote-core/src/session.rs`）+ 编辑页 `rdp_settings_card`（`superremote-app/src/ui/editor.rs`）。
两侧都支持「不设=继承全局默认」的覆盖语义（旧版用 `0/null` 表示继承，新版用 `Option::None`）。

| # | 功能点 | 旧版实现 | 当前实现 | 差距 |
|---|---|---|---|---|
| 1 | **分辨率** | `SizeIndex`：使用默认 / **自适应** / **全屏** / **系统动态分辨率列表**（`EnumDisplaySettings` 枚举显卡全部模式，≥800px，如 1366×768、1600×900、1920×1200…） | `RdpDesktopSize`：跟随窗口 / 1280×720 / 1920×1080 / 2560×1440 / 3840×2160 | ⚠️ **缺 2 项**：①「全屏」作为持久化设置（打开即全屏，当前只能打开后再点全屏按钮）②系统动态分辨率（当前固定 4 档，非标分辨率如 1600×900 选不了） |
| 2 | **音频播放**（声音去哪） | `AudioRedirectionMode`：使用默认 / **在本机播放** / **在远程计算机播放** / **不要播放** → `AudioRedirectionMode` | `RdpAudio`：Local（本机）/ Remote（远端）/ None（不播放）+ 继承全局 | ✅ **已实现**，语义一一对应（`audiomode:i: 0/1/2`） |
| 3 | **录音重定向** | `AudioCaptureRedirectionMode`：使用默认 / 从此计算机录制 / 不录制 | `audio_capture: Option<bool>` + 继承 | ✅ 已实现 |
| 4 | 性能 | `Performance`：默认 / 调制解调器 / 低速带宽 / 卫星 / 高速宽带 / WAN / LAN / 自动检测 | `RdpPerformance`：Auto/Modem/BroadbandLow/Satellite/BroadbandHigh/Wan/Lan | ✅ 已实现（7 档全） |
| 5 | 颜色深度 | `ColorDepthMode`：默认 / 增强色15位 / 16位 / 真彩24位 / 最高32位 | `RdpColorDepth`：15/16/24/32 bpp | ✅ 已实现 |
| 6 | 组合键 | `KeyboardHookMode`：默认 / 在这台计算机 / 在远程桌面 / 在全屏显示时 | `RdpKeyboard`：Local/Remote/Fullscreen + 继承 | ✅ 已实现 |
| 7 | 重定向打印机 | `RedirectionPrintf` 三态 | `redirect_printers: Option<bool>` | ✅ 已实现 |
| 8 | 重定向剪贴板 | `RedirectionClipboard` 三态 | `redirect_clipboard: Option<bool>` | ✅ 已实现 |
| 9 | 重定向智能卡 | `RedirectionsMartcard` 三态 | `redirect_smartcards: Option<bool>` | ✅ 已实现 |
| 10 | 重定向端口 | `RedirectionsPort` 三态 | `redirect_ports: Option<bool>` | ✅ 已实现 |
| 11 | 重定向驱动器 | `RedirectionsDriver` 三态 | `redirect_drives: Option<bool>` | ✅ 已实现 |

**分辨率差距细化（#1）：**

- 旧版 `GetSystemResolutions()` 用 `EnumDisplaySettings` 逐个枚举显卡上报的显示模式（去重宽度、过滤 <800px、倒序排大分辨率在前），下拉框里是完整的分辨率表。当前固定 4 档覆盖不了「笔记本 1600×900 / 1366×768」「带鱼屏 3440×1440」这类真实场景。
- 旧版 `SizeIndex == DESKSIZE_FULLSCREEN` 时取主屏分辨率并由 `GetIsFullscreen()` 判定全屏——**分辨率设置里内嵌了「打开即全屏」**。当前全屏只是会话页头的一个运行时按钮（`ToggleRdpFullscreen`），没有持久化的「本连接默认全屏打开」。

**声音对照细化（#2）——用户关注点，当前不缺：**

| 行为 | 旧版 | 当前 `.rdp` 键 |
|---|---|---|
| 在本机播放 | `AudioRedirectionMode = 0` | `audiomode:i:0` |
| 在远程计算机播放 | `AudioRedirectionMode = 1` | `audiomode:i:1` |
| 不播放 | `AudioRedirectionMode = 2` | `audiomode:i:2` |
| 录音 | `AudioCaptureRedirectionMode` | `audiocapturemode:i:0/1` |

编辑页「RDP 设置」卡片里就有「音频」「录音」下拉，且支持「继承全局默认」。

---

## 二、通用连接设置（基类 `DbItemSetting`，所有协议共享）

| # | 功能点 | 旧版实现 | 当前实现 | 差距 |
|---|---|---|---|---|
| 12 | **首选打开方式** | `OpenMode`：使用默认 / 使用选项卡 / 使用选项卡(后台) / 使用外部窗口——**每条连接可持久化偏好**，双击即按偏好打开 | 三个动作（打开 / 在后台打开 / 在新窗口打开）都在右键菜单里**手动选**，无持久化 | ⚠️ **缺**：每连接（及全局）的默认打开方式 |
| 13 | 独立窗口尺寸记忆 | `ExternalWindowWidth/Height/IsMaximize`（`DbItemRemoteLink`）：上次独立窗口的大小/最大化状态写回连接项 | 独立窗口固定 980×660，每次新开同尺寸 | ⚠️ **缺**：记住上次独立窗口尺寸 |

---

## 三、全局默认设置的编辑入口

| # | 功能点 | 旧版实现 | 当前实现 | 差距 |
|---|---|---|---|---|
| 14 | **全局默认设置编辑器** | `Home_Setting` 里挂 `SettingRdp`/`SettingSsh`/`SettingTelnet`，「设为默认设置」按钮调 `SetDefaultSetting()` 写库 | 数据层**已支持**（`Store::rdp_defaults/set_rdp_defaults`、`terminal_defaults/set_terminal_defaults`，`config.toml` 落盘）；编辑页下拉也显示「继承（当前值）」 | ⚠️ **UI 缺失**：设置页只有外观/语言/数据目录/凭据加密四张卡片，**改不了 RDP 与终端的全局默认**。用户看到「继承（LAN、32 位…）」却无处改这些默认值 |

这是「旧版有分辨率/声音设置，新版是不是没有」的另一半答案：**单连接层面有，全局默认层面能存不能改。**

---

## 四、终端（SSH/Telnet）设置对照

旧版 `SettingPutty.xaml`（SSH/Telnet 共用）；新版 `TerminalSettings` + `terminal_settings_card`。

| # | 功能点 | 旧版实现 | 当前实现 | 差距 |
|---|---|---|---|---|
| 15 | 光标形状 | 使用默认 / 显示块 / 下划线 / 垂直线 | `TerminalCursor`：Block / Underline / Beam | ✅ 已实现 |
| 16 | 字体 | 系统等宽字体下拉（`GetEqualWidthFonts()`） | `fonts.rs` 用 `EnumFontFamiliesExW` 枚举等宽族，下拉可选；未列出的已存值以「（自定义）」追加 | ✅ 已实现（判据与旧版一致：只收固定字距） |
| 17 | 字号 | 下拉 | `font_size: Option<u8>` | ✅ 已实现 |
| 18 | **字符集** | 系统编码下拉（`Encoding.GetEncodings()`） | 无此项 | ⚠️ **缺** |
| 19 | 回退键 | 使用默认 / Backspace / Delete | `TerminalBackspace`：Backspace / Delete | ✅ 已实现 |
| 20 | **鼠标动作** | 使用默认 / 混合模式(中键扩展，右键粘贴) / xterm(右键扩展，中键粘贴) / Windows(中键扩展，右键弹出菜单) | 无此项（固定行为：拖选 + Ctrl+Shift+V 粘贴 + 右键粘贴） | ⚠️ **缺** |
| 21 | 配色方案 | **本文原判断有误**：`GetPuttyColorlNames()` 读的是它自己的 LiteDB `PuttyColor` 表，旧版 README 写的也是「内置多套 SSH/Telnet 配色方案」，全仓库没有读 PuTTY 注册表或导入配色文件的代码 | `TerminalColorScheme` 内置 5 套：默认深色、Solarized 明/暗、Linux 控制台（VGA 表）、xterm（X11 表） | ✅ 与旧版对齐（同为内置方案）。若将来要做「导入 PuTTY 配色」，那是**新增能力**而不是补差，需要先定来源（注册表 `HKCU\Software\SimonTatham\PuTTY` 还是 `.reg`/TOML 文件）与文件选择器方案 |
| 22 | Home/End | 使用默认 / 标准 / rxvt | `TerminalHomeEnd`：Standard / Rxvt | ✅ 已实现 |
| 23 | **Fn 与小键盘** | 使用默认 / ESC[n~ / Linux | 无此项 | ⚠️ **缺** |

---

## 五、连接行为层（旧版硬编码，非用户设置）

`RemoteControl_rdp.Connect()` 里固定打开的行为，新版 `.rdp` 生成（`rdp.rs`）对应情况：

| 行为 | 旧版 | 当前 |
|---|---|---|
| 位图缓存 | `BitmapPeristence=1` + `BitmapPersistence=1` | mstsc 默认开启（`smart sizing:i:1` 等） |
| 压缩 | `Compress=1` | `networkautodetect/bandwidthautodetect` 按性能档写 |
| 禁用连接栏 | `DisableConnectionBar=true` | 内嵌到自有窗口，无 mstsc 连接栏 |
| 连接时不抢焦点 | `GrabFocusOnConnect=false` | `rdp_host` 按需 `show/hide` |
| CredSSP | `EnableCredSspSupport=true` | mstsc 默认开启 |
| 凭据提示 | `PromptForCredentials=false` + `AllowPromptingForCredentials=true` | `prompt for credentials:i:0`，凭据走系统凭据库 |
| 自适应分辨率的取值 | 控件物理像素尺寸（高 DPI 不二次放大） | `pane_size` 按窗口 DPI 换算 |

行为层无缺口，且新版在多窗口内嵌场景更完整（按实例贴边）。

---

## 六、差距汇总与建议优先级

### P0（用户直接可感，建议先做）

| # | 缺口 | 改法要点 |
|---|---|---|
| 1a | 分辨率缺「全屏」档 | `RdpDesktopSize` 加 `Fullscreen` 变体：`desktopwidth/height` 写主屏分辨率，`session.kind==Rdp` 打开时自动进全屏（复用 `rdp_fullscreen` 机制） |
| 1b | 分辨率缺系统动态分辨率 | 启动时 `EnumDisplaySettings` 枚举可选模式（或预置 1366×768/1600×900/1680×1050/1920×1200/3440×1440 补齐），`RdpDesktopSize` 从闭集枚举改为「预设 + Custom(w,h)」 |
| 14 | 全局默认设置改不了 | 设置页加「连接默认」卡片（RDP + 终端两块），复用编辑页的 `pick_field` 控件，落 `set_rdp_defaults`/`set_terminal_defaults` |

### P1（补齐全流程一致性）

| # | 缺口 | 改法要点 |
|---|---|---|
| 12 | 首选打开方式 | `SessionSettings` 加 `open_mode: Option<OpenMode>`（Tab/Background/Detached），树双击/菜单「打开」按偏好分发；全局默认同样进设置页 |
| 13 | 独立窗口尺寸记忆 | `Session` 或 `SessionSettings` 存 `external_window: Option<(f32,f32,bool)>`，关窗时写回 |

### P2（终端补齐，体验类）

| # | 缺口 | 改法要点 |
|---|---|---|
| 18 | 字符集 | 终端设置加编码下拉（`encoding_rs` 支持的常用编码即可，不必全量 GetEncodings） |
| 20 | 鼠标动作 | 终端设置加选择模式/粘贴键位（3 选 1，对应旧版混合/xterm/Windows） |
| 23 | Fn/小键盘 | 终端设置加键盘模式（ESC[n~ / Linux） |
| 16/21 | 字体列表、自定义配色 | 改善型：字体下拉（枚举等宽字体）、配色导入 |

### 不缺的（防误判）

声音播放、录音、颜色深度、性能档、组合键、5 项重定向、光标形状、回退键、Home/End——**旧版有的这些当前都有**，且继承语义一致。

---

## 七、复核与收口状态（2026-10-28）

本文的 P0/P1/P2 六项已全部落地。逐项现状与验证方式：

| # | 缺口 | 现状 | 验证方式 |
|---|---|---|---|
| 1a | 分辨率「全屏」档 | `RdpDesktopSize::Fullscreen`；`.rdp` 的 `desktopwidth/height` 写主屏分辨率（`display::primary_size()`），`starts_fullscreen()` 让这条连接**打开即全屏** | `only_the_fullscreen_choice_starts_fullscreen`、`desktop_size_codes_round_trip_and_old_presets_still_parse`；`launch_rdp` 分支 |
| 1b | 系统动态分辨率 | `display.rs` 用 `EnumDisplaySettingsW` 枚举显示模式，并入 `DESKTOP_PRESETS` 后去重降序，缓存在 `OnceLock` | `presets_survive_an_empty_system_list`、`system_modes_are_merged_deduped_and_sorted`、`out_of_range_modes_are_dropped` |
| 14 | 全局默认设置改不了 | 设置页新增「远程桌面默认」「打开方式默认」「终端默认」三张卡，与编辑页共用 `ui::fields` 控件，落 `set_rdp_defaults` / `set_terminal_defaults` / `set_open_mode`，各有「恢复内置默认」 | 无窗口渲染核对过整页排版；`rdp_overrides_and_global_defaults_round_trip`、`terminal_overrides_and_defaults_round_trip` |
| 12 | 首选打开方式 | `SessionSettings::open_mode`（`None` = 用全局默认），双击/回车/详情页「打开连接」走 `open_preferred`，右键三项仍是显式指定 | `the_preferred_open_mode_decides_where_a_connection_lands`、`the_preferred_open_mode_survives_a_restart` |
| 13 | 独立窗口尺寸记忆 | `Session::window`（整数逻辑像素，关窗时 `remember_window_size` 写回），`detached_size` 读取并夹到最小值，没记录时用 980×660 | `a_detached_window_remembers_its_size_for_the_next_time`（app.rs） |
| 18 | 字符集 | `TerminalCharset`（UTF-8/GBK/Big5/Latin-1），非 UTF-8 走 `encoding_rs` **增量**解码（`Terminal::decode_chunk`，解码器按连接持有）；只影响收，发送仍是 UTF-8 | `non_utf8_charsets_are_transcoded_before_the_grid`、`a_multibyte_character_split_across_reads_still_decodes`、`a_utf16_bom_in_a_gbk_stream_is_not_sniffed` |
| 20 | 鼠标动作 | `TerminalMouseMode`（混合/xterm/Windows），右键与中键的语义按模式切换（`right_click_pastes` / `middle_click_pastes`） | 无自动化用例（要真窗口点鼠标）；Windows 模式暂退化为「右键粘贴」（菜单未做），代码里已注明 |
| 23 | Fn 与小键盘 | `TerminalKeypad`（标准/ESC[n~/Linux）决定 F1–F4 的转义序列 | `function_keys_follow_the_keypad_setting`（含「F5 不受影响」） |
| 16 | 字体列表 | `fonts.rs` 枚举系统等宽字体族供下拉选择 | `enumerated_families_are_clean_sorted_and_unique`、`windows_enumeration_reaches_beyond_the_fallback_list`、`fields::font_choices` 三条用例 |
| 21 | 配色 | 内置 5 套（见第四节勘误） | `console_and_xterm_use_the_documented_palettes`、`every_palette_keeps_foreground_and_background_apart`、`every_terminal_scheme_code_round_trips` |

门禁实测（本机，2026-10-28）：`cargo test --workspace` 176 passed / 0 failed，
`cargo clippy --workspace --all-targets -- -D warnings` 退出码 0。
协议连通性（真实 RDP 主机、非 UTF-8 远端主机）仍未验证——没有对应的测试目标。

---

## 附：旧版功能点索引（出处）

| 功能组 | 文件 |
|---|---|
| RDP 设置模型 | `legacy/SimoleRemote/Modes/DbItemSettingRdp.cs` |
| 基类（打开方式/分辨率） | `legacy/SimoleRemote/Modes/DbItemSetting.cs` |
| RDP 设置界面 | `legacy/SimoleRemote/Container/RemoteSetting/SettingRdp.xaml` |
| 分辨率/打开方式界面 | `legacy/SimoleRemote/Container/RemoteSetting/SettingUtility.xaml` |
| 系统分辨率枚举 | `legacy/SimoleRemote/Core/CommonServices.cs`（`GetSystemResolutions`） |
| 终端设置界面 | `legacy/SimoleRemote/Container/RemoteSetting/SettingPutty.xaml` |
| 设置应用到 RDP 控件 | `legacy/SimoleRemote/Container/RemoteControl_rdp.xaml.cs`（`Connect`） |
| 连接项（窗口尺寸记忆） | `legacy/SimoleRemote/Modes/DbItemRemoteLink.cs` |
