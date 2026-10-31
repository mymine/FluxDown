//! 分享面板窗口的寻址：同一组任务只对应一个面板（一个任务集一条规范链接）。

use fluxdown_protocol::ShareDto;

/// 打开分享面板的入口目标。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PanelTarget {
    /// 从本机已完成任务创建（或取回已有的）分享。
    Tasks(Vec<String>),
    /// 查看分享页里已有的分享。
    Share(String),
}

/// 任务集的规范键：去重后按字典序排序再拼接，所以多选的点选顺序不影响窗口去重。
#[must_use]
pub fn tasks_key(task_ids: &[String]) -> String {
    let mut ids: Vec<&str> = task_ids.iter().map(String::as_str).collect();
    ids.sort_unstable();
    ids.dedup();
    format!("tasks:{}", ids.join(","))
}

/// 面板窗口键。`Share(id)` 优先归并到它的任务集键（与创建时的键一致，避免同一分享开两个窗口）；
/// 重启后任务集丢失的分享退回 `share:<id>`。
#[must_use]
pub fn panel_key(target: &PanelTarget, shares: &[ShareDto]) -> String {
    match target {
        PanelTarget::Tasks(task_ids) => tasks_key(task_ids),
        PanelTarget::Share(id) => shares
            .iter()
            .find(|share| &share.id == id)
            .filter(|share| !share.task_ids.is_empty())
            .map_or_else(|| format!("share:{id}"), |share| tasks_key(&share.task_ids)),
    }
}

#[cfg(test)]
mod tests {
    use fluxdown_protocol::ShareDto;

    use super::{PanelTarget, panel_key, tasks_key};

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn task_set_key_ignores_selection_order_and_duplicates() {
        assert_eq!(tasks_key(&ids(&["b", "a", "b"])), "tasks:a,b");
        assert_eq!(tasks_key(&ids(&["a", "b"])), tasks_key(&ids(&["b", "a"])));
    }

    #[test]
    fn existing_share_reuses_the_task_set_window() {
        let shares = vec![
            ShareDto {
                id: "s1".into(),
                task_ids: ids(&["b", "a"]),
                ..ShareDto::default()
            },
            ShareDto {
                id: "s2".into(),
                ..ShareDto::default()
            },
        ];
        assert_eq!(
            panel_key(&PanelTarget::Share("s1".into()), &shares),
            panel_key(&PanelTarget::Tasks(ids(&["a", "b"])), &shares)
        );
        assert_eq!(
            panel_key(&PanelTarget::Share("s2".into()), &shares),
            "share:s2"
        );
        assert_eq!(
            panel_key(&PanelTarget::Share("gone".into()), &shares),
            "share:gone"
        );
    }
}
