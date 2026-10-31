//! 哈希与块校验。

use fluxdown_share::hashing::{manifest_root_is_consistent, parse_hash, root_of_chunks, to_hex};
use fluxdown_share::{CHUNK_SIZE, ChunkError, hash_file, hash_file_blocking, verify_chunk};
use tokio_util::sync::CancellationToken;

use crate::common::pseudo_random;

fn write_temp(data: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.bin");
    std::fs::write(&path, data).unwrap();
    (dir, path)
}

#[tokio::test]
async fn chunking_and_root_are_deterministic() {
    let len = usize::try_from(CHUNK_SIZE).unwrap() * 2 + 12_345;
    let data = pseudo_random(len, 7);
    let (_dir, path) = write_temp(&data);

    let progress = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = progress.clone();
    let a = hash_file(
        path.clone(),
        Some("renamed.bin".into()),
        move |p| sink.lock().unwrap().push(p),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let b = hash_file_blocking(&path, None, |_| {}, &CancellationToken::new()).unwrap();

    assert_eq!(a.name, "renamed.bin");
    assert_eq!(b.name, "f.bin");
    assert_eq!(a.chunk_hashes, b.chunk_hashes);
    assert_eq!(a.root_hash, b.root_hash);
    assert_eq!(a.size, len as u64);
    assert_eq!(a.chunk_hashes.len(), 3);
    assert_eq!(a.chunk_count(), 3);

    // 块哈希 = 各块字节的 BLAKE3；根 = 块哈希拼接的 BLAKE3。
    let chunk = usize::try_from(CHUNK_SIZE).unwrap();
    let expect: Vec<[u8; 32]> = data
        .chunks(chunk)
        .map(|c| *blake3::hash(c).as_bytes())
        .collect();
    assert_eq!(
        a.chunk_hashes,
        expect.iter().map(|h| to_hex(h)).collect::<Vec<_>>()
    );
    let mut concat = Vec::new();
    for h in &expect {
        concat.extend_from_slice(h);
    }
    assert_eq!(a.root_hash, to_hex(blake3::hash(&concat).as_bytes()));
    assert_eq!(
        parse_hash(&a.root_hash),
        Some(root_of_chunks(expect.iter()))
    );
    assert!(manifest_root_is_consistent(&a));

    let seen = progress.lock().unwrap();
    assert_eq!(seen.last().map(|p| p.done), Some(len as u64));
    assert!(seen.windows(2).all(|w| w[0].done < w[1].done));
}

#[tokio::test]
async fn chunk_boundaries() {
    let chunk = usize::try_from(CHUNK_SIZE).unwrap();
    for (len, chunks) in [
        (0usize, 0usize),
        (1, 1),
        (chunk - 1, 1),
        (chunk, 1),
        (chunk + 1, 2),
    ] {
        let (_dir, path) = write_temp(&pseudo_random(len, 3));
        let f = hash_file(path, None, |_| {}, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(f.chunk_hashes.len(), chunks, "len {len}");
        assert!(manifest_root_is_consistent(&f), "len {len}");
        if len == 0 {
            assert_eq!(f.root_hash, to_hex(blake3::hash(b"").as_bytes()));
            assert_eq!(f.chunk_range(0), None);
        } else {
            let last = f.chunk_count() - 1;
            assert_eq!(f.chunk_range(last).map(|(_, e)| e), Some(len as u64));
            assert_eq!(f.chunk_range(last + 1), None);
        }
    }
}

#[tokio::test]
async fn verify_chunk_accepts_good_and_rejects_bad() {
    let chunk = usize::try_from(CHUNK_SIZE).unwrap();
    let data = pseudo_random(chunk + 1000, 11);
    let (_dir, path) = write_temp(&data);
    let f = hash_file(path, None, |_| {}, CancellationToken::new())
        .await
        .unwrap();

    verify_chunk(&f, 0, &data[..chunk]).unwrap();
    verify_chunk(&f, 1, &data[chunk..]).unwrap();

    let mut flipped = data[..chunk].to_vec();
    flipped[chunk / 2] ^= 0x01;
    assert_eq!(
        verify_chunk(&f, 0, &flipped),
        Err(ChunkError::HashMismatch(0))
    );
    assert_eq!(
        verify_chunk(&f, 0, &data[..chunk - 1]),
        Err(ChunkError::BadLength {
            index: 0,
            expected: CHUNK_SIZE,
            got: CHUNK_SIZE - 1
        })
    );
    assert_eq!(verify_chunk(&f, 2, &[]), Err(ChunkError::OutOfRange(2)));

    let mut broken = f.clone();
    broken.chunk_hashes[1] = "zz".into();
    assert_eq!(
        verify_chunk(&broken, 1, &data[chunk..]),
        Err(ChunkError::MissingHash(1))
    );
    assert!(!manifest_root_is_consistent(&broken));
}

#[tokio::test]
async fn hashing_can_be_cancelled_and_detects_changes() {
    let (_dir, path) = write_temp(&pseudo_random(3 * 1024 * 1024, 5));
    let cancel = CancellationToken::new();
    cancel.cancel();
    let err = hash_file(path.clone(), None, |_| {}, cancel)
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("cancelled"));

    // 哈希期间文件被改 → 报错（回调里改文件模拟）。
    let target = path.clone();
    let err = hash_file(
        path,
        None,
        move |p| {
            if p.done == 1024 * 1024 {
                std::fs::OpenOptions::new()
                    .append(true)
                    .open(&target)
                    .unwrap()
                    .set_len(5 * 1024 * 1024)
                    .unwrap();
            }
        },
        CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert!(format!("{err:#}").contains("changed"), "{err:#}");
}
