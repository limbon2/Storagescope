use std::fs::Metadata;
use std::path::Path;

#[cfg(windows)]
use std::collections::hash_map::DefaultHasher;
#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::hash::{Hash, Hasher};

pub type FilesystemId = u64;

pub fn allocated_size(path: &Path, metadata: &Metadata) -> u64 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let blocks = metadata.blocks();
        if blocks > 0 {
            return blocks.saturating_mul(512);
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        let len = metadata.file_size();
        const CLUSTER: u64 = 4096;
        if len == 0 {
            return 0;
        }
        return ((len + CLUSTER - 1) / CLUSTER) * CLUSTER;
    }

    let _ = path;
    metadata.len()
}

#[cfg(unix)]
pub fn filesystem_id(_path: &Path, metadata: &Metadata) -> Option<FilesystemId> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.dev())
}

#[cfg(windows)]
pub fn filesystem_id(path: &Path, _metadata: &Metadata) -> Option<FilesystemId> {
    use std::path::Component;

    let prefix = path.components().find_map(|component| {
        if let Component::Prefix(prefix_component) = component {
            Some(prefix_component.as_os_str())
        } else {
            None
        }
    });

    prefix.map(hash_os_str)
}

#[cfg(not(any(unix, windows)))]
pub fn filesystem_id(path: &Path, metadata: &Metadata) -> Option<FilesystemId> {
    let _ = (path, metadata);
    None
}

#[cfg(windows)]
fn hash_os_str(value: &OsStr) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}
