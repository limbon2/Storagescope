use std::fmt;
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SizeMetric {
    Allocated,
    Apparent,
}

impl SizeMetric {
    pub fn toggle(self) -> Self {
        match self {
            Self::Allocated => Self::Apparent,
            Self::Apparent => Self::Allocated,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allocated => "allocated",
            Self::Apparent => "apparent",
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum FsEntryKind {
    File,
    Dir,
    Symlink,
    Other,
}

impl fmt::Display for FsEntryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::File => "file",
            Self::Dir => "dir",
            Self::Symlink => "symlink",
            Self::Other => "other",
        };
        write!(f, "{label}")
    }
}

#[derive(Debug, Clone)]
pub struct NodeSummary {
    pub path: PathBuf,
    pub kind: FsEntryKind,
    pub apparent_bytes: u64,
    pub allocated_bytes: u64,
    pub children_count: u64,
    pub is_complete: bool,
    pub last_updated: SystemTime,
}

impl NodeSummary {
    pub fn metric_bytes(&self, metric: SizeMetric) -> u64 {
        match metric {
            SizeMetric::Allocated => self.allocated_bytes,
            SizeMetric::Apparent => self.apparent_bytes,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SortMode {
    SizeDesc,
    SizeAsc,
    Name,
}

impl SortMode {
    pub fn cycle(self) -> Self {
        match self {
            Self::SizeDesc => Self::SizeAsc,
            Self::SizeAsc => Self::Name,
            Self::Name => Self::SizeDesc,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::SizeDesc => "size-desc",
            Self::SizeAsc => "size-asc",
            Self::Name => "name",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScanOptions {
    pub root: PathBuf,
    pub one_file_system: bool,
    pub follow_symlinks: bool,
    pub show_hidden: bool,
    pub show_files: bool,
    pub max_depth: Option<usize>,
}

#[derive(Debug, Clone, Default)]
pub struct ScanProgress {
    pub visited_entries: u64,
    pub warnings: u64,
    pub apparent_bytes_seen: u64,
    pub allocated_bytes_seen: u64,
}

#[derive(Debug, Clone)]
pub enum ScanEvent {
    Reset { root: PathBuf },
    NodeUpdated(NodeSummary),
    Progress(ScanProgress),
    Warning { path: PathBuf, message: String },
    Complete(ScanProgress),
    Error(String),
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metric_toggle_cycles() {
        assert_eq!(SizeMetric::Allocated.toggle(), SizeMetric::Apparent);
        assert_eq!(SizeMetric::Apparent.toggle(), SizeMetric::Allocated);
    }

    #[test]
    fn sort_mode_cycles() {
        assert_eq!(SortMode::SizeDesc.cycle(), SortMode::SizeAsc);
        assert_eq!(SortMode::SizeAsc.cycle(), SortMode::Name);
        assert_eq!(SortMode::Name.cycle(), SortMode::SizeDesc);
    }
}
