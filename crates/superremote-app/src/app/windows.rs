use super::*;

use iced::widget::{column, row};

impl App {
    pub(super) fn window_handle_task(window: window::Id) -> Task<Message> {
        window::run(window, rdp_host::hwnd_of).map(move |hwnd| Message::WindowHandle(window, hwnd))
    }

    /// 窗口真正开出来之后再查句柄。
    ///
    /// `window::run` 的 Action 在窗口还没注册进 `window_manager` 时会被**静默丢弃**
    /// （`if let Some(window) = ...` 直接跳过），句柄查询必须挂在 `window::open`
    /// 的回执后面，否则 `WindowHandle` 永远不会到达、RDP 永远不会启动。
    pub(super) fn window_ready_task(opened: Task<window::Id>) -> Task<Message> {
        opened.then(|window| {
            Task::batch([
                Task::done(Message::WindowOpened(window)),
                Self::window_handle_task(window),
            ])
        })
    }

    /// 关掉一个窗口。独立窗口顺手收掉它那一个会话；关主窗口则收掉全部 RDP 进程。
    pub(super) fn close_window(&mut self, window: window::Id) -> Task<Message> {
        let detached = match self.windows.get(&window) {
            Some(WindowSlot::Detached(tab)) => Some(tab.instance),
            _ => None,
        };

        if let Some(instance) = detached {
            self.forget_terminal(instance);
        } else {
            for (_, mut process) in self.rdp.drain() {
                process.close();
            }
        }

        window::close(window)
    }

    /// 最后一个窗口关掉之后是否该结束整个进程。
    ///
    /// 关主窗口而独立窗口还开着时**不**退出：那些窗口里是正在跑的会话，
    /// 用户没让它们结束。等最后一个窗口也没了才退出。
    pub(super) fn should_exit_after_close(&self) -> bool {
        self.windows.is_empty()
    }

    /// 某个运行实例待在哪个窗口里（主窗口的标签，或某个独立窗口）。
    pub(super) fn window_of_instance(&self, instance: TabId) -> Option<window::Id> {
        for (id, slot) in &self.windows {
            match slot {
                WindowSlot::Main => {
                    if self.tabs.tabs().iter().any(|tab| tab.instance == instance) {
                        return Some(*id);
                    }
                }
                WindowSlot::Detached(tab) if tab.instance == instance => return Some(*id),
                WindowSlot::Detached(_) => {}
            }
        }
        None
    }

    pub(super) fn with_window(
        action: impl Fn(window::Id) -> Task<Message> + Send + 'static,
    ) -> Task<Message> {
        window::latest().and_then(action)
    }

    pub(super) fn set_window_fullscreen(fullscreen: bool) -> Task<Message> {
        Self::with_window(move |id| {
            window::set_mode(
                id,
                if fullscreen {
                    window::Mode::Fullscreen
                } else {
                    window::Mode::Windowed
                },
            )
        })
    }

    /// 全屏的那一路被关掉/断开后，把窗口从全屏模式放回来。
    ///
    /// 视图已经切回普通布局，如果系统窗口还停在全屏，标题栏会贴在屏幕最上沿，
    /// 用户找不到还原按钮——所以这里必须同时改窗口模式。
    pub(super) fn leave_fullscreen_if_gone(&mut self) -> Task<Message> {
        let Some(instance) = self.rdp_fullscreen else {
            return Task::none();
        };

        if self.rdp.contains_key(&instance) {
            return Task::none();
        }

        self.rdp_fullscreen = None;
        self.sync_rdp_layout();
        Self::set_window_fullscreen(false)
    }

    /// 这个窗口是否已经最大化（按最后一次切换记的状态）。
    pub(crate) fn window_maximized(&self, window: window::Id) -> bool {
        self.maximized.contains(&window)
    }

    /// 独立窗口：一条可拖拽的标题条 + 会话画面。没有侧栏、没有标签条。
    /// 无边框窗口自己补上八条缩放热区（`decorations: false` 会连原生边框一起去掉）。
    pub(super) fn detached_view<'a>(
        &'a self,
        window: window::Id,
        tab: &'a Tab,
    ) -> Element<'a, Message> {
        let body = column![
            ui::chrome::detached_bar(self, tab),
            ui::workspace::detached_body(self, tab),
        ]
        .width(Fill)
        .height(Fill);

        ui::chrome::with_resize_edges(body.into(), window)
    }

    /// 主窗口的逻辑宽度。
    ///
    /// 主窗口还没登记（启动首帧、或用例里直接 `view`）时，取已知窗口里
    /// 最宽的那个：宁可先按大屏排，也不要在大屏上先闪一帧窄版面。
    pub(super) fn main_width(&self) -> f32 {
        self.main_window
            .and_then(|window| self.window_sizes.get(&window))
            .or_else(|| self.window_sizes.values().max_by_key(|size| size.width))
            .map(|size| size.width as f32)
            .unwrap_or(1180.0)
    }

    /// 手动拖边缩放的最小尺寸，与 `window::Settings::min_size` 保持一致。
    pub(super) fn window_min_size(&self, window: window::Id) -> (f32, f32) {
        if self.main_window == Some(window) {
            (MAIN_MIN_W, MAIN_MIN_H)
        } else {
            (DETACHED_MIN_W, DETACHED_MIN_H)
        }
    }

    /// 工作区可用宽度：窗口宽 − 图标栏 − 侧栏 − 两条 1px 分隔线。
    ///
    /// 设置页的分栏断点按它算（见 `ui::WIDE_MIN`）。主窗口尺寸还没回来时
    /// 按默认窗口 1180 算，正好落在窄版区间，不会在启动瞬间闪一下宽版面。
    pub(super) fn workspace_width(&self) -> f32 {
        (self.main_width() - ui::RAIL_W - ui::SIDEBAR_W - 2.0).max(0.0)
    }

    pub(super) fn main_view(&self, window: window::Id) -> Element<'_, Message> {
        // RDP 全屏时不画标题栏、侧栏和标签条，只留顶部 30px 的退出栏。
        // 原生画面是 Iced 窗口的子窗口，会盖在这块占位区域上。
        if self.rdp_fullscreen.is_some() {
            return iced::widget::column![
                ui::chrome::fullscreen_bar(self),
                ui::workspace::fullscreen_view(self),
            ]
            .width(Fill)
            .height(Fill)
            .into();
        }

        let page = match &self.screen {
            Screen::Unlock => ui::unlock::view(self),
            Screen::Fatal(message) => ui::unlock::fatal(message),
            Screen::List => row![
                ui::sidebar::rail(self),
                ui::vertical_rule(),
                ui::list::view(self, self.main_width())
            ]
            .width(Fill)
            .height(Fill)
            .into(),
            Screen::Remote | Screen::Settings | Screen::NewTab => {
                // 会话画面整窗铺开（终稿 ②）：图标栏和连接列表都让位给远程画面，
                // 切回主页/设置时再出现。标签条留在标题栏里，随时能切回来。
                let session_full = matches!(&self.screen, Screen::Remote)
                    && !self.show_home
                    && self.active_tab().is_some();

                if session_full {
                    ui::workspace::view(self, self.main_width())
                } else {
                    let available = self.workspace_width();

                    row![
                        ui::sidebar::rail(self),
                        ui::vertical_rule(),
                        ui::sidebar::view(self),
                        ui::vertical_rule(),
                        ui::workspace::view(self, available)
                    ]
                    .width(Fill)
                    .height(Fill)
                    .into()
                }
            }
        };

        let with_title = iced::widget::column![ui::chrome::title_bar(self), page]
            .width(Fill)
            .height(Fill);

        // 提示条不在这里：它画在自己的窗口里，页面叠层会被原生 RDP 画面整个盖掉。
        ui::chrome::with_resize_edges(with_title.into(), window)
    }

    /// 在鼠标位置开一个会话菜单窗口（独立窗口，不是 overlay，原因见 [`MenuPopup`]）。
    ///
    /// 控件上报的锚点是父窗口内的逻辑坐标，而 `Position::Specific` 要的是桌面坐标，
    /// 所以得先知道父窗口在哪。句柄在窗口开出来那一刻就登记进 `hwnds` 了；
    /// 问不到位置就干脆不开——菜单凭空出现在屏幕中间，比不出现更让人摸不着头脑。
    pub(super) fn open_session_menu(&mut self, instance: TabId, anchor: Point) -> Task<Message> {
        let Some(origin) = self
            .window_of_instance(instance)
            .and_then(|window| self.hwnds.get(&window).copied())
            .and_then(rdp_host::window_origin)
        else {
            return Task::none();
        };

        self.show_menu(instance, anchor, origin)
    }

    /// 菜单窗口本体：尺寸就是面板，位置 = 父窗口左上角 + 锚点。
    /// 单独拆出来是为了能在没有真实窗口的用例里验证这套换算。
    pub(super) fn show_menu(
        &mut self,
        instance: TabId,
        anchor: Point,
        origin: (f32, f32),
    ) -> Task<Message> {
        let Some(tab) = self.tab_of(instance) else {
            return Task::none();
        };
        // 独立窗口没有标签条：「关闭其他标签」「在新窗口打开」对它没有意义。
        let detached = self
            .tabs
            .tabs()
            .iter()
            .all(|entry| entry.instance != instance);
        let (items, groups) = ui::workspace::session_actions(tab, detached);
        let size = ui::context_menu::panel_size(&items, &groups);
        let position = Point::new(origin.0 + anchor.x, origin.1 + anchor.y);

        // 已经开着就先关掉：菜单窗口是置顶的，漏一个就会一直挂在屏幕上。
        // 正常路径是先失焦再按下，这里兜住的是事件顺序不保证的那一面。
        let stale = self.close_session_menu();
        let (window, opened) = window::open(popup_window_settings(size, position, true));
        self.menu = Some(MenuPopup {
            window,
            target: instance,
            items,
            groups,
        });

        Task::batch([stale, opened.map(Message::WindowOpened)])
    }

    /// 收起会话菜单，并把键盘焦点还给远端画面：菜单窗口刚才抢走了焦点，
    /// 不还回去的话用户接着打字会打在已经关掉的窗口上。
    pub(super) fn close_session_menu(&mut self) -> Task<Message> {
        let Some(menu) = self.menu.take() else {
            return Task::none();
        };

        if self.rdp.contains_key(&menu.target) && self.active_instances().contains(&menu.target) {
            if let Some(hwnd) = self.hwnd_of_instance(menu.target) {
                rdp_host::focus(hwnd);
            }
        }

        window::close(menu.window)
    }

    /// 窗口生命周期事件。菜单只关心一件事：自己失焦 = 用户点了别处 = 收起。
    pub(super) fn on_window_event(
        &mut self,
        window: window::Id,
        event: iced::window::Event,
    ) -> Task<Message> {
        let owns_menu = self.menu.as_ref().is_some_and(|menu| menu.window == window);

        match event {
            iced::window::Event::Unfocused if owns_menu => self.close_session_menu(),
            iced::window::Event::Closed if owns_menu => {
                self.menu = None;
                Task::none()
            }
            _ => Task::none(),
        }
    }

    /// 提示条窗口跟着 `status` 开合，并在新消息更长/更短时把窗口改到新的尺寸。
    pub(super) fn sync_toast(&mut self) -> Task<Message> {
        if let Some((window, shown)) = self.toast {
            return match self.status.as_ref() {
                None => {
                    self.toast = None;
                    window::close(window)
                }
                Some(status) => {
                    let size = ui::toast_size(&status.message);
                    if size == shown {
                        Task::none()
                    } else {
                        self.toast = Some((window, size));
                        window::resize(window, size)
                    }
                }
            };
        }

        let Some(message) = self.status.as_ref().map(|status| status.message.clone()) else {
            return Task::none();
        };

        self.open_toast(&message)
    }

    /// 在父窗口底部居中开一个提示条窗口。
    ///
    /// 按 `visible: false` 开、拿到句柄后再 `SW_SHOWNA` 显示：提示条不能抢键盘焦点
    /// ——远端桌面里正打字时，一条「已保存」把焦点带走是事故而不是便利。
    pub(super) fn open_toast(&mut self, message: &str) -> Task<Message> {
        // 挂在哪个窗口：主窗口优先；主窗口关了（只剩独立窗口）就挂还在的那个。
        let Some(parent) = self
            .main_window
            .or_else(|| self.windows.keys().next().copied())
        else {
            return Task::none();
        };
        let Some(origin) = self
            .hwnds
            .get(&parent)
            .copied()
            .and_then(rdp_host::window_origin)
        else {
            return Task::none();
        };

        let size = ui::toast_size(message);
        let parent_size = self
            .window_sizes
            .get(&parent)
            .copied()
            .unwrap_or(WindowSize {
                width: 1180,
                height: 760,
            });
        let position = Point::new(
            origin.0 + (parent_size.width as f32 - size.width) / 2.0,
            origin.1 + parent_size.height as f32 - size.height - TOAST_BOTTOM,
        );

        let (window, opened) = window::open(popup_window_settings(size, position, false));
        self.toast = Some((window, size));

        // 句柄查询必须挂在 `window::open` 的回执之后，否则会被静默丢弃。
        opened.then(move |window| {
            window::run(window, rdp_host::hwnd_of)
                .map(move |hwnd| Message::ToastHandle(window, hwnd))
        })
    }

    /// 把独立窗口关闭时的尺寸写回连接项，下次「在新窗口打开」直接复用。
    ///
    /// 尺寸来自最近一次 resize 事件；一次都没量到（窗口开出来就被关掉）时不写，
    /// 免得把默认的 980×660 当成用户手动调过的结果记下来。
    pub(super) fn remember_window_size(&mut self, id: &str, size: Option<WindowSize>) {
        let Some(size) = size else {
            return;
        };
        let Some(store) = self.store.as_mut() else {
            return;
        };

        let _ = store
            .tree_mut()
            .with_session_mut(id, |session| session.window = Some(size));
    }
}
