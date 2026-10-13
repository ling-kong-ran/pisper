//! Pisper 的会话子树删除；与普通追加分开，整份日志只提交一次。
use super::*;
use std::collections::HashSet;

pub(super) const POSITION_TYPE: &str = "pisper.session-tree-leaf";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchDeletion {
    pub deleted_entry_ids: Vec<String>,
    pub leaf_id: Option<String>,
}

impl SessionManager {
    /// Delete a message and its descendants without changing sibling branches.
    /// The caller owns the session's exclusive mutation/run lease.
    pub fn delete_branch(&mut self, entry_id: &str) -> Result<BranchDeletion, SessionManagerError> {
        let target = self
            .get_entry(entry_id)
            .ok_or_else(|| err("Session tree entry not found"))?;
        if !matches!(target, SessionEntry::Message(message) if matches!(message.message, AgentMessage::User(_) | AgentMessage::Assistant(_)))
        {
            return Err(err(
                "Only user or assistant messages can start a deleted branch",
            ));
        }
        let target_parent = target.parent_id().map(str::to_owned);
        let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
        for entry in &self.file_entries {
            if let (Some(id), Some(parent)) = (journal_id(entry), journal_parent(entry)) {
                children.entry(parent).or_default().push(id);
            }
        }
        let mut deleted = HashSet::new();
        let mut pending = vec![entry_id];
        while let Some(id) = pending.pop() {
            if deleted.insert(id.to_owned()) {
                pending.extend(children.get(id).into_iter().flatten().copied());
            }
        }
        let leaf_id = if self.leaf_id.as_ref().is_some_and(|id| deleted.contains(id)) {
            // 未知扩展与旧客户端续写的位置标记不能成为可恢复的逻辑叶子。
            // 沿原始父链回到最近已知祖先；断开的父链回到空分支。
            let mut ancestor = target_parent;
            let mut visited = HashSet::new();
            while let Some(id) = ancestor.as_deref() {
                if !visited.insert(id.to_owned()) || deleted.contains(id) {
                    return Err(err("Cannot delete a cyclic session branch"));
                }
                if self
                    .get_entry(id)
                    .is_some_and(|entry| !entry.is_branch_position())
                {
                    break;
                }
                ancestor = self
                    .file_entries
                    .iter()
                    .find(|entry| journal_id(entry) == Some(id))
                    .and_then(journal_parent)
                    .map(str::to_owned);
            }
            ancestor
        } else {
            self.leaf_id.clone()
        };
        if leaf_id.as_ref().is_some_and(|id| deleted.contains(id)) {
            return Err(err("Cannot delete a cyclic session branch"));
        }
        let mut staged = Vec::with_capacity(self.file_entries.len() + 1);
        let mut deleted_entry_ids = Vec::new();
        let mut resolved_labels = HashMap::<String, String>::new();
        let mut tail = None;
        for (index, entry) in self.file_entries.iter().enumerate() {
            if journal_id(entry).is_some_and(|id| deleted.contains(id)) {
                deleted_entry_ids.extend(journal_id(entry).map(str::to_owned));
                continue;
            }
            let mut kept = entry.clone();
            if let FileEntry::Entry(entry) = entry {
                // 标记可能写在另一分支上。保留其结构位置，清除已删除目标的引用，
                // 避免修改兄弟消息的父节点及压缩边界。
                let removed_reference = match entry {
                    SessionEntry::Label(label) => deleted.contains(&label.target_id),
                    SessionEntry::ContextEdit(edit) => deleted.contains(&edit.target_id),
                    _ => false,
                };
                if removed_reference {
                    kept = FileEntry::Entry(SessionEntry::Custom(CustomEntry {
                        custom_type: "pisper.removed-entry-reference".into(),
                        data: None,
                        id: entry.id().unwrap_or_default().into(),
                        parent_id: entry.parent_id().map(str::to_owned),
                        timestamp: entry.timestamp().into(),
                    }));
                } else if let SessionEntry::Label(label) = entry {
                    match label.label.as_ref().filter(|label| !label.is_empty()) {
                        Some(value) => {
                            resolved_labels.insert(label.target_id.clone(), value.clone());
                        }
                        None => {
                            resolved_labels.remove(&label.target_id);
                        }
                    }
                }
                tail = entry.id().map(str::to_owned).or(tail);
            }
            staged.push((Some(index), kept));
        }
        let mut used_ids: HashSet<String> = self
            .file_entries
            .iter()
            .filter_map(journal_id)
            .map(str::to_owned)
            .collect();
        // 标记是全局状态；删除承载新标记/清除标记的分支不能让存活节点恢复旧标记。
        for (id, _) in self.by_id.iter() {
            if deleted.contains(id) {
                continue;
            }
            let wanted = self.get_label(id).filter(|value| !value.is_empty());
            if wanted == resolved_labels.get(id).map(String::as_str) {
                continue;
            }
            let label_id = generate_id_excluding(&|candidate| used_ids.contains(candidate));
            used_ids.insert(label_id.clone());
            let timestamp = self
                .file_entries
                .iter()
                .rev()
                .find_map(|entry| match entry {
                    FileEntry::Entry(SessionEntry::Label(label)) if label.target_id == id => {
                        Some(label.timestamp.clone())
                    }
                    _ => None,
                })
                .unwrap_or_else(now_iso);
            staged.push((
                None,
                FileEntry::Entry(SessionEntry::Label(LabelEntry {
                    id: label_id.clone(),
                    parent_id: tail,
                    timestamp,
                    target_id: id.to_owned(),
                    label: wanted.map(str::to_owned),
                })),
            ));
            tail = Some(label_id);
        }
        let position_id = generate_id_excluding(&|candidate| used_ids.contains(candidate));
        // 与删除一起提交位置。旧版读取器仍能沿 custom.parentId 得到相同上下文；
        // 新版重建索引恢复真实叶子，不把记录用的节点当作用户的新分支。
        staged.push((
            None,
            FileEntry::Entry(SessionEntry::Custom(CustomEntry {
                custom_type: POSITION_TYPE.into(),
                data: Some(serde_json::json!({"leafId": leaf_id})),
                id: position_id,
                parent_id: leaf_id.clone(),
                timestamp: now_iso(),
            })),
        ));
        self.persist_pruned_entries(&staged)?;
        self.file_entries = staged.into_iter().map(|(_, entry)| entry).collect();
        self.build_index();
        if self.persist {
            self.flushed = true;
        }
        Ok(BranchDeletion {
            deleted_entry_ids,
            leaf_id,
        })
    }

    fn persist_pruned_entries(
        &self,
        staged: &[(Option<usize>, FileEntry)],
    ) -> Result<(), SessionManagerError> {
        if !self.persist {
            return Ok(());
        }
        let path = std::path::Path::new(
            self.session_file
                .as_deref()
                .ok_or_else(|| err("Session file missing"))?,
        );
        let original = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !self.flushed => {
                String::new()
            }
            Err(error) => return Err(error.into()),
        };
        let raw: Vec<_> = original
            .split_inclusive('\n')
            .filter_map(|line| parse_session_entry_line(line).map(|entry| (entry, line)))
            .collect();
        if (self.flushed || !original.is_empty())
            && raw
                .iter()
                .map(|(entry, _)| entry)
                .ne(self.file_entries.iter())
        {
            return Err(err("Session file changed before branch deletion"));
        }
        if original
            .lines()
            .any(|line| !line.trim().is_empty() && parse_session_entry_line(line).is_none())
        {
            return Err(err("Session file contains an invalid journal entry"));
        }
        let parent = path
            .parent()
            .ok_or_else(|| err("Session directory missing"))?;
        let temporary = parent.join(format!(".pisper-prune-{}.tmp", mint_session_id()));
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        let result = (|| -> Result<(), SessionManagerError> {
            for (index, entry) in staged {
                if let Some((_, line)) = index
                    .and_then(|index| raw.get(index))
                    .filter(|(original, _)| original == entry)
                {
                    file.write_all(line.as_bytes())?;
                    if !line.ends_with('\n') {
                        file.write_all(b"\n")?;
                    }
                } else {
                    serde_json::to_writer(&mut file, entry)
                        .map_err(|error| err(error.to_string()))?;
                    file.write_all(b"\n")?;
                }
            }
            file.sync_all()?;
            drop(file);
            std::fs::rename(&temporary, path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }
}

// 未知扩展仍可能有已知树链接；保留其原文，但不能漏删它及其已知类型的后代。
fn journal_id(entry: &FileEntry) -> Option<&str> {
    match entry {
        FileEntry::Entry(entry) => entry.id(),
        FileEntry::Unparsed(value) if value["type"] != "session" => value["id"].as_str(),
        _ => None,
    }
}

fn journal_parent(entry: &FileEntry) -> Option<&str> {
    match entry {
        FileEntry::Entry(entry) => entry.parent_id(),
        FileEntry::Unparsed(value) if value["type"] != "session" => value["parentId"].as_str(),
        _ => None,
    }
}

impl SessionEntry {
    /// Internal journal position, excluded from the application's visible tree.
    pub fn is_branch_position(&self) -> bool {
        matches!(self, Self::Custom(entry) if entry.custom_type == POSITION_TYPE)
    }
}
