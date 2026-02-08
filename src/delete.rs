use std::fs;
use std::path::Path;

use crate::errors::AppError;

pub fn delete_target(target: &Path, scan_root: &Path) -> Result<(), AppError> {
    let target_canonical = fs::canonicalize(target).map_err(|error| AppError::Delete {
        path: target.to_path_buf(),
        reason: format!("cannot resolve path: {error}"),
    })?;

    let root_canonical = fs::canonicalize(scan_root).map_err(|error| AppError::Delete {
        path: scan_root.to_path_buf(),
        reason: format!("cannot resolve scan root: {error}"),
    })?;

    if target_canonical == root_canonical {
        return Err(AppError::Delete {
            path: target.to_path_buf(),
            reason: "refusing to delete active scan root".to_string(),
        });
    }

    let metadata = fs::symlink_metadata(target).map_err(|error| AppError::Delete {
        path: target.to_path_buf(),
        reason: format!("cannot stat target: {error}"),
    })?;

    let file_type = metadata.file_type();

    let delete_result = if file_type.is_symlink() {
        fs::remove_file(target).or_else(|_| fs::remove_dir(target))
    } else if file_type.is_dir() {
        fs::remove_dir_all(target)
    } else {
        fs::remove_file(target)
    };

    delete_result.map_err(|error| AppError::Delete {
        path: target.to_path_buf(),
        reason: format!("delete failed: {error}"),
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::delete_target;

    #[test]
    fn prevents_deleting_scan_root() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path();

        let error = delete_target(root, root).expect_err("must fail");
        assert!(
            error
                .to_string()
                .contains("refusing to delete active scan root")
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
}
