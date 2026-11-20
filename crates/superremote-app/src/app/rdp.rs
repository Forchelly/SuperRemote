use super::*;

#[cfg(test)]
pub(super) use crate::rdp::write_profile;
pub(super) use crate::rdp::{
    add_activex_session, clean_stale_temp_files, forget_credentials, launch_activex, session_key,
};

/// 共用宿主两次崩溃的判定窗口。在这个窗口内再崩一次，说明这条路不稳，
/// 提示用户改用独立宿主重连。
const SHARED_CRASH_WINDOW: Duration = Duration::from_secs(30);

fn read_host_title(window: isize, session: &str) -> String {
    let started = Instant::now();
    let title = rdp_host::window_title(window);
    let elapsed = started.elapsed().as_millis();
    // 只记异常与接近 50ms 超时的读取，正常轮询不刷日志
    if elapsed >= 40 || title.is_empty() {
        crate::rdp::trace(&format!(
            "host-title session={session} title-ms={elapsed} empty={}",
            title.is_empty()
        ));
    }
    title
}

impl App {
    /// 某个实例的 RDP 画面贴边布局：独立窗口 / 主窗口 / 全屏，三种装饰不同。
    pub(super) fn pane_layout_of(&self, instance: TabId) -> rdp_host::PaneLayout {
        if self.rdp_fullscreen == Some(instance) {
            return rdp_host::PaneLayout::Fullscreen;
        }
        match self.window_of_instance(instance) {
            Some(window) if matches!(self.windows.get(&window), Some(WindowSlot::Detached(_))) => {
                rdp_host::PaneLayout::Detached
            }
            _ => rdp_host::PaneLayout::Main,
        }
    }

    /// 某个实例所在窗口的原生句柄。RDP 画面要贴到那个窗口的内容区里。
    pub(super) fn hwnd_of_instance(&self, instance: TabId) -> Option<isize> {
        let window = self.window_of_instance(instance)?;
        self.hwnds.get(&window).copied()
    }

    /// 当前正在被用户看着的实例：主窗口看活动标签，独立窗口各看各的。
    pub(super) fn active_instances(&self) -> Vec<TabId> {
        let mut active = Vec::new();

        if self.screen == Screen::Remote && !self.show_home {
            if let Some(tab) = self.tabs.active_tab() {
                if tab.session.kind == SessionKind::Rdp {
                    active.push(tab.instance);
                }
            }
        }

        for slot in self.windows.values() {
            if let WindowSlot::Detached(tab) = slot {
                if tab.session.kind == SessionKind::Rdp {
                    active.push(tab.instance);
                }
            }
        }

        active
    }

    /// 「在新窗口打开」的 RDP：独立窗口**内嵌** RDP 画面（对齐旧版 `RemoteWinControl`），
    /// 打开即连接，不再把 mstsc 放养成它自己的原生窗口。
    ///
    /// mstsc 要 `SetParent` 进哪个窗口由 [`Self::hwnd_of_instance`] 决定；独立窗口的
    /// 句柄可能还没回来，`launch_rdp` 会把它挂进 [`Self::pending_rdp`] 等就绪后补启动。
    pub(super) fn launch_rdp_detached(&mut self, session: &Session) -> Task<Message> {
        let instance = self.tabs.allocate();
        self.terminal_slot(
            instance,
            INITIAL_COLS,
            INITIAL_ROWS,
            ResolvedTerminalSettings::default(),
        )
        .status = ConnStatus::Connecting;

        let (window, opened) = window::open(detached_window_settings(session.window));
        self.window_sizes
            .insert(window, remembered_size(detached_size(session.window)));
        self.windows.insert(
            window,
            WindowSlot::Detached(Tab {
                session: session.clone(),
                instance,
            }),
        );

        // 句柄可能还没回来，`launch_rdp` 会把它挂进 `pending_rdp`，等 `WindowHandle`
        // 到达后补启动；因此这里拿到的一般是 `Task::none()`。
        let launched = self.launch_rdp(instance);

        Task::batch([Self::window_ready_task(opened), launched])
    }

    pub(super) fn launch_rdp(&mut self, instance: TabId) -> Task<Message> {
        let Some(session) = self.session_of(instance) else {
            return Task::none();
        };

        let Some(iced) = self.hwnd_of_instance(instance) else {
            // 句柄任务还没回来（例如启动时自动打开）：先记下，等 WindowHandle
            // 到达后再补启动。这里如果直接报错，会把正常的启动时序当成环境故障。
            self.pending_rdp.push(instance);
            self.terminal_slot(
                instance,
                INITIAL_COLS,
                INITIAL_ROWS,
                ResolvedTerminalSettings::default(),
            )
            .status = ConnStatus::Connecting;
            return Task::none();
        };

        // 首次连接（还没有运行实例）才吸附窗口；重连保持用户当前的窗口尺寸。
        let first = !self.rdp.contains_key(&instance);
        if let Some(mut process) = self.rdp.remove(&instance) {
            process.close();
        }

        let password = self
            .store
            .as_ref()
            .and_then(|store| store.password(&session.id).ok().flatten());
        let viewport = rdp_host::pane_size(iced, self.pane_layout_of(instance));
        let defaults = self
            .store
            .as_ref()
            .map(Store::rdp_defaults)
            .unwrap_or_default();
        let settings = session.settings.rdp.resolve(&defaults);

        // 固定分辨率先把窗口吸附到「画面区正好等于该分辨率」的大小：此时控件
        // 正好等于远端桌面、没有滚动条也没有留边，画面是原尺寸。之后窗口变小
        // 只会出现滚动条（1:1、左上对齐），变大则由 `#ABABAB` 补右侧/下侧。
        // 换算与菜单里的画面档位共用 `window_size_for_pane`。
        let snap = match (
            first.then(|| settings.desktop_size.pixels()).flatten(),
            self.window_of_instance(instance),
        ) {
            (Some((width, height)), Some(window)) => {
                let chrome = pane_chrome(self.pane_layout_of(instance), session.kind);
                Task::batch([
                    // 最大化时 Windows 会忽略 `SetWindowPos` 的宽高，先还原。
                    window::maximize(window, false),
                    window::scale_factor(window).then(move |scale| {
                        window::resize(window, window_size_for_pane((width, height), chrome, scale))
                    }),
                ])
            }
            _ => Task::none(),
        };

        // RDP 直连 ActiveX 宿主：不生成 `.rdp`，因此不会触发 2026-04 的
        // 「未签名 .rdp」安全确认；证书/重定向确认由宿主在进程内关闭。
        //
        // 默认复用共用宿主；独立宿主只服务这一路。共用宿主已经不在了就
        // 新起一个，而不是让这次连接失败。
        let isolated = settings.host_mode == RdpHostMode::Isolated;
        let launched = if isolated {
            rdp::launch_activex(
                instance,
                &session,
                password.as_deref(),
                Some(viewport),
                &settings,
                true,
            )
        } else if let Some(host) = self.shared_host.clone() {
            rdp::add_activex_session(
                &host,
                instance,
                &session,
                password.as_deref(),
                Some(viewport),
                &settings,
            )
            .or_else(|_| {
                rdp::launch_activex(
                    instance,
                    &session,
                    password.as_deref(),
                    Some(viewport),
                    &settings,
                    false,
                )
            })
        } else {
            rdp::launch_activex(
                instance,
                &session,
                password.as_deref(),
                Some(viewport),
                &settings,
                false,
            )
        };

        match launched {
            Ok(process) => {
                if !isolated {
                    self.shared_host = Some(process.host.clone());
                }
                self.rdp.insert(instance, process);
                self.terminal_slot(
                    instance,
                    INITIAL_COLS,
                    INITIAL_ROWS,
                    ResolvedTerminalSettings::default(),
                )
                .status = ConnStatus::Connecting;
                self.status = None;
                self.sync_rdp_layout();

                // 「全屏」是分辨率设置里的一个档位：选中它意味着这条连接
                // **打开即全屏**，而不是连上之后再让用户去点全屏按钮。
                if settings.desktop_size.starts_fullscreen() {
                    return self.enter_rdp_fullscreen(instance);
                }
                return snap;
            }
            Err(err) => {
                self.terminal_slot(
                    instance,
                    INITIAL_COLS,
                    INITIAL_ROWS,
                    ResolvedTerminalSettings::default(),
                )
                .status = ConnStatus::Failed(err.clone());
                self.status = Some(Status::error(err));
            }
        }

        Task::none()
    }

    /// 把某一路 RDP 铺满整个窗口。与手动点「全屏」走的是同一段状态迁移。
    pub(super) fn enter_rdp_fullscreen(&mut self, instance: TabId) -> Task<Message> {
        if !self.rdp.contains_key(&instance) {
            return Task::none();
        }

        self.rdp_fullscreen = Some(instance);
        self.screen = Screen::Remote;
        self.show_home = false;
        self.sync_rdp_layout();
        Self::set_window_fullscreen(true)
    }

    pub(super) fn disconnect_rdp(&mut self, instance: TabId) {
        if let Some(mut process) = self.rdp.remove(&instance) {
            process.close();
        }
        if let Some(terminal) = self.terminals.get_mut(&instance) {
            terminal.status = ConnStatus::Closed(None);
        }
        self.sync_rdp_layout();
    }

    /// 内置宿主标题的格式：`SUPERREMOTE|<session>|<state>|<message>`。
    ///
    /// 只接受属于本会话的标题；旧宿主（标题没有会话段）在升级期间仍放行。
    pub(super) fn activex_status<'a>(title: &'a str, session: &str) -> Option<(&'a str, &'a str)> {
        let host = superremote_win::contract::parse_title(title)?;
        match host.session {
            Some(found) if found != session => None,
            _ => Some((host.state, host.message)),
        }
    }

    /// 把 mstsc 的会话窗口发现、挂载、贴边、退出收敛在一条轮询里。
    ///
    /// 不在启动时同步等待窗口：mstsc 建窗口的时机由它自己决定，阻塞 UI 线程
    /// 会让整个界面在连接阶段冻住。150ms 的轮询只在这段时间存在，连上后放慢。
    pub(super) fn poll_rdp(&mut self) -> Task<Message> {
        if self.rdp.is_empty() {
            return Task::none();
        }

        // 每个实例待的窗口可能不同（主窗口的标签 vs 独立窗口），句柄与画面区都要
        // 按实例取。先算好：循环里再借 `&self` 会和 `self.rdp` 的可变借用打架。
        let hosts: HashMap<TabId, isize> = self
            .rdp
            .keys()
            .filter_map(|instance| Some((*instance, self.hwnd_of_instance(*instance)?)))
            .collect();
        let layouts: HashMap<TabId, rdp_host::PaneLayout> = self
            .rdp
            .keys()
            .map(|instance| (*instance, self.pane_layout_of(*instance)))
            .collect();
        let active = self.active_instances();
        // 探活必须打真实宿主窗口；`hosts` 里的 Iced 父窗口无法反映 ActiveX 线程状态
        let trace_windows: Vec<isize> = self
            .rdp
            .values()
            .filter_map(|process| process.window)
            .collect();
        crate::rdp::trace_tick(&trace_windows);
        // 父窗口 → 要同步带动的子窗口（窗口过程用，见 `set_sizing_children`）。
        // 只收**当前显示着**的实例：窗口过程和拖边抓帧都取表里第一项，
        // 多标签时不能把隐藏标签的旧窗口当成本页画面。
        let mut sized: HashMap<isize, Vec<(isize, rdp_host::PaneLayout)>> = HashMap::new();

        let mut updates = Vec::new();
        let mut finished = Vec::new();
        let mut left_fullscreen = false;

        // 共用宿主一旦退出，挂在它下面的每一路会话都跟着断。先把每个宿主只判
        // 一次存活，别对同一个已死进程重复 `try_wait`，也让「影响 N 路会话」
        // 用的是崩溃瞬间的会话数。
        let mut session_counts: HashMap<u32, usize> = HashMap::new();
        for process in self.rdp.values() {
            *session_counts.entry(process.host.pid).or_default() += 1;
        }
        let mut dead_hosts: HashMap<u32, (u32, usize, bool)> = HashMap::new();
        for (pid, count) in &session_counts {
            let Some(process) = self.rdp.values().find(|process| process.host.pid == *pid) else {
                continue;
            };
            if let Some(code) = process.host.try_wait() {
                dead_hosts.insert(*pid, (code, *count, process.host.isolated));
            }
        }
        let now = Instant::now();
        let repeated_crash = self
            .shared_crash_at
            .is_some_and(|at| now.duration_since(at) < SHARED_CRASH_WINDOW);

        for (instance, process) in self.rdp.iter_mut() {
            let session = rdp::session_key(*instance);
            if let Some((code, count, isolated)) = dead_hosts.get(&process.host.pid).copied() {
                // 不自动批量重连：用户点标签上的「重新连接」逐路恢复。提示里写清
                // 是哪个进程带走了几路会话，30 秒内二次崩溃则直接建议换独立宿主。
                let message = if isolated {
                    format!("独立 RDP 宿主进程退出（退出码 {code}）")
                } else if repeated_crash {
                    format!(
                        "共享宿主进程退出（退出码 {code}），影响 {count} 路会话；\
                         30 秒内二次崩溃，建议改用「独立宿主」重连"
                    )
                } else {
                    format!("共享宿主进程退出（退出码 {code}），影响 {count} 路会话")
                };
                updates.push((*instance, ConnStatus::Failed(message)));
                finished.push(*instance);
                left_fullscreen |= self.rdp_fullscreen == Some(*instance);
                continue;
            }

            // 「在新窗口打开」的 RDP 不内嵌：窗口是 mstsc 自己的，
            // 我们只负责观察它还活着没有，不参与贴边与显隐。
            if !process.embed {
                if !process.live {
                    process.live = true;
                    updates.push((*instance, ConnStatus::Live));
                }
                continue;
            }

            let Some(iced) = hosts.get(instance).copied() else {
                continue;
            };
            let layout = layouts
                .get(instance)
                .copied()
                .unwrap_or(rdp_host::PaneLayout::Main);
            let viewport = rdp_host::pane_rect(iced, layout);

            if process.window.is_none() {
                let found = if process.activex {
                    rdp_host::find_activex_window(process.host.pid, &session)
                } else {
                    rdp_host::find_session_window(process.host.pid)
                };
                match found {
                    Some(window) => {
                        if let Err(err) = rdp_host::embed(window, iced, viewport) {
                            updates.push((*instance, ConnStatus::Failed(err.clone())));
                            self.status = Some(Status::error(err));
                            finished.push(*instance);
                            continue;
                        }
                        process.window = Some(window);
                        process.rect = Some(viewport);
                        if process.activex {
                            // 宿主自己连上之前不能亮出黑屏；标题里就是状态。
                            rdp_host::hide(window);
                            let title = read_host_title(window, &session);
                            if let Some(status) = Self::activex_status(&title, &session) {
                                if status.0 == "connected" {
                                    process.live = true;
                                    updates.push((*instance, ConnStatus::Live));
                                } else if status.0 == "failed" {
                                    updates
                                        .push((*instance, ConnStatus::Failed(status.1.to_owned())));
                                    finished.push(*instance);
                                }
                            }
                        } else {
                            // mstsc 是隐藏启动的：它不会自己变成可见，等我们挂好之后再
                            // 由下面的 `show` 亮出来。`live` 不能等 `IsWindowVisible`
                            // （那永远不会成立），否则会话一直停在「连接中」。
                            process.live = true;
                            updates.push((*instance, ConnStatus::Live));
                        }
                    }
                    None => continue,
                }
            }

            let Some(window) = process.window else {
                continue;
            };

            if !rdp_host::is_window(window) {
                updates.push((*instance, ConnStatus::Closed(None)));
                finished.push(*instance);
                left_fullscreen |= self.rdp_fullscreen == Some(*instance);
                continue;
            }

            if process.activex {
                let title = read_host_title(window, &session);
                if let Some(status) = Self::activex_status(&title, &session) {
                    if status.0 == "failed" {
                        crate::rdp::trace(&format!(
                            "host-failed session={session} msg={}",
                            status.1
                        ));
                        updates.push((*instance, ConnStatus::Failed(status.1.to_owned())));
                        finished.push(*instance);
                        left_fullscreen |= self.rdp_fullscreen == Some(*instance);
                        continue;
                    }
                    if status.0 == "connected" && !process.live {
                        process.live = true;
                        updates.push((*instance, ConnStatus::Live));
                    }
                }
            } else if rdp_host::is_window_visible(window) && !process.live {
                process.live = true;
                updates.push((*instance, ConnStatus::Live));
            }

            if active.contains(instance) {
                sized.entry(iced).or_default().push((window, layout));
                // 坐标没变就不重复调 SetWindowPos；显隐也只在状态变化时切。
                // 页面切换时 `sync_rdp_layout` 会先隐藏，轮询这里不再每 150ms
                // 对同一个子窗口发一遍跨进程调用。
                if process.rect != Some(viewport) {
                    rdp_host::reposition(window, viewport);
                    process.rect = Some(viewport);
                }
                if process.live && !process.shown {
                    rdp_host::show(window);
                    process.shown = true;
                }
            } else if process.shown {
                rdp_host::hide(window);
                process.shown = false;
                process.rect = None;
            }
        }

        let just_became_live = updates.iter().any(|(instance, status)| {
            matches!(status, ConnStatus::Live) && active.contains(instance)
        });

        for instance in finished {
            if let Some(mut process) = self.rdp.remove(&instance) {
                process.close();
            }
        }

        // 共用宿主引用要跟着失效：下一路连接会重新起一个，而不是往一个已经
        // 退出的进程发 `add`。崩溃时间用来判定 30 秒内的二次崩溃。
        if let Some(host) = self.shared_host.clone() {
            if dead_hosts.contains_key(&host.pid) {
                self.shared_host = None;
                self.shared_crash_at = Some(now);
            }
        }

        for (instance, status) in updates {
            if let Some(terminal) = self.terminals.get_mut(&instance) {
                terminal.status = status;
            }
        }

        if just_became_live {
            crate::rdp::trace(&format!("host-live sessions={}", self.rdp.len()));
            self.focus_active_rdp();
        }

        // 名单逐个父窗口重给：没有活动子窗口的父窗口也要重给空名单，
        // 否则切回主页/设置后窗口过程还按过期名单摆位。
        let mut parents: Vec<isize> = hosts.values().copied().collect();
        parents.sort_unstable();
        parents.dedup();
        for parent in parents {
            rdp_host::set_sizing_children(parent, sized.remove(&parent).unwrap_or_default());
        }

        if left_fullscreen {
            self.rdp_fullscreen = None;
            self.sync_rdp_layout();
            return Self::set_window_fullscreen(false);
        }

        Task::none()
    }

    /// 切到 RDP 标签、或远程画面第一次出现时，把键盘焦点交给会话窗口。
    ///
    /// 只在这两个时机调用：每次贴边都 `SetFocus` 会把用户正在输入的工具条
    /// / 搜索框抢走。
    pub(super) fn focus_active_rdp(&self) {
        let Some(tab) = self.active_tab() else {
            return;
        };
        if tab.session.kind != SessionKind::Rdp {
            return;
        }

        if let Some(process) = self.rdp.get(&tab.instance) {
            if process.live {
                if let Some(window) = process.window {
                    rdp_host::focus(window);
                }
            }
        }
    }

    /// 按当前页面/标签/全屏状态，把会话窗口贴到内容区或藏起来。
    pub(super) fn sync_rdp_layout(&mut self) {
        if self.rdp.is_empty() {
            return;
        }

        // 句柄与画面区按实例取：主窗口的标签和独立窗口各贴各的内容区。
        let hosts: HashMap<TabId, isize> = self
            .rdp
            .keys()
            .filter_map(|instance| Some((*instance, self.hwnd_of_instance(*instance)?)))
            .collect();
        let layouts: HashMap<TabId, rdp_host::PaneLayout> = self
            .rdp
            .keys()
            .map(|instance| (*instance, self.pane_layout_of(*instance)))
            .collect();
        let active = self.active_instances();
        // 父窗口 → 要同步带动的子窗口。交给 `rdp_host` 的窗口过程，
        // 因为拖拽期间系统模态缩放循环不跑 `update`（见 `set_sizing_children`）。
        // 只收当前显示着的实例，理由同 `poll_rdp`。
        let mut sized: HashMap<isize, Vec<(isize, rdp_host::PaneLayout)>> = HashMap::new();

        for (instance, process) in self.rdp.iter_mut() {
            if !process.embed {
                continue;
            }

            let Some(window) = process.window else {
                continue;
            };
            if !rdp_host::is_window(window) {
                continue;
            }
            let Some(iced) = hosts.get(instance).copied() else {
                continue;
            };

            let layout = layouts
                .get(instance)
                .copied()
                .unwrap_or(rdp_host::PaneLayout::Main);
            let viewport = rdp_host::pane_rect(iced, layout);
            if active.contains(instance) {
                sized.entry(iced).or_default().push((window, layout));
                if process.rect != Some(viewport) {
                    rdp_host::reposition(window, viewport);
                    process.rect = Some(viewport);
                }
                if process.live && !process.shown {
                    rdp_host::show(window);
                    process.shown = true;
                }
            } else if process.shown {
                rdp_host::hide(window);
                process.shown = false;
                process.rect = None;
            }
        }

        // 逐个父窗口重给名单（含空名单），理由同 `poll_rdp`。
        let mut parents: Vec<isize> = hosts.values().copied().collect();
        parents.sort_unstable();
        parents.dedup();
        for parent in parents {
            rdp_host::set_sizing_children(parent, sized.remove(&parent).unwrap_or_default());
        }
    }

    pub(super) fn forget_rdp_credentials(&mut self, instance: TabId) {
        let Some(session) = self.session_of(instance) else {
            return;
        };

        // 只有系统真的删掉了才报「已清除」——上一版无条件报成功，
        // 用户看到绿色提示、系统里却还留着旧口令，下次连接会用一个早已作废的凭据。
        self.status = Some(match rdp::forget_credentials(&session.host) {
            Ok(()) => Status::info(i18n::lang().rdp_credentials_cleared(&session.host)),
            Err(err) => Status::error(err),
        });
    }
}
