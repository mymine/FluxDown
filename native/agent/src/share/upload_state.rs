//! 云端暂存上传的断点文件：`<数据目录>/share/uploads/<shareId>.json`。
//!
//! 只在分片 / 文件完成时写入，上传完成、取消或删除云端副本时删除。内容是 etag 与本机文件指纹：
//! etag 只有上传它的那次 PUT 响应里才有，重启后据此向云端提交 `complete`；云端另以 ListParts 为准，
//! 文件丢失时只会让 `complete` 带的是子集。

use serde::{Deserialize, Serialize};

/// 已完成的分片。`etag` 保持云端返回时的原样（含引号）。
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct DonePart {
    pub number: u32,
    pub etag: String,
}

/// 一个文件的断点：与 `upload_id` 一一对应，二者任一变化都意味着旧分片作废。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct FileProgress {
    pub index: u32,
    /// 本机文件指纹（大小 + 修改时间），与上传开始时一致才可续传。
    pub size: u64,
    pub mtime_ms: i64,
    pub upload_id: String,
    pub done_parts: Vec<DonePart>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(super) struct UploadState {
    pub files: Vec<FileProgress>,
}

impl UploadState {
    /// 取文件的断点（缺失则新建）。指纹或 `upload_id` 与记录不符 → 旧分片作废后重建。
    pub(super) fn entry(
        &mut self,
        index: u32,
        size: u64,
        mtime_ms: i64,
        upload_id: &str,
    ) -> &mut FileProgress {
        let position = self.files.iter().position(|f| f.index == index);
        let reusable = position.is_some_and(|p| {
            let file = &self.files[p];
            file.size == size && file.mtime_ms == mtime_ms && file.upload_id == upload_id
        });
        match position {
            Some(p) if reusable => &mut self.files[p],
            Some(p) => {
                self.files[p] = FileProgress {
                    index,
                    size,
                    mtime_ms,
                    upload_id: upload_id.to_owned(),
                    done_parts: Vec::new(),
                };
                &mut self.files[p]
            }
            None => {
                self.files.push(FileProgress {
                    index,
                    size,
                    mtime_ms,
                    upload_id: upload_id.to_owned(),
                    done_parts: Vec::new(),
                });
                let last = self.files.len() - 1;
                &mut self.files[last]
            }
        }
    }

    /// 记录一个刚上传完成的分片（同号覆盖）。
    pub(super) fn record(&mut self, index: u32, part: DonePart) {
        let Some(file) = self.files.iter_mut().find(|f| f.index == index) else {
            return;
        };
        match file.done_parts.iter_mut().find(|p| p.number == part.number) {
            Some(existing) => *existing = part,
            None => file.done_parts.push(part),
        }
    }

    /// 文件已知的已完成分片，按编号升序（`complete` 请求体）。
    pub(super) fn done_parts(&self, index: u32) -> Vec<DonePart> {
        let mut parts = self
            .files
            .iter()
            .find(|f| f.index == index)
            .map(|f| f.done_parts.clone())
            .unwrap_or_default();
        parts.sort_by_key(|p| p.number);
        parts
    }

    pub(super) fn remove(&mut self, index: u32) {
        self.files.retain(|f| f.index != index);
    }
}

#[cfg(test)]
mod tests {
    use super::{DonePart, UploadState};

    fn part(number: u32) -> DonePart {
        DonePart {
            number,
            etag: format!("\"e{number}\""),
        }
    }

    #[test]
    fn upload_id_or_fingerprint_change_discards_old_parts() {
        let mut state = UploadState::default();
        state.entry(0, 10, 5, "u1");
        state.record(0, part(2));
        state.record(0, part(1));
        assert_eq!(state.done_parts(0), [part(1), part(2)]);
        // 同一 uploadId 与指纹：保留。
        assert_eq!(state.entry(0, 10, 5, "u1").done_parts.len(), 2);
        // uploadId 变了：作废。
        assert!(state.entry(0, 10, 5, "u2").done_parts.is_empty());
        state.record(0, part(1));
        // 指纹变了：作废。
        assert!(state.entry(0, 11, 5, "u2").done_parts.is_empty());
        state.remove(0);
        assert!(state.done_parts(0).is_empty());
    }
}
