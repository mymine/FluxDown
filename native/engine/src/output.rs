use std::io;
use std::path::Path;

/// Ensure that an output directory exists, preserving the original IO kind
/// while adding the directory and operation to the error message.
pub(crate) async fn ensure_dir(dir: &Path) -> io::Result<()> {
    if dir.as_os_str().is_empty() {
        return Ok(());
    }
    tokio::fs::create_dir_all(dir)
        .await
        .map_err(|source| with_output_context("create output directory", dir, source))
}

/// Ensure the parent directory for an output path exists.
pub(crate) async fn ensure_parent(path: &Path) -> io::Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    ensure_dir(parent).await
}

#[cfg(any(feature = "bt", test))]
/// Synchronous counterpart used by blocking protocol completion paths.
pub(crate) fn ensure_dir_sync(dir: &Path) -> io::Result<()> {
    if dir.as_os_str().is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)
        .map_err(|source| with_output_context("create output directory", dir, source))
}

fn with_output_context(operation: &str, path: &Path, source: io::Error) -> io::Error {
    io::Error::new(
        source.kind(),
        format!("{operation} '{}': {source}", path.display()),
    )
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::{ensure_dir, ensure_dir_sync, ensure_parent};

    #[tokio::test]
    async fn creates_missing_nested_output_directory() {
        let root =
            std::env::temp_dir().join(format!("fluxdown-output-dir-{}", uuid::Uuid::new_v4()));
        let target = root.join("中文").join("nested");
        let file = target.join("payload.bin");

        ensure_parent(&file).await.expect("create parent");
        assert!(target.is_dir());

        if let Err(error) = tokio::fs::remove_dir_all(root).await
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("best-effort test directory cleanup: {error}");
        }
    }

    #[test]
    fn sync_creation_is_idempotent() {
        let root =
            std::env::temp_dir().join(format!("fluxdown-output-dir-sync-{}", uuid::Uuid::new_v4()));
        ensure_dir_sync(&root).expect("create directory");
        ensure_dir_sync(&root).expect("existing directory is fine");
        assert!(root.is_dir());

        if let Err(error) = std::fs::remove_dir_all(root)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("best-effort test directory cleanup: {error}");
        }
    }

    #[tokio::test]
    async fn reports_the_directory_when_creation_fails() {
        let root =
            std::env::temp_dir().join(format!("fluxdown-output-file-{}", uuid::Uuid::new_v4()));
        std::fs::write(&root, b"occupied").expect("create blocking file");
        let child = root.join("child");

        let error = ensure_dir(&child).await.expect_err("must fail");
        let message = error.to_string();
        assert!(message.contains("create output directory"));
        assert!(message.contains(child.to_string_lossy().as_ref()));

        if let Err(error) = std::fs::remove_file(root) {
            assert_eq!(
                error.kind(),
                std::io::ErrorKind::NotFound,
                "clean test path: {error}"
            );
        }
    }
}
