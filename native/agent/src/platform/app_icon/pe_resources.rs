//! 只读的 PE 资源目录解析（纯 Rust，边界检查，无 OS 依赖）：从 exe 文件里取出 `(类型, 名称 ID, 语言)`
//! 对应的资源数据与某资源可用的语言。
//!
//! 程序本体图标补丁用它读取 `RT_GROUP_ICON` / `RT_ICON`：
//! - 与 `LoadLibraryExW` + `LockResource` 相比，不映射模块、没有指向系统映射内存的裸指针；
//! - 读的是文件字节本身，补丁写出的临时副本换入前可以用同一段代码独立校验；
//! - 资源语言（`UpdateResourceW` 删除 / 覆盖必须给出精确语言）直接从目录里得到。
//!
//! 只支持按整数 ID 的三级目录（类型 → 名称 → 语言），这是图标资源唯一的形态；具名条目忽略。

use std::io::{Read, Seek, SeekFrom};
#[cfg(windows)]
use std::path::Path;

/// 目录项偏移 / 名称的最高位：偏移指向子目录；名称是字符串。
const HIGH_BIT: u32 = 0x8000_0000;
const DOS_HEADER_LEN: usize = 0x40;
const PE_SIGNATURE: [u8; 4] = *b"PE\0\0";
const COFF_HEADER_LEN: usize = 20;
const SECTION_HEADER_LEN: usize = 40;
const OPTIONAL_PE32: u16 = 0x10b;
const OPTIONAL_PE32_PLUS: u16 = 0x20b;
/// 数据目录里资源表的下标。
const RESOURCE_DIRECTORY_INDEX: usize = 2;
/// 资源所在节的读取上限；图标资源只有数百 KiB，超过说明不是我们认识的程序。
const MAX_RESOURCE_SECTION: usize = 64 * 1024 * 1024;
const MAX_OPTIONAL_HEADER: usize = 1024;
const MAX_SECTIONS: usize = 96;

#[derive(Debug, thiserror::Error)]
pub(super) enum PeError {
    #[error("could not read executable: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a supported PE file: {0}")]
    Malformed(&'static str),
}

/// 已载入内存的资源节及资源目录位置。
#[derive(Debug)]
pub(super) struct PeResources {
    /// 资源节的虚拟地址（RVA）；数据项里的 RVA 减去它就是 `bytes` 内偏移。
    section_rva: u32,
    /// 资源节的文件内容。
    bytes: Vec<u8>,
    /// 资源目录根在 `bytes` 内的偏移。
    root: usize,
}

fn u16_at(bytes: &[u8], offset: usize) -> Result<u16, PeError> {
    let end = offset.checked_add(2).ok_or(PeError::Malformed("offset"))?;
    let raw: [u8; 2] = bytes
        .get(offset..end)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(PeError::Malformed("truncated"))?;
    Ok(u16::from_le_bytes(raw))
}

fn u32_at(bytes: &[u8], offset: usize) -> Result<u32, PeError> {
    let end = offset.checked_add(4).ok_or(PeError::Malformed("offset"))?;
    let raw: [u8; 4] = bytes
        .get(offset..end)
        .and_then(|slice| slice.try_into().ok())
        .ok_or(PeError::Malformed("truncated"))?;
    Ok(u32::from_le_bytes(raw))
}

fn to_usize(value: u32) -> Result<usize, PeError> {
    usize::try_from(value).map_err(|_| PeError::Malformed("value too large"))
}

fn read_at<R: Read + Seek>(reader: &mut R, offset: u64, len: usize) -> Result<Vec<u8>, PeError> {
    reader.seek(SeekFrom::Start(offset))?;
    let mut buffer = vec![0u8; len];
    reader.read_exact(&mut buffer)?;
    Ok(buffer)
}

/// 一个目录项：整数 ID（`key`，高位置位 = 具名）与偏移（高位置位 = 子目录）。
struct Entry {
    key: u32,
    offset: u32,
}

impl PeResources {
    #[cfg(windows)]
    pub(super) fn open(path: &Path) -> Result<Self, PeError> {
        Self::from_reader(&mut std::fs::File::open(path)?)
    }

    pub(super) fn from_reader<R: Read + Seek>(reader: &mut R) -> Result<Self, PeError> {
        let dos = read_at(reader, 0, DOS_HEADER_LEN)?;
        if dos.get(..2) != Some(b"MZ") {
            return Err(PeError::Malformed("missing MZ header"));
        }
        let pe_offset = u64::from(u32_at(&dos, 0x3c)?);
        let coff = read_at(reader, pe_offset, 4 + COFF_HEADER_LEN)?;
        if coff.get(..4) != Some(&PE_SIGNATURE[..]) {
            return Err(PeError::Malformed("missing PE signature"));
        }
        let section_count = usize::from(u16_at(&coff, 4 + 2)?);
        let optional_len = usize::from(u16_at(&coff, 4 + 16)?);
        if section_count == 0 || section_count > MAX_SECTIONS || optional_len > MAX_OPTIONAL_HEADER
        {
            return Err(PeError::Malformed("implausible header sizes"));
        }
        let optional_offset = pe_offset + (4 + COFF_HEADER_LEN) as u64;
        let optional = read_at(reader, optional_offset, optional_len)?;
        let directories_at = match u16_at(&optional, 0)? {
            OPTIONAL_PE32 => 96,
            OPTIONAL_PE32_PLUS => 112,
            _ => return Err(PeError::Malformed("unknown optional header")),
        };
        let directory_count = to_usize(u32_at(&optional, directories_at - 4)?)?;
        if directory_count <= RESOURCE_DIRECTORY_INDEX {
            return Err(PeError::Malformed("no resource directory"));
        }
        let entry_at = directories_at + RESOURCE_DIRECTORY_INDEX * 8;
        let directory_rva = u32_at(&optional, entry_at)?;
        if directory_rva == 0 || u32_at(&optional, entry_at + 4)? == 0 {
            return Err(PeError::Malformed("no resource directory"));
        }

        let sections_offset = optional_offset + optional_len as u64;
        let sections = read_at(reader, sections_offset, section_count * SECTION_HEADER_LEN)?;
        for index in 0..section_count {
            let header = index * SECTION_HEADER_LEN;
            let virtual_size = u32_at(&sections, header + 8)?;
            let virtual_address = u32_at(&sections, header + 12)?;
            let raw_size = to_usize(u32_at(&sections, header + 16)?)?;
            let raw_pointer = u32_at(&sections, header + 20)?;
            let extent = virtual_size.max(u32::try_from(raw_size).unwrap_or(u32::MAX));
            let Some(relative) = directory_rva.checked_sub(virtual_address) else {
                continue;
            };
            if relative >= extent {
                continue;
            }
            if raw_size == 0 || raw_size > MAX_RESOURCE_SECTION {
                return Err(PeError::Malformed("unsupported resource section size"));
            }
            let bytes = read_at(reader, u64::from(raw_pointer), raw_size)?;
            return Ok(Self {
                section_rva: virtual_address,
                bytes,
                root: to_usize(relative)?,
            });
        }
        Err(PeError::Malformed(
            "resource directory is outside every section",
        ))
    }

    fn entries(&self, directory: usize) -> Result<Vec<Entry>, PeError> {
        let base = self
            .root
            .checked_add(directory)
            .ok_or(PeError::Malformed("offset"))?;
        let count = usize::from(u16_at(&self.bytes, base + 12)?)
            + usize::from(u16_at(&self.bytes, base + 14)?);
        (0..count)
            .map(|index| {
                let at = base + 16 + index * 8;
                Ok(Entry {
                    key: u32_at(&self.bytes, at)?,
                    offset: u32_at(&self.bytes, at + 4)?,
                })
            })
            .collect()
    }

    /// `directory` 下整数 ID 为 `id` 的子目录偏移；不存在返回 `None`。
    fn child_directory(&self, directory: usize, id: u16) -> Result<Option<usize>, PeError> {
        for entry in self.entries(directory)? {
            if entry.key & HIGH_BIT == 0 && entry.key == u32::from(id) {
                if entry.offset & HIGH_BIT == 0 {
                    return Err(PeError::Malformed("expected a resource subdirectory"));
                }
                return to_usize(entry.offset & !HIGH_BIT).map(Some);
            }
        }
        Ok(None)
    }

    fn language_directory(&self, kind: u16, id: u16) -> Result<Option<usize>, PeError> {
        match self.child_directory(0, kind)? {
            Some(names) => self.child_directory(names, id),
            None => Ok(None),
        }
    }

    /// `(类型, 名称 ID)` 下存在的语言；资源不存在返回空。
    pub(super) fn languages(&self, kind: u16, id: u16) -> Result<Vec<u16>, PeError> {
        let Some(directory) = self.language_directory(kind, id)? else {
            return Ok(Vec::new());
        };
        Ok(self
            .entries(directory)?
            .into_iter()
            .filter(|entry| entry.key & HIGH_BIT == 0)
            .filter_map(|entry| u16::try_from(entry.key).ok())
            .collect())
    }

    /// 精确 `(类型, 名称 ID, 语言)` 的资源数据；不存在返回 `None`。
    pub(super) fn data(&self, kind: u16, id: u16, language: u16) -> Result<Option<&[u8]>, PeError> {
        let Some(directory) = self.language_directory(kind, id)? else {
            return Ok(None);
        };
        let Some(entry) = self
            .entries(directory)?
            .into_iter()
            .find(|entry| entry.key & HIGH_BIT == 0 && entry.key == u32::from(language))
        else {
            return Ok(None);
        };
        if entry.offset & HIGH_BIT != 0 {
            return Err(PeError::Malformed("expected a resource data entry"));
        }
        let at = self
            .root
            .checked_add(to_usize(entry.offset)?)
            .ok_or(PeError::Malformed("offset"))?;
        let rva = u32_at(&self.bytes, at)?;
        let size = to_usize(u32_at(&self.bytes, at + 4)?)?;
        let start = to_usize(
            rva.checked_sub(self.section_rva)
                .ok_or(PeError::Malformed("resource data before its section"))?,
        )?;
        let end = start
            .checked_add(size)
            .ok_or(PeError::Malformed("offset"))?;
        self.bytes
            .get(start..end)
            .map(Some)
            .ok_or(PeError::Malformed("resource data outside its section"))
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    const SECTION_RVA: u32 = 0x3000;
    const RAW_POINTER: usize = 0x400;

    /// 构造只含一个 `.rsrc` 节的最小 PE32+：`resources` 为 `(类型, 名称 ID, 语言, 数据)`。
    fn build_pe(resources: &[(u16, u16, u16, Vec<u8>)]) -> Vec<u8> {
        // 资源节布局：根目录 | 类型目录们 | 名称目录们 | 数据项们 | 数据们。
        let mut kinds: Vec<u16> = resources.iter().map(|r| r.0).collect();
        kinds.sort_unstable();
        kinds.dedup();
        let names_of = |kind: u16| {
            let mut names: Vec<u16> = resources
                .iter()
                .filter(|r| r.0 == kind)
                .map(|r| r.1)
                .collect();
            names.sort_unstable();
            names.dedup();
            names
        };
        let langs_of = |kind: u16, id: u16| {
            let mut langs: Vec<u16> = resources
                .iter()
                .filter(|r| r.0 == kind && r.1 == id)
                .map(|r| r.2)
                .collect();
            langs.sort_unstable();
            langs
        };
        let directory_len = |entries: usize| 16 + entries * 8;

        let mut offset = directory_len(kinds.len());
        let mut type_dirs = Vec::new();
        for &kind in &kinds {
            type_dirs.push((kind, offset));
            offset += directory_len(names_of(kind).len());
        }
        let mut name_dirs = Vec::new();
        for &kind in &kinds {
            for id in names_of(kind) {
                name_dirs.push((kind, id, offset));
                offset += directory_len(langs_of(kind, id).len());
            }
        }
        let mut data_entries = Vec::new();
        for &(kind, id, _) in &name_dirs {
            for lang in langs_of(kind, id) {
                data_entries.push((kind, id, lang, offset));
                offset += 16;
            }
        }
        let mut blob_at = offset;
        let mut section = vec![0u8; offset];
        let put_u16 = |section: &mut Vec<u8>, at: usize, value: u16| {
            section[at..at + 2].copy_from_slice(&value.to_le_bytes());
        };
        let put_u32 = |section: &mut Vec<u8>, at: usize, value: u32| {
            section[at..at + 4].copy_from_slice(&value.to_le_bytes());
        };
        put_u16(&mut section, 14, u16::try_from(kinds.len()).expect("kinds"));
        for (index, &(kind, dir)) in type_dirs.iter().enumerate() {
            put_u32(&mut section, 16 + index * 8, u32::from(kind));
            put_u32(&mut section, 16 + index * 8 + 4, HIGH_BIT | dir as u32);
            put_u16(
                &mut section,
                dir + 14,
                u16::try_from(names_of(kind).len()).expect("names"),
            );
        }
        for &(kind, id, dir) in &name_dirs {
            let parent = type_dirs.iter().find(|t| t.0 == kind).expect("type dir").1;
            let slot = names_of(kind).iter().position(|&n| n == id).expect("slot");
            put_u32(&mut section, parent + 16 + slot * 8, u32::from(id));
            put_u32(
                &mut section,
                parent + 16 + slot * 8 + 4,
                HIGH_BIT | dir as u32,
            );
            put_u16(
                &mut section,
                dir + 14,
                u16::try_from(langs_of(kind, id).len()).expect("langs"),
            );
        }
        for &(kind, id, lang, entry) in &data_entries {
            let parent = name_dirs
                .iter()
                .find(|n| n.0 == kind && n.1 == id)
                .expect("name dir")
                .2;
            let slot = langs_of(kind, id)
                .iter()
                .position(|&l| l == lang)
                .expect("slot");
            put_u32(&mut section, parent + 16 + slot * 8, u32::from(lang));
            put_u32(&mut section, parent + 16 + slot * 8 + 4, entry as u32);
            let payload = &resources
                .iter()
                .find(|r| r.0 == kind && r.1 == id && r.2 == lang)
                .expect("resource")
                .3;
            section.resize(section.len().max(blob_at + payload.len()), 0);
            put_u32(&mut section, entry, SECTION_RVA + blob_at as u32);
            put_u32(&mut section, entry + 4, payload.len() as u32);
            section[blob_at..blob_at + payload.len()].copy_from_slice(payload);
            blob_at += payload.len();
        }

        let mut file = vec![0u8; RAW_POINTER];
        file[..2].copy_from_slice(b"MZ");
        file[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        file[0x80..0x84].copy_from_slice(&PE_SIGNATURE);
        file[0x84 + 2..0x84 + 4].copy_from_slice(&1u16.to_le_bytes()); // 节数
        file[0x84 + 16..0x84 + 18].copy_from_slice(&240u16.to_le_bytes()); // 可选头长度
        let optional = 0x84 + COFF_HEADER_LEN;
        file[optional..optional + 2].copy_from_slice(&OPTIONAL_PE32_PLUS.to_le_bytes());
        file[optional + 108..optional + 112].copy_from_slice(&16u32.to_le_bytes());
        let directory = optional + 112 + RESOURCE_DIRECTORY_INDEX * 8;
        file[directory..directory + 4].copy_from_slice(&SECTION_RVA.to_le_bytes());
        file[directory + 4..directory + 8].copy_from_slice(&(section.len() as u32).to_le_bytes());
        let header = optional + 240;
        file[header..header + 6].copy_from_slice(b".rsrc\0");
        file[header + 8..header + 12].copy_from_slice(&(section.len() as u32).to_le_bytes());
        file[header + 12..header + 16].copy_from_slice(&SECTION_RVA.to_le_bytes());
        file[header + 16..header + 20].copy_from_slice(&(section.len() as u32).to_le_bytes());
        file[header + 20..header + 24].copy_from_slice(&(RAW_POINTER as u32).to_le_bytes());
        file.extend_from_slice(&section);
        file
    }

    fn open(file: Vec<u8>) -> Result<PeResources, PeError> {
        PeResources::from_reader(&mut Cursor::new(file))
    }

    #[test]
    fn reads_data_and_languages_by_type_id_and_language() {
        let pe = open(build_pe(&[
            (14, 1, 0x0409, vec![1, 2, 3]),
            (3, 1, 0x0409, vec![9; 5]),
            (3, 2, 0x0409, vec![8; 7]),
            (3, 2, 0x0804, vec![7; 2]),
        ]))
        .expect("parse");

        assert_eq!(pe.languages(14, 1).expect("langs"), vec![0x0409]);
        assert_eq!(pe.languages(3, 2).expect("langs"), vec![0x0409, 0x0804]);
        assert_eq!(
            pe.data(14, 1, 0x0409).expect("data"),
            Some(&[1u8, 2, 3][..])
        );
        assert_eq!(pe.data(3, 2, 0x0804).expect("data"), Some(&[7u8, 7][..]));
        assert_eq!(
            pe.data(3, 2, 0x0409).expect("data").map(<[u8]>::len),
            Some(7)
        );
    }

    #[test]
    fn missing_resources_are_none_or_empty() {
        let pe = open(build_pe(&[(14, 1, 0x0409, vec![1])])).expect("parse");

        assert!(pe.languages(3, 1).expect("langs").is_empty());
        assert!(pe.languages(14, 2).expect("langs").is_empty());
        assert_eq!(pe.data(14, 1, 0x0804).expect("data"), None);
        assert_eq!(pe.data(16, 1, 0x0409).expect("data"), None);
    }

    #[test]
    fn rejects_non_pe_and_truncated_files() {
        assert!(matches!(open(Vec::new()), Err(PeError::Io(_))));
        assert!(matches!(open(vec![0; 0x100]), Err(PeError::Malformed(_))));
        let mut file = build_pe(&[(14, 1, 0x0409, vec![1, 2, 3])]);
        file.truncate(RAW_POINTER + 20);
        assert!(open(file).is_err());
    }

    #[test]
    fn data_outside_the_section_is_an_error_not_a_panic() {
        let mut file = build_pe(&[(14, 1, 0x0409, vec![1, 2, 3])]);
        // 把数据项里的大小改成远超节的长度。
        let pe = open(file.clone()).expect("parse");
        // 根、类型目录、名称目录各 16 + 8 字节（各一项），之后是第一个数据项。
        let entry = pe.root + 24 * 3;
        let size_at = RAW_POINTER + entry + 4;
        file[size_at..size_at + 4].copy_from_slice(&0x00ff_ffffu32.to_le_bytes());
        let pe = open(file).expect("parse");
        assert!(pe.data(14, 1, 0x0409).is_err());
    }
}
