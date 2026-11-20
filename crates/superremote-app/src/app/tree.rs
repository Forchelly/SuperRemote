use super::*;

impl App {
    pub(crate) fn tree(&self) -> Option<&SessionTree> {
        self.store.as_ref().map(Store::tree)
    }

    pub(crate) fn rows(&self) -> &[TreeRow] {
        &self.rows
    }

    pub(crate) fn is_selected(&self, id: &str) -> bool {
        self.selected.as_deref() == Some(id)
    }

    /// 这一行是不是拖拽的落点候选——拖拽中要比普通悬停更醒目。
    pub(crate) fn is_drop_target(&self, id: &str) -> bool {
        self.drag
            .as_ref()
            .is_some_and(|drag| drag.active && drag.over.as_deref() == Some(id))
    }

    /// 「+」标签指向的会话（如果还没被删除）。
    pub(crate) fn new_tab_session(&self) -> Option<&Session> {
        self.new_tab
            .as_ref()
            .and_then(|id| self.tree().and_then(|tree| tree.find_session(id)))
    }

    pub(super) fn sync_rows(&mut self) {
        self.rows = self
            .tree()
            .map(|tree| tree.rows(&self.search))
            .unwrap_or_default();
    }

    pub(super) fn load_draft(&mut self, id: &str) -> Draft {
        let mut draft = Draft::default();

        if let Some(session) = self.tree().and_then(|tree| tree.find_session(id)) {
            draft.name = session.name.clone();
            draft.host = session.host.clone();
            draft.port = session.port.to_string();
            draft.username = session.username.clone().unwrap_or_default();
            draft.note = session.settings.note.clone();
        } else if let Some(name) = self.tree().and_then(|tree| tree.node_name(id)) {
            draft.name = name.to_owned();
        }

        match self.store.as_ref().map(|store| store.password(id)) {
            Some(Ok(value)) => draft.password = value.unwrap_or_default(),
            Some(Err(err)) => self.status = Some(Status::error(err.to_string())),
            None => {}
        }

        match self.store.as_ref().map(|store| store.private_key(id)) {
            Some(Ok(value)) => draft.private_key = value.unwrap_or_default(),
            Some(Err(err)) => self.status = Some(Status::error(err.to_string())),
            None => {}
        }

        draft
    }

    /// 把「+」标签指向的会话设为选中，并回填详情编辑器。返回节点是否还在。
    /// 选中节点并把它的草稿/私钥/备注装进编辑器。多处入口共用，避免复制粘贴。
    pub(super) fn load_draft_into_editor(&mut self, id: &str) {
        self.selected = Some(id.to_owned());
        self.draft = self.load_draft(id);
        self.private_key_editor = text_editor::Content::with_text(&self.draft.private_key);
        self.note_editor = text_editor::Content::with_text(&self.draft.note);
    }

    pub(super) fn focus_new_tab(&mut self, id: &str) -> bool {
        if self.tree().and_then(|tree| tree.find_session(id)).is_none() {
            self.new_tab = None;
            self.go_sessions();
            return false;
        }

        self.load_draft_into_editor(id);
        true
    }

    /// 关闭「+」标签时，只有仍是空草稿（从未保存成功）的节点才删除；
    /// 已经填过地址并保存的连接要留下。
    pub(super) fn discard_blank_new_tab(&mut self, id: &str) {
        let blank = self
            .tree()
            .and_then(|tree| tree.find_session(id))
            .is_some_and(|session| session.host.is_empty());

        if blank {
            if let Some(store) = self.store.as_mut() {
                store.tree_mut().remove_node(id);
            }
            if self.selected.as_deref() == Some(id) {
                self.selected = None;
                self.draft = Draft::default();
                self.private_key_editor = text_editor::Content::with_text("");
                self.note_editor = text_editor::Content::with_text("");
            }
            self.sync_rows();
        }
    }

    pub(super) fn create_node(&mut self, kind: NewKind) {
        let Some(store) = self.store.as_mut() else {
            return;
        };

        let id = store.next_id();
        let parent = store.tree().parent_for_new(self.selected.as_deref());
        let node = match kind {
            NewKind::Directory => SessionNode::Directory(Directory {
                id: id.clone(),
                name: i18n::lang().new_folder_name().to_owned(),
                children: Vec::new(),
            }),
            NewKind::Session(protocol) => SessionNode::Session(Session {
                id: id.clone(),
                name: i18n::lang().new_session_name(protocol.label()),
                kind: protocol,
                host: String::new(),
                port: protocol.default_port(),
                username: None,
                settings: Default::default(),
                window: None,
            }),
        };

        store.tree_mut().push_node(parent.as_deref(), node);
        self.load_draft_into_editor(&id);
        self.go_sessions();
        // 草稿要回填刚创建的节点，否则名称框是空的，直接保存会报「名称不能为空」。
        self.sync_rows();
    }

    pub(super) fn apply_draft(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let draft = self.draft.clone();

        let name = draft.name.trim();
        if name.is_empty() {
            self.status = Some(Status::error(i18n::t().err_empty_name));
            return;
        }

        let is_directory = self.store.as_ref().map(|s| s.tree().is_directory(&id)) == Some(true);

        let result = if is_directory {
            self.store
                .as_mut()
                .map(|store| store.tree_mut().rename(&id, name))
        } else {
            let host = draft.host.trim();
            if host.is_empty() {
                self.status = Some(Status::error(i18n::t().err_empty_host));
                return;
            }
            let Ok(port) = draft.port.trim().parse::<u16>() else {
                self.status = Some(Status::error(i18n::t().err_bad_port));
                return;
            };
            let username =
                (!draft.username.trim().is_empty()).then(|| draft.username.trim().to_owned());

            self.store.as_mut().map(|store| {
                store.tree_mut().with_session_mut(&id, |session| {
                    session.name = name.to_owned();
                    session.host = host.to_owned();
                    session.port = port;
                    session.username = username;
                    session.settings.note = draft.note.trim().to_owned();
                })
            })
        };

        if result == Some(false) {
            self.status = Some(Status::error(i18n::t().err_node_gone));
            return;
        }

        // 口令/私钥框留空表示「不改」，要清除得走对应的「清除」按钮，避免误删。
        if !is_directory && !draft.password.is_empty() {
            if let Some(store) = self.store.as_mut() {
                if let Err(err) = store.set_password(&id, Some(&draft.password)) {
                    self.status = Some(Status::error(err.to_string()));
                    return;
                }
            }
        }

        if !is_directory && !draft.private_key.is_empty() {
            if let Some(store) = self.store.as_mut() {
                if let Err(err) = store.set_private_key(&id, Some(&draft.private_key)) {
                    self.status = Some(Status::error(err.to_string()));
                    return;
                }
            }
        }

        self.status = Some(Status::info(i18n::t().info_saved));
        self.sync_rows();
        self.save_now();
    }

    pub(super) fn clear_draft_password(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };

        if let Some(store) = self.store.as_mut() {
            match store.set_password(&id, None) {
                Ok(()) => {
                    self.draft.password.clear();
                    self.status = Some(Status::info(i18n::t().info_password_cleared));
                }
                Err(err) => self.status = Some(Status::error(err.to_string())),
            }
        }
        self.save_now();
    }

    pub(super) fn clear_draft_private_key(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };

        if let Some(store) = self.store.as_mut() {
            match store.set_private_key(&id, None) {
                Ok(()) => {
                    self.draft.private_key.clear();
                    self.private_key_editor = text_editor::Content::with_text("");
                    self.status = Some(Status::info(i18n::t().info_private_key_cleared));
                }
                Err(err) => self.status = Some(Status::error(err.to_string())),
            }
        }
        self.save_now();
    }

    pub(super) fn delete_selected(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };

        let removed = self
            .store
            .as_mut()
            .and_then(|store| store.tree_mut().remove_node(&id));

        let Some(node) = removed else {
            self.status = Some(Status::error(i18n::t().err_selection_gone));
            return;
        };

        let affected = SessionTree::subtree_session_ids(&node);
        if self
            .new_tab
            .as_ref()
            .is_some_and(|id| affected.contains(id))
        {
            self.new_tab = None;
        }
        if let Some(store) = self.store.as_mut() {
            for session_id in &affected {
                if let Err(err) = store.set_password(session_id, None) {
                    self.status = Some(Status::error(err.to_string()));
                }
                if let Err(err) = store.set_private_key(session_id, None) {
                    self.status = Some(Status::error(err.to_string()));
                }
            }
        }
        for session_id in &affected {
            // 先把运行中的连接断掉，再关标签，否则线程会挂在已释放的会话上。
            let instances: Vec<TabId> = self
                .tabs
                .tabs()
                .iter()
                .filter(|tab| tab.id() == session_id)
                .map(|tab| tab.instance)
                .collect();
            for instance in instances {
                self.forget_terminal(instance);
            }
            self.tabs.close_session(session_id);
        }

        self.selected = None;
        self.draft = Draft::default();
        self.private_key_editor = text_editor::Content::with_text("");
        self.go_sessions();
        self.sync_rows();
        self.save_now();
    }

    /// 右键菜单的动作。与点界面上的按钮走**同一段代码**（`open_session` /
    /// `delete_selected` 等），这样「右键删除」与「选中后点删除」的行为必然一致。
    pub(super) fn run_tree_action(&mut self, action: TreeAction) -> Task<Message> {
        let id = action.id().clone();

        // 「新建」先选中那一行再建：目录行 = 建到里面，连接行 = 建到根层。
        match &action {
            TreeAction::NewDirectory(_) | TreeAction::NewSession(_, _) => {
                // 空白处右键时 id 为空：新节点落到根层，而不是落到上次选中项下面。
                self.selected = (!id.is_empty()).then(|| id.clone());
                self.create_node(match &action {
                    TreeAction::NewDirectory(_) => NewKind::Directory,
                    TreeAction::NewSession(_, kind) => NewKind::Session(*kind),
                    _ => NewKind::Directory,
                });
                return Task::none();
            }
            _ => {}
        }

        match action {
            TreeAction::Open(_) => {
                self.selected = Some(id.clone());
                self.open_preferred(&id)
            }
            TreeAction::OpenInBackground(_) => self.open_session(&id, false),
            TreeAction::OpenInNewWindow(_) => self.open_in_new_window(&id),
            TreeAction::Rename(_) => {
                self.load_draft_into_editor(&id);
                // 改名只在编辑页才看得见，所以跳过去，并把光标直接送进名称框。
                self.go_sessions();
                iced::widget::operation::focus(ui::name_field_id())
            }
            TreeAction::CopyAddress(_) => {
                let address = self
                    .tree()
                    .and_then(|tree| tree.find_session(&id))
                    .map(|session| session.address());

                self.status = Some(match address {
                    Some(address) => match clipboard::write_text(&address) {
                        Ok(()) => Status::info(i18n::t().info_copied),
                        Err(err) => Status::error(err),
                    },
                    None => Status::error(i18n::t().err_node_gone),
                });
                Task::none()
            }
            TreeAction::Delete(_) => {
                self.selected = Some(id);
                self.update(Message::DeleteSelected)
            }
            TreeAction::Cut(_) => {
                self.node_clipboard = self
                    .tree()
                    .and_then(|tree| tree.find_node(&id))
                    .cloned()
                    .map(|node| NodeClipboard { node, cut: true });
                Task::none()
            }
            TreeAction::Copy(_) => {
                self.node_clipboard = self
                    .tree()
                    .and_then(|tree| tree.find_node(&id))
                    .cloned()
                    .map(|node| NodeClipboard { node, cut: false });
                Task::none()
            }
            TreeAction::Paste(_) => self.paste_node(&id),
            // 新建在上面已经处理并提前返回；这里保持 match 穷尽。
            TreeAction::NewDirectory(_) | TreeAction::NewSession(_, _) => Task::none(),
        }
    }

    /// 粘贴剪贴板里的节点。
    ///
    /// 剪切走 `move_node`（保留 id，凭据不用搬）；复制做一次深拷贝并换新 id，
    /// 同时把口令/私钥一起复制到新 id 下——否则「复制出来的连接」会在第一次
    /// 连接时才发现没有凭据。
    pub(super) fn paste_node(&mut self, target: &str) -> Task<Message> {
        let Some(clipboard) = self.node_clipboard.clone() else {
            return Task::none();
        };
        let Some(store) = self.store.as_mut() else {
            return Task::none();
        };
        let t = i18n::t();

        // 右键的如果是剪贴板自己，就按旧版语义贴到它的父级。
        let target = if target == clipboard.node.id() {
            store.tree().parent_id_of(target).unwrap_or_default()
        } else {
            target.to_owned()
        };

        let target_is_dir = !target.is_empty() && store.tree().is_directory(&target);
        let target_is_session =
            !target.is_empty() && !target_is_dir && store.tree().find_session(&target).is_some();

        if !target.is_empty() && !target_is_dir && !target_is_session {
            self.status = Some(Status::error(t.err_node_gone));
            return Task::none();
        }

        if clipboard.cut {
            let source = clipboard.node.id().clone();
            let target = if target.is_empty() {
                MoveTarget::ToRoot
            } else if target_is_dir {
                MoveTarget::IntoDirectory(target)
            } else {
                MoveTarget::NextTo(target)
            };

            if !store.tree_mut().move_node(&source, target) {
                self.status = Some(Status::error(t.err_invalid_paste));
                return Task::none();
            }

            self.node_clipboard = None;
            self.selected = Some(source);
            self.sync_rows();
            self.save_now();
            return Task::none();
        }

        let parent = if target.is_empty() {
            None
        } else if target_is_dir {
            Some(target)
        } else {
            store.tree().parent_id_of(&target)
        };

        let copy = copy_node(store, &clipboard.node);
        let new_id = copy.id().clone();
        if !store.tree_mut().push_node(parent.as_deref(), copy) {
            self.status = Some(Status::error(t.err_invalid_paste));
            return Task::none();
        }

        self.load_draft_into_editor(&new_id);
        self.go_sessions();
        self.sync_rows();
        self.save_now();
        Task::none()
    }

    /// 拖拽结束。落在 `Some(target)` 上就按落点移动；落在空白处、或根本没拖起来
    /// （只是一次点击）就作罢。整棵树的移动语义在 `SessionTree::move_node`。
    pub(super) fn finish_drag(&mut self, target: Option<SessionId>) -> Task<Message> {
        let Some(drag) = self.drag.take() else {
            // 没有进行中的拖拽：普通点击的松手也会走到这里。
            return Task::none();
        };

        if !drag.active {
            return Task::none();
        }

        let Some(target) = target else {
            return Task::none();
        };

        // 落在自己身上 = 原地不动。
        if target == drag.source {
            return Task::none();
        }

        let moved = {
            let Some(store) = self.store.as_mut() else {
                return Task::none();
            };

            // 落在目录上是搬进去，落在连接上是插到它旁边（同级）。
            let move_to = if store.tree().is_directory(&target) {
                MoveTarget::IntoDirectory(target.clone())
            } else {
                MoveTarget::NextTo(target.clone())
            };

            store.tree_mut().move_node(&drag.source, move_to)
        };

        if moved {
            self.sync_rows();
            self.save_now();
        } else {
            self.status = Some(Status::error(i18n::t().err_invalid_drop));
        }

        Task::none()
    }
}
