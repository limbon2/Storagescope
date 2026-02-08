use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::SystemTime;

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};

use crate::model::{FsEntryKind, NodeSummary, ScanEvent, ScanOptions, ScanProgress};
use crate::platform::{FilesystemId, allocated_size, filesystem_id};

pub struct ScanSession {
    receiver: Receiver<ScanEvent>,
    cancel: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl ScanSession {
    pub fn receiver(&self) -> &Receiver<ScanEvent> {
        &self.receiver
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn stop(&mut self) {
        self.cancel();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for ScanSession {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn start_scan(options: ScanOptions) -> ScanSession {
    // Bounded queue prevents unbounded RAM growth when scanning massive trees.
    let (tx, rx) = bounded(4096);
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_thread = Arc::clone(&cancel);

    let join = thread::spawn(move || {
        run_scan(options, tx, &cancel_for_thread);
    });

    ScanSession {
        receiver: rx,
        cancel,
        join: Some(join),
    }
}

pub fn run_scan_blocking(options: ScanOptions) -> Vec<ScanEvent> {
    let (tx, rx) = unbounded();
    let cancel = Arc::new(AtomicBool::new(false));
    run_scan(options, tx, &cancel);
    rx.try_iter().collect()
}

struct ScannerState<'a> {
    options: &'a ScanOptions,
    tx: &'a Sender<ScanEvent>,
    cancel: &'a AtomicBool,
    progress: ScanProgress,
    root_fs: Option<FilesystemId>,
    emitted_progress_entries: u64,
    visited_symlink_dirs: HashSet<PathBuf>,
}

impl<'a> ScannerState<'a> {
    fn should_cancel(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    fn send_event(&self, event: ScanEvent) {
        let _ = self.tx.send(event);
    }

    fn bump_warning(&mut self, path: &Path, message: impl Into<String>) {
        self.progress.warnings = self.progress.warnings.saturating_add(1);
        self.send_event(ScanEvent::Warning {
            path: path.to_path_buf(),
            message: message.into(),
        });
    }

    fn bump_entry(&mut self, apparent: u64, allocated: u64) {
        self.progress.visited_entries = self.progress.visited_entries.saturating_add(1);
        self.progress.apparent_bytes_seen =
            self.progress.apparent_bytes_seen.saturating_add(apparent);
        self.progress.allocated_bytes_seen =
            self.progress.allocated_bytes_seen.saturating_add(allocated);

        if self
            .progress
            .visited_entries
            .saturating_sub(self.emitted_progress_entries)
            >= 128
        {
            self.emitted_progress_entries = self.progress.visited_entries;
            self.send_event(ScanEvent::Progress(self.progress.clone()));
        }
    }

    fn emit_progress_now(&mut self) {
        self.emitted_progress_entries = self.progress.visited_entries;
        self.send_event(ScanEvent::Progress(self.progress.clone()));
    }
}

enum ScanControl {
    Continue(Option<NodeSummary>),
    Cancelled,
}

pub fn run_scan(options: ScanOptions, tx: Sender<ScanEvent>, cancel: &AtomicBool) {
    let _ = tx.send(ScanEvent::Reset {
        root: options.root.clone(),
    });

    let root_meta = match fs::symlink_metadata(&options.root) {
        Ok(meta) => meta,
        Err(error) => {
            let _ = tx.send(ScanEvent::Error(format!(
                "failed to stat {}: {error}",
                options.root.display()
            )));
            return;
        }
    };

    let root_fs = if options.one_file_system {
        filesystem_id(&options.root, &root_meta)
    } else {
        None
    };

    let mut state = ScannerState {
        options: &options,
        tx: &tx,
        cancel,
        progress: ScanProgress::default(),
        root_fs,
        emitted_progress_entries: 0,
        visited_symlink_dirs: HashSet::new(),
    };

    match scan_entry(&options.root, 0, &mut state) {
        ScanControl::Continue(Some(_)) => {
            state.emit_progress_now();
            let progress = state.progress.clone();
            state.send_event(ScanEvent::Complete(progress));
        }
        ScanControl::Continue(None) => {
            state.emit_progress_now();
            let progress = state.progress.clone();
            state.send_event(ScanEvent::Complete(progress));
        }
        ScanControl::Cancelled => {
            state.send_event(ScanEvent::Cancelled);
        }
    }
}

fn scan_entry(path: &Path, depth: usize, state: &mut ScannerState<'_>) -> ScanControl {
    if state.should_cancel() {
        return ScanControl::Cancelled;
    }

    if depth > 0 && matches!(state.options.max_depth, Some(max_depth) if depth > max_depth) {
        return ScanControl::Continue(None);
    }

    let symlink_meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(error) => {
            state.bump_warning(path, format!("cannot stat path: {error}"));
            return ScanControl::Continue(None);
        }
    };

    let symlink_type = symlink_meta.file_type();
    let is_symlink = symlink_type.is_symlink();

    if is_symlink && !state.options.follow_symlinks {
        let apparent = symlink_meta.len();
        let allocated = allocated_size(path, &symlink_meta);
        state.bump_entry(apparent, allocated);

        let summary = NodeSummary {
            path: path.to_path_buf(),
            kind: FsEntryKind::Symlink,
            apparent_bytes: apparent,
            allocated_bytes: allocated,
            children_count: 0,
            last_updated: SystemTime::now(),
        };
        if state.options.show_files {
            state.send_event(ScanEvent::NodeUpdated(summary.clone()));
        }
        return ScanControl::Continue(Some(summary));
    }

    let resolved_meta = if is_symlink {
        match fs::metadata(path) {
            Ok(meta) => meta,
            Err(error) => {
                state.bump_warning(path, format!("cannot follow symlink target: {error}"));
                return ScanControl::Continue(None);
            }
        }
    } else {
        symlink_meta
    };

    if state.options.one_file_system && depth > 0 && resolved_meta.is_dir() {
        if let Some(root_id) = state.root_fs {
            if let Some(this_id) = filesystem_id(path, &resolved_meta) {
                if root_id != this_id {
                    return ScanControl::Continue(None);
                }
            }
        }
    }

    if resolved_meta.is_file() {
        let apparent = resolved_meta.len();
        let allocated = allocated_size(path, &resolved_meta);
        state.bump_entry(apparent, allocated);
        let kind = if is_symlink {
            FsEntryKind::Symlink
        } else {
            FsEntryKind::File
        };

        let summary = NodeSummary {
            path: path.to_path_buf(),
            kind,
            apparent_bytes: apparent,
            allocated_bytes: allocated,
            children_count: 0,
            last_updated: SystemTime::now(),
        };
        if state.options.show_files {
            state.send_event(ScanEvent::NodeUpdated(summary.clone()));
        }
        return ScanControl::Continue(Some(summary));
    }

    if resolved_meta.is_dir() {
        return scan_dir(path, depth, is_symlink, &resolved_meta, state);
    }

    let apparent = resolved_meta.len();
    let allocated = allocated_size(path, &resolved_meta);
    state.bump_entry(apparent, allocated);

    let summary = NodeSummary {
        path: path.to_path_buf(),
        kind: FsEntryKind::Other,
        apparent_bytes: apparent,
        allocated_bytes: allocated,
        children_count: 0,
        last_updated: SystemTime::now(),
    };
    if state.options.show_files {
        state.send_event(ScanEvent::NodeUpdated(summary.clone()));
    }
    ScanControl::Continue(Some(summary))
}

fn scan_dir(
    path: &Path,
    depth: usize,
    is_symlink_dir: bool,
    metadata: &fs::Metadata,
    state: &mut ScannerState<'_>,
) -> ScanControl {
    if state.should_cancel() {
        return ScanControl::Cancelled;
    }

    if is_symlink_dir {
        match fs::canonicalize(path) {
            Ok(canonical) => {
                if !state.visited_symlink_dirs.insert(canonical.clone()) {
                    state.bump_warning(path, "detected symlink cycle, skipping traversal");
                    return ScanControl::Continue(None);
                }
            }
            Err(error) => {
                state.bump_warning(path, format!("cannot canonicalize symlink dir: {error}"));
                return ScanControl::Continue(None);
            }
        }
    }

    let dir_apparent = metadata.len();
    let dir_allocated = allocated_size(path, metadata);

    let read_dir = match fs::read_dir(path) {
        Ok(rd) => rd,
        Err(error) => {
            state.bump_warning(path, format!("cannot read directory: {error}"));
            let summary = NodeSummary {
                path: path.to_path_buf(),
                kind: if is_symlink_dir {
                    FsEntryKind::Symlink
                } else {
                    FsEntryKind::Dir
                },
                apparent_bytes: dir_apparent,
                allocated_bytes: dir_allocated,
                children_count: 0,
                last_updated: SystemTime::now(),
            };
            state.bump_entry(summary.apparent_bytes, summary.allocated_bytes);
            state.send_event(ScanEvent::NodeUpdated(summary.clone()));
            return ScanControl::Continue(Some(summary));
        }
    };

    let mut apparent_total = dir_apparent;
    let mut allocated_total = dir_allocated;
    let mut children_count = 0_u64;

    for entry_result in read_dir {
        if state.should_cancel() {
            return ScanControl::Cancelled;
        }

        let entry = match entry_result {
            Ok(entry) => entry,
            Err(error) => {
                state.bump_warning(path, format!("cannot read entry: {error}"));
                continue;
            }
        };

        let child_name = entry.file_name();
        if !state.options.show_hidden && is_hidden(&child_name) {
            continue;
        }

        let child_path = entry.path();
        match scan_entry(&child_path, depth + 1, state) {
            ScanControl::Continue(Some(child)) => {
                children_count = children_count.saturating_add(1);
                apparent_total = apparent_total.saturating_add(child.apparent_bytes);
                allocated_total = allocated_total.saturating_add(child.allocated_bytes);
            }
            ScanControl::Continue(None) => {}
            ScanControl::Cancelled => return ScanControl::Cancelled,
        }
    }

    let summary = NodeSummary {
        path: path.to_path_buf(),
        kind: if is_symlink_dir {
            FsEntryKind::Symlink
        } else {
            FsEntryKind::Dir
        },
        apparent_bytes: apparent_total,
        allocated_bytes: allocated_total,
        children_count,
        last_updated: SystemTime::now(),
    };

    state.bump_entry(dir_apparent, dir_allocated);
    state.send_event(ScanEvent::NodeUpdated(summary.clone()));
    ScanControl::Continue(Some(summary))
}

fn is_hidden(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::model::{ScanEvent, ScanOptions};

    #[test]
    fn scans_and_reports_nodes() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path();
        fs::create_dir(root.join("a")).expect("dir");
        fs::write(root.join("a").join("file1.bin"), vec![0_u8; 100]).expect("file1");
        fs::write(root.join("file2.bin"), vec![0_u8; 50]).expect("file2");

        let events = run_scan_blocking(ScanOptions {
            root: root.to_path_buf(),
            one_file_system: true,
            follow_symlinks: false,
            show_hidden: true,
            show_files: true,
            max_depth: None,
        });

        assert!(events.iter().any(|event| {
            matches!(
                event,
                ScanEvent::Complete(progress) if progress.visited_entries >= 3
            )
        }));

        assert!(events.iter().any(|event| {
            matches!(
                event,
                ScanEvent::NodeUpdated(node)
                    if node.path.ends_with("file2.bin") && node.apparent_bytes == 50
            )
        }));
    }
}
