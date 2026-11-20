use super::*;

impl App {
    pub(crate) fn active_tab(&self) -> Option<&Tab> {
        if self.show_home {
            None
        } else {
            self.tabs.active_tab()
        }
    }

    pub(super) fn instance_at(&self, index: usize) -> Option<TabId> {
        self.tabs.tabs().get(index).map(|tab| tab.instance)
    }

    /// 会话菜单的动作。会话页没有页头之后，重连/全屏/断开都在这里；
    /// 标签条与独立窗口标题条共用这一份分发，所以每一步都按运行实例找归属。
    pub(super) fn run_tab_action(&mut self, action: TabAction) -> Task<Message> {
        match action {
            TabAction::Reconnect(instance) => self.update(Message::Reconnect(instance)),
            TabAction::Fullscreen(instance) => self.update(Message::ToggleRdpFullscreen(instance)),
            TabAction::FitSize(instance, fit) => self.fit_pane(instance, fit),
            TabAction::Disconnect(instance) => self.update(Message::DisconnectRdp(instance)),
            TabAction::Detach(instance) => match self.session_of(instance) {
                Some(session) => self.open_in_new_window(&session.id),
                None => Task::none(),
            },
            TabAction::Close(instance) => self.close_instance(instance),
            TabAction::CloseOthers(keep) => {
                // 从后往前关：先关高下标，剩下的下标不会左移。
                let mut task = Task::none();
                for index in (0..self.tabs.len()).rev() {
                    if self.instance_at(index) != Some(keep) {
                        task = Task::batch([task, self.update(Message::CloseTab(index))]);
                    }
                }
                task
            }
        }
    }

    /// 关掉一个运行实例：在主窗口里是关标签，在独立窗口里是关那个窗口。
    /// 独立窗口的实例不在标签条里，只按下标关会静默失效。
    pub(super) fn close_instance(&mut self, instance: TabId) -> Task<Message> {
        if let Some(index) = self
            .tabs
            .tabs()
            .iter()
            .position(|tab| tab.instance == instance)
        {
            return self.update(Message::CloseTab(index));
        }

        match self.window_of_instance(instance) {
            Some(window) => self.close_window(window),
            None => Task::none(),
        }
    }

    /// 把某个会话所在窗口撑到画面正好 `fit` 那一档实际像素：RDP 因此是 1:1 呈现
    /// （不出滚动条也不缩放），终端按同样的像素重排字格。
    /// 150% 屏上「画面 1920×1080」等于把主窗口开到 1288×762（画面 1280×720 逻辑）。
    pub(super) fn fit_pane(&self, instance: TabId, fit: PaneFit) -> Task<Message> {
        let Some(kind) = self.session_of(instance).map(|session| session.kind) else {
            return Task::none();
        };
        let Some(window) = self.window_of_instance(instance) else {
            return Task::none();
        };

        let chrome = pane_chrome(self.pane_layout_of(instance), kind);

        // 先还原再改尺寸：最大化状态下 Windows 会把 `SetWindowPos` 的宽高按
        // 最大化矩形覆盖掉，不还原就等于什么都没发生（菜单就在标题栏里，
        // 用户很可能正是双击放大之后直接点的）。
        // 没最大化时这条是无操作——winit 只在 MAXIMIZED 标志真的翻转时才发
        // `ShowWindow`，不会顺手抢焦点。
        Task::batch([
            window::maximize(window, false),
            window::scale_factor(window).then(move |scale| {
                window::resize(window, window_size_for_pane(fit.size(), chrome, scale))
            }),
        ])
    }

    /// 按运行实例取标签本体：主窗口的标签条，或某个独立窗口装的那一条。
    pub(super) fn tab_of(&self, instance: TabId) -> Option<&Tab> {
        self.tabs
            .tabs()
            .iter()
            .find(|tab| tab.instance == instance)
            .or_else(|| {
                self.windows.values().find_map(|slot| match slot {
                    WindowSlot::Detached(tab) if tab.instance == instance => Some(tab),
                    _ => None,
                })
            })
    }

    /// 一个运行实例现在是否真的画在某个窗口里。
    ///
    /// 主窗口只有**活动标签**看得见；独立窗口整个窗口就是那一路会话，开着即可见。
    /// `pump_network` 按它决定要不要物化 Frame：后台标签只喂网格，省掉每 16ms
    /// 一次的整屏分配（见 `Terminal::feed_background`）。
    pub(super) fn is_instance_visible(&self, instance: TabId) -> bool {
        if self.active_terminal_instance() == Some(instance) {
            return true;
        }
        self.windows
            .values()
            .any(|slot| matches!(slot, WindowSlot::Detached(tab) if tab.instance == instance))
    }

    pub(super) fn cycle_tab(&mut self, delta: isize) -> Task<Message> {
        match self.tabs.activate_offset(delta) {
            Some(index) => self.update(Message::ActivateTab(index)),
            None => Task::none(),
        }
    }

    /// 把当前活动标签积压的帧补上：后台标签只喂网格不物化
    /// （`Terminal::feed_background`），切到它面前那一刻补一次，
    /// 否则用户看到的是冻结的旧画面，直到下一批数据才恢复。
    pub(super) fn refresh_active_frame(&mut self) {
        if let Some(instance) = self.active_terminal_instance() {
            if let Some(terminal) = self.terminals.get_mut(&instance) {
                terminal.refresh_frame();
            }
        }
    }
}
