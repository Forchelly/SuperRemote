//! 多标签宿主逻辑。
//!
//! 对应旧版 `RemoteTabControl` 的职责：一条会话打开成一个标签，激活与关闭的规则
//! 都与 UI 无关，因此放在可单测的领域层。

use crate::session::{Session, SessionId};

/// 标签实例编号。
///
/// 同一个会话条目允许开出多个标签（SSH/Telnet 可以开多份），所以不能拿
/// [`SessionId`] 当运行时连接的键，必须给每个标签一个独立的实例编号。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(u64);

impl TabId {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// 一个已打开的会话。
///
/// 这里保存的是会话快照，与旧版 `GetLastSetting()` 在连接瞬间取快照的约定一致：
/// 之后编辑树里的条目不会影响已经在跑的标签。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    pub session: Session,
    pub instance: TabId,
}

impl Tab {
    pub fn id(&self) -> &SessionId {
        &self.session.id
    }

    pub fn title(&self) -> &str {
        &self.session.name
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TabManager {
    tabs: Vec<Tab>,
    active: Option<usize>,
    next_instance: u64,
}

impl TabManager {
    /// 打开会话：允许重复实例的协议新建标签，否则激活已存在的标签。
    ///
    /// 返回实际承载会话的运行实例——调用方要靠它把连接接到正确的终端上，
    /// 不能事后靠「刚插进去的肯定是活动标签」去猜。
    pub fn open(&mut self, session: Session) -> TabId {
        if !session.kind.allows_multiple_instances() {
            if let Some(index) = self.index_of(&session.id) {
                self.active = Some(index);
                return self.tabs[index].instance;
            }
        }

        self.next_instance += 1;
        let instance = TabId(self.next_instance);
        self.tabs.push(Tab { session, instance });
        self.active = Some(self.tabs.len() - 1);
        instance
    }

    /// 只分配一个运行实例编号，不进标签条。
    ///
    /// 独立窗口用它：一个窗口只装一个会话，不需要标签条来承载，
    /// 但终端/连接的运行时仍然按 [`TabId`] 索引，所以编号要从同一个池子里出。
    pub fn allocate(&mut self) -> TabId {
        self.next_instance += 1;
        TabId(self.next_instance)
    }

    /// 在后台打开：新标签入栈但**不抢焦点**，用户可以继续看当前这一页。
    ///
    /// 第一次打开（当前没有活动标签）时例外——那时没有「保持原状」可言，
    /// 不给焦点会让用户停在一个空壳视图上。返回承载会话的运行实例。
    pub fn open_in_background(&mut self, session: Session) -> TabId {
        let previous = self.active;
        let instance = self.open(session);

        if previous.is_some() {
            self.active = previous;
        }

        instance
    }

    pub fn activate(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = Some(index);
        }
    }

    /// 相对当前位置切换标签，`delta` 为正表示向右。越界时按标签数环绕。
    ///
    /// 返回切换后的下标；一个标签都没有时返回 `None`，调用方据此跳过整条动作。
    /// 环绕是刻意的：`Ctrl+Tab` 在最后一个标签上应该回到第一个，而不是什么都不做。
    pub fn activate_offset(&mut self, delta: isize) -> Option<usize> {
        let len = self.tabs.len();
        if len == 0 {
            return None;
        }

        let current = self.active.unwrap_or(0) as isize;
        let next = (current + delta).rem_euclid(len as isize) as usize;
        self.active = Some(next);
        Some(next)
    }

    /// 用树上的最新会话快照替换某个运行实例。重连前调用，设置/地址才会生效。
    pub fn replace_session(&mut self, instance: TabId, session: Session) {
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.instance == instance) {
            tab.session = session;
        }
    }

    pub fn close(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }

        self.tabs.remove(index);

        self.active = match self.active {
            None => None,
            Some(_) if self.tabs.is_empty() => None,
            Some(active) if active > index => Some(active - 1),
            Some(active) if active != index => Some(active),
            Some(_) => Some(index.min(self.tabs.len() - 1)),
        };
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.active.and_then(|index| self.tabs.get(index))
    }

    pub fn active_index(&self) -> Option<usize> {
        self.active
    }

    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    pub fn len(&self) -> usize {
        self.tabs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    /// 某个会话条目当前是否有打开的标签，用于左侧树的选中高亮。
    pub fn has_session(&self, id: &str) -> bool {
        self.tabs.iter().any(|tab| tab.id() == id)
    }

    /// 会话被删除时连带关掉它的标签，返回关掉了几个。
    pub fn close_session(&mut self, id: &str) -> usize {
        let before = self.tabs.len();
        self.tabs.retain(|tab| tab.id() != id);
        let removed = before - self.tabs.len();

        if removed > 0 {
            self.active = if self.tabs.is_empty() {
                None
            } else {
                Some(self.active.unwrap_or(0).min(self.tabs.len() - 1))
            };
        }
        removed
    }

    fn index_of(&self, id: &str) -> Option<usize> {
        self.tabs.iter().position(|tab| tab.id() == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::SessionKind;

    fn session(id: &str, kind: SessionKind) -> Session {
        Session {
            id: id.into(),
            name: id.into(),
            kind,
            host: "10.0.0.1".into(),
            port: kind.default_port(),
            username: None,
            settings: Default::default(),
            window: None,
        }
    }

    #[test]
    fn ssh_can_be_opened_twice_and_last_one_is_active() {
        let mut tabs = TabManager::default();
        tabs.open(session("web", SessionKind::Ssh));
        tabs.open(session("web", SessionKind::Ssh));

        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs.active_index(), Some(1));
    }

    #[test]
    fn rdp_reopens_focus_the_existing_tab() {
        let mut tabs = TabManager::default();
        tabs.open(session("jump", SessionKind::Rdp));
        tabs.open(session("other", SessionKind::Ssh));
        tabs.open(session("jump", SessionKind::Rdp));

        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs.active_index(), Some(0));
        assert_eq!(tabs.active_tab().map(Tab::title), Some("jump"));
    }

    #[test]
    fn a_background_open_does_not_steal_the_focus() {
        let mut tabs = TabManager::default();
        tabs.open(session("first", SessionKind::Ssh));
        let background = tabs.open_in_background(session("later", SessionKind::Ssh));

        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs.active_index(), Some(0), "后台打开不该抢焦点");
        assert_eq!(background.get(), 2, "返回的必须是新标签的运行实例");

        // RDP 不允许重复实例：后台打开已存在的那条，也不能把焦点从当前标签拽走。
        tabs.open(session("jump", SessionKind::Rdp));
        let reused = tabs.open_in_background(session("jump", SessionKind::Rdp));

        assert_eq!(tabs.len(), 3);
        assert_eq!(
            tabs.active_index(),
            Some(2),
            "前台打开的 jump 应仍是活动标签"
        );
        assert_eq!(reused.get(), 3, "已存在的 RDP 复用原实例");
    }

    #[test]
    fn the_first_background_open_still_becomes_the_active_tab() {
        let mut tabs = TabManager::default();
        let instance = tabs.open_in_background(session("only", SessionKind::Ssh));

        assert_eq!(
            tabs.active_index(),
            Some(0),
            "没有可保留的焦点时，后台打开也要落位，否则用户停在空壳视图上"
        );
        assert_eq!(tabs.active_tab().map(|tab| tab.instance), Some(instance));
    }

    #[test]
    fn closing_an_earlier_tab_keeps_the_same_session_active() {
        let mut tabs = TabManager::default();
        tabs.open(session("a", SessionKind::Ssh));
        tabs.open(session("b", SessionKind::Ssh));
        tabs.open(session("c", SessionKind::Ssh));
        tabs.activate(2);
        tabs.close(0);

        assert_eq!(tabs.active_tab().map(Tab::title), Some("c"));
        assert_eq!(tabs.active_index(), Some(1));
    }

    #[test]
    fn closing_the_active_tab_selects_its_right_neighbor_then_the_last_one() {
        let mut tabs = TabManager::default();
        for name in ["a", "b", "c"] {
            tabs.open(session(name, SessionKind::Ssh));
        }
        tabs.activate(0);
        tabs.close(0);
        assert_eq!(tabs.active_tab().map(Tab::title), Some("b"));

        tabs.activate(1);
        tabs.close(1);
        assert_eq!(tabs.active_tab().map(Tab::title), Some("b"));

        tabs.close(0);
        assert!(tabs.is_empty());
        assert_eq!(tabs.active_tab(), None);
    }

    #[test]
    fn out_of_range_indices_are_ignored() {
        let mut tabs = TabManager::default();
        tabs.open(session("a", SessionKind::Ssh));
        tabs.close(7);
        tabs.activate(7);

        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs.active_index(), Some(0));
    }

    #[test]
    fn cycling_wraps_in_both_directions() {
        let mut tabs = TabManager::default();
        for name in ["a", "b", "c"] {
            tabs.open(session(name, SessionKind::Ssh));
        }
        tabs.activate(0);

        assert_eq!(tabs.activate_offset(1), Some(1));
        assert_eq!(tabs.activate_offset(1), Some(2));
        assert_eq!(tabs.activate_offset(1), Some(0), "末位之后应回到首位");
        assert_eq!(tabs.activate_offset(-1), Some(2), "首位之前应回到末位");

        // 一个标签时来回切都是它自己，不能越界也不能返回 None。
        let mut single = TabManager::default();
        single.open(session("only", SessionKind::Ssh));
        assert_eq!(single.activate_offset(1), Some(0));
        assert_eq!(single.activate_offset(-1), Some(0));
    }

    #[test]
    fn cycling_an_empty_manager_is_a_no_op() {
        let mut tabs = TabManager::default();
        assert_eq!(tabs.activate_offset(1), None);
        assert_eq!(tabs.active_index(), None);
    }

    #[test]
    fn has_session_tracks_closed_tabs() {
        let mut tabs = TabManager::default();
        tabs.open(session("a", SessionKind::Ssh));
        assert!(tabs.has_session("a"));
        tabs.close(0);
        assert!(!tabs.has_session("a"));
    }

    #[test]
    fn close_session_removes_every_tab_of_that_session_and_reactivates() {
        let mut tabs = TabManager::default();
        tabs.open(session("a", SessionKind::Ssh));
        tabs.open(session("b", SessionKind::Ssh));
        tabs.open(session("a", SessionKind::Ssh));
        tabs.activate(1);

        assert_eq!(tabs.close_session("a"), 2);
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs.active_tab().map(Tab::title), Some("b"));

        assert_eq!(tabs.close_session("b"), 1);
        assert_eq!(tabs.active_tab(), None);
        assert_eq!(tabs.close_session("missing"), 0);
    }
}
