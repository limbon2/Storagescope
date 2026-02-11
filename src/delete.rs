use std::fs;
use std::path::{Path, PathBuf};

use crate::errors::AppError;

pub fn delete_target(target: &Path, scan_root: &Path) -> Result<(), AppError> {
    let root_canonical = fs::canonicalize(scan_root).map_err(|error| AppError::Delete {
        path: scan_root.to_path_buf(),
        reason: format!("cannot resolve scan root: {error}"),
    })?;

    let initial_target = canonical_target_within_root(target, &root_canonical)?;
    let confirmed_target = canonical_target_within_root(target, &root_canonical)?;
    if confirmed_target != initial_target {
        return Err(AppError::Delete {
            path: target.to_path_buf(),
            reason: "target changed during safety check; refusing delete to reduce race risk"
                .to_string(),
        });
    }

    // There is still a small race window between the final check and the delete call.
    // We prefer operation-first deletes here to reduce metadata/check sequencing gaps.
    remove_target(target).map_err(|error| AppError::Delete {
        path: target.to_path_buf(),
        reason: format!("delete failed (path may have changed concurrently): {error}"),
    })
}

fn canonical_target_within_root(target: &Path, root_canonical: &Path) -> Result<PathBuf, AppError> {
    let target_canonical = fs::canonicalize(target).map_err(|error| AppError::Delete {
        path: target.to_path_buf(),
        reason: format!("cannot resolve path: {error}"),
    })?;

    if target_canonical == root_canonical {
        return Err(AppError::Delete {
            path: target.to_path_buf(),
            reason: "refusing to delete startup root".to_string(),
        });
    }

    if !target_canonical.starts_with(root_canonical) {
        return Err(AppError::Delete {
            path: target.to_path_buf(),
            reason: "refusing to delete outside startup root".to_string(),
        });
    }

    Ok(target_canonical)
}

fn remove_target(target: &Path) -> std::io::Result<()> {
    match fs::remove_file(target) {
        Ok(()) => Ok(()),
        Err(remove_file_error) => match fs::remove_dir(target) {
            Ok(()) => Ok(()),
            Err(remove_dir_error) => match fs::remove_dir_all(target) {
                Ok(()) => Ok(()),
                Err(remove_dir_all_error) => Err(std::io::Error::new(
                    remove_dir_all_error.kind(),
                    format!(
                        "remove_file: {remove_file_error}; remove_dir: {remove_dir_error}; remove_dir_all: {remove_dir_all_error}"
                    ),
                )),
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::TempDir;

    use super::delete_target;

    #[cfg(unix)]
    fn create_file_symlink(original: &Path, link: &Path) -> std::io::Result<()> {
        std::os::unix::fs::symlink(original, link)
    }

    #[cfg(windows)]
    fn create_file_symlink(original: &Path, link: &Path) -> std::io::Result<()> {
        std::os::windows::fs::symlink_file(original, link)
    }

    #[test]
    fn prevents_deleting_scan_root() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path();

        let error = delete_target(root, root).expect_err("must fail");
        assert!(
            error
                .to_string()
                .contains("refusing to delete startup root")
        );
    }

    #[test]
    fn deletes_regular_file() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path();
        let target = root.join("delete-me.txt");
        fs::write(&target, "x").expect("write");

        delete_target(&target, root).expect("delete should succeed");
        assert!(!target.exists());
    }

    #[test]
    fn rejects_target_outside_startup_root() {
        let root = TempDir::new().expect("root temp dir");
        let outside = TempDir::new().expect("outside temp dir");
        let outside_file = outside.path().join("outside.txt");
        fs::write(&outside_file, "x").expect("write outside");

        let error = delete_target(&outside_file, root.path()).expect_err("must fail");
        assert!(
            error
                .to_string()
                .contains("refusing to delete outside startup root")
        );
        assert!(outside_file.exists());
    }

    #[test]
    fn rejects_symlink_escape_outside_startup_root() {
        let root = TempDir::new().expect("root temp dir");
        let outside = TempDir::new().expect("outside temp dir");
        let outside_file = outside.path().join("outside.txt");
        fs::write(&outside_file, "x").expect("write outside");
        let link = root.path().join("escape-link");

        match create_file_symlink(&outside_file, &link) {
            Ok(()) => {}
            Err(error) => {
                #[cfg(windows)]
                {
                    if error.kind() == std::io::ErrorKind::PermissionDenied {
                        eprintln!("skipping symlink test without privilege on Windows");
                        return;
                    }
                }
                panic!("create symlink: {error}");
            }
        }

        let error = delete_target(&link, root.path()).expect_err("must fail");
        assert!(
            error
                .to_string()
                .contains("refusing to delete outside startup root")
        );
        assert!(outside_file.exists());
        assert!(link.exists());
    }
}
