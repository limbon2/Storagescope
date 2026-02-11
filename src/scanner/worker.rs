use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

use crossbeam_channel::{Receiver, Sender, TrySendError, bounded, unbounded};

use crate::model::{FsEntryKind, NodeSummary, ScanEvent, ScanOptions, ScanProgress};
use crate::platform::{FilesystemId, allocated_size, filesystem_id};

const EVENT_QUEUE_CAPACITY: usize = 8192;
const PROGRESS_EMIT_EVERY: u64 = 512;
const CRITICAL_EVENT_RETRY_DELAY: Duration = Duration::from_millis(1);

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
        self.cancel.store(true, Ordering::Release);
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
    let (tx, rx) = bounded(EVENT_QUEUE_CAPACITY);
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
    pending_progress: Option<ScanProgress>,
    dropped_node_updates: u64,
    deferred_progress_updates: u64,
    coalesced_progress_updates: u64,
    backpressure_warning_emitted: bool,
    channel_closed: bool,
}

impl<'a> ScannerState<'a> {
    fn should_cancel(&self) -> bool {
        self.channel_closed || self.cancel.load(Ordering::Acquire)
    }

    fn send_critical(&mut self, mut event: ScanEvent) {
        if self.channel_closed {
            return;
        }

        self.try_flush_pending_progress();
        loop {
            match self.tx.try_send(event) {
                Ok(()) => return,
                Err(TrySendError::Full(returned)) => {
                    event = returned;
                    thread::sleep(CRITICAL_EVENT_RETRY_DELAY);
                    self.try_flush_pending_progress();
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.channel_closed = true;
                    return;
                }
            }
        }
    }

    fn send_noncritical(&mut self, event: ScanEvent) {
        if self.channel_closed {
            return;
        }

        self.try_flush_pending_progress();
        match self.tx.try_send(event) {
            Ok(()) => {}
            Err(TrySendError::Full(event)) => self.handle_noncritical_backpressure(event),
            Err(TrySendError::Disconnected(_)) => {
                self.channel_closed = true;
            }
        }
    }

    fn send_node_update(&mut self, summary: NodeSummary) {
        self.send_noncritical(ScanEvent::NodeUpdated(summary));
    }

    fn try_flush_pending_progress(&mut self) {
        let Some(progress) = self.pending_progress.take() else {
            return;
        };

        match self.tx.try_send(ScanEvent::Progress(progress)) {
            Ok(()) => {}
            Err(TrySendError::Full(ScanEvent::Progress(progress))) => {
                self.pending_progress = Some(progress);
            }
            Err(TrySendError::Full(_)) => unreachable!("only progress events are buffered"),
            Err(TrySendError::Disconnected(_)) => {
                self.channel_closed = true;
            }
        }
    }

    fn handle_noncritical_backpressure(&mut self, event: ScanEvent) {
        match event {
            ScanEvent::NodeUpdated(_) => {
                self.dropped_node_updates = self.dropped_node_updates.saturating_add(1);
            }
            ScanEvent::Progress(progress) => {
                self.deferred_progress_updates = self.deferred_progress_updates.saturating_add(1);
                if self.pending_progress.is_some() {
                    self.coalesced_progress_updates =
                        self.coalesced_progress_updates.saturating_add(1);
                }
                self.pending_progress = Some(progress);
            }
            _ => unreachable!("only non-critical events should use this path"),
        }
    }

    fn emit_backpressure_warning_if_needed(&mut self) {
        if self.backpressure_warning_emitted || self.channel_closed {
            return;
        }

        if self.dropped_node_updates == 0 && self.deferred_progress_updates == 0 {
            return;
        }

        self.backpressure_warning_emitted = true;
        self.progress.warnings = self.progress.warnings.saturating_add(1);
        let message = format!(
            "scanner backpressure: dropped {} node updates; deferred {} progress updates ({} coalesced)",
            self.dropped_node_updates,
            self.deferred_progress_updates,
            self.coalesced_progress_updates
        );

        self.send_critical(ScanEvent::Warning {
            path: self.options.root.clone(),
            message,
        });
    }

    fn bump_warning(&mut self, path: &Path, message: impl Into<String>) {
        self.progress.warnings = self.progress.warnings.saturating_add(1);
        self.send_critical(ScanEvent::Warning {
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
            >= PROGRESS_EMIT_EVERY
        {
            self.emitted_progress_entries = self.progress.visited_entries;
            self.send_noncritical(ScanEvent::Progress(self.progress.clone()));
        }
    }

    fn emit_progress_now(&mut self) {
        self.emitted_progress_entries = self.progress.visited_entries;
        self.send_noncritical(ScanEvent::Progress(self.progress.clone()));
    }
}

enum ScanControl {
    Continue(Option<NodeSummary>),
    Cancelled,
}

fn send_critical_event(tx: &Sender<ScanEvent>, mut event: ScanEvent) -> bool {
    loop {
        match tx.try_send(event) {
            Ok(()) => return true,
            Err(TrySendError::Full(returned)) => {
                event = returned;
                thread::sleep(CRITICAL_EVENT_RETRY_DELAY);
            }
            Err(TrySendError::Disconnected(_)) => return false,
        }
    }
}

pub fn run_scan(options: ScanOptions, tx: Sender<ScanEvent>, cancel: &AtomicBool) {
    if !send_critical_event(
        &tx,
        ScanEvent::Reset {
            root: options.root.clone(),
        },
    ) {
        return;
    }

    let root_meta = match fs::symlink_metadata(&options.root) {
        Ok(meta) => meta,
        Err(error) => {
            let _ = send_critical_event(
                &tx,
                ScanEvent::Error(format!(
                    "failed to stat {}: {error}",
                    options.root.display()
                )),
            );
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
        pending_progress: None,
        dropped_node_updates: 0,
        deferred_progress_updates: 0,
        coalesced_progress_updates: 0,
        backpressure_warning_emitted: false,
        channel_closed: false,
    };

    match scan_entry(&options.root, 0, &mut state) {
        ScanControl::Continue(_) => {
            state.emit_backpressure_warning_if_needed();
            state.emit_progress_now();
            let progress = state.progress.clone();
            state.send_critical(ScanEvent::Complete(progress));
        }
        ScanControl::Cancelled => {
            state.emit_backpressure_warning_if_needed();
            state.send_critical(ScanEvent::Cancelled);
        }
    }
}

fn scan_entry(path: &Path, depth: usize, state: &mut ScannerState<'_>) -> ScanControl {
    if state.should_cancel() {
        return ScanControl::Cancelled;
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
        let summary = summarize_non_dir(
            path,
            FsEntryKind::Symlink,
            &symlink_meta,
            state.options.show_files,
            state,
        );
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

    if state.options.one_file_system
        && depth > 0
        && resolved_meta.is_dir()
        && let Some(root_id) = state.root_fs
        && let Some(this_id) = filesystem_id(path, &resolved_meta)
        && root_id != this_id
    {
        return ScanControl::Continue(None);
    }

    if resolved_meta.is_file() {
        let kind = kind_from_non_dir(&resolved_meta, is_symlink);
        let summary =
            summarize_non_dir(path, kind, &resolved_meta, state.options.show_files, state);
        return ScanControl::Continue(Some(summary));
    }

    if resolved_meta.is_dir() {
        return scan_dir(path, depth, is_symlink, &resolved_meta, true, true, state);
    }

    let kind = kind_from_non_dir(&resolved_meta, is_symlink);
    let summary = summarize_non_dir(path, kind, &resolved_meta, state.options.show_files, state);
    ScanControl::Continue(Some(summary))
}

struct PendingDir {
    path: PathBuf,
    is_symlink_dir: bool,
    metadata: fs::Metadata,
    emitted_initial: bool,
    emit_updates: bool,
}

fn scan_dir(
    path: &Path,
    depth: usize,
    is_symlink_dir: bool,
    metadata: &fs::Metadata,
    emit_initial: bool,
    emit_updates: bool,
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

    if emit_initial && emit_updates {
        let initial_summary = NodeSummary {
            path: path.to_path_buf(),
            kind: if is_symlink_dir {
                FsEntryKind::Symlink
            } else {
                FsEntryKind::Dir
            },
            apparent_bytes: dir_apparent,
            allocated_bytes: dir_allocated,
            children_count: 0,
            is_complete: false,
            last_updated: SystemTime::now(),
        };
        state.send_node_update(initial_summary);
    }

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
                is_complete: true,
                last_updated: SystemTime::now(),
            };
            state.bump_entry(summary.apparent_bytes, summary.allocated_bytes);
            if emit_updates {
                state.send_node_update(summary.clone());
            }
            return ScanControl::Continue(Some(summary));
        }
    };

    let mut apparent_total = dir_apparent;
    let mut allocated_total = dir_allocated;
    let mut children_count = 0_u64;
    let mut pending_dirs: Vec<PendingDir> = Vec::new();

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
        let child_path = entry.path();
        let child_depth = depth + 1;
        let show_hidden = state.options.show_hidden || !is_hidden(&child_name);
        let within_display_depth =
            !matches!(state.options.max_depth, Some(max_depth) if child_depth > max_depth);
        let child_emit_updates = emit_updates && show_hidden && within_display_depth;

        let child_symlink_meta = match fs::symlink_metadata(&child_path) {
            Ok(meta) => meta,
            Err(error) => {
                state.bump_warning(&child_path, format!("cannot stat path: {error}"));
                continue;
            }
        };

        let child_is_symlink = child_symlink_meta.file_type().is_symlink();
        if child_is_symlink && !state.options.follow_symlinks {
            let child = summarize_non_dir(
                &child_path,
                FsEntryKind::Symlink,
                &child_symlink_meta,
                child_emit_updates && state.options.show_files,
                state,
            );
            children_count = children_count.saturating_add(1);
            apparent_total = apparent_total.saturating_add(child.apparent_bytes);
            allocated_total = allocated_total.saturating_add(child.allocated_bytes);
            continue;
        }

        let child_resolved_meta = if child_is_symlink {
            match fs::metadata(&child_path) {
                Ok(meta) => meta,
                Err(error) => {
                    state.bump_warning(
                        &child_path,
                        format!("cannot follow symlink target: {error}"),
                    );
                    continue;
                }
            }
        } else {
            child_symlink_meta
        };

        if state.options.one_file_system
            && child_resolved_meta.is_dir()
            && let Some(root_id) = state.root_fs
            && let Some(this_id) = filesystem_id(&child_path, &child_resolved_meta)
            && root_id != this_id
        {
            continue;
        }

        if child_resolved_meta.is_dir() {
            let emitted_initial = if child_is_symlink || !child_emit_updates {
                false
            } else {
                state.send_node_update(NodeSummary {
                    path: child_path.clone(),
                    kind: FsEntryKind::Dir,
                    apparent_bytes: child_resolved_meta.len(),
                    allocated_bytes: allocated_size(&child_path, &child_resolved_meta),
                    children_count: 0,
                    is_complete: false,
                    last_updated: SystemTime::now(),
                });
                true
            };

            pending_dirs.push(PendingDir {
                path: child_path,
                is_symlink_dir: child_is_symlink,
                metadata: child_resolved_meta,
                emitted_initial,
                emit_updates: child_emit_updates,
            });
            continue;
        }

        let kind = kind_from_non_dir(&child_resolved_meta, child_is_symlink);
        let child = summarize_non_dir(
            &child_path,
            kind,
            &child_resolved_meta,
            child_emit_updates && state.options.show_files,
            state,
        );
        children_count = children_count.saturating_add(1);
        apparent_total = apparent_total.saturating_add(child.apparent_bytes);
        allocated_total = allocated_total.saturating_add(child.allocated_bytes);
    }

    for pending in pending_dirs {
        match scan_dir(
            &pending.path,
            depth + 1,
            pending.is_symlink_dir,
            &pending.metadata,
            !pending.emitted_initial,
            pending.emit_updates,
            state,
        ) {
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
        is_complete: true,
        last_updated: SystemTime::now(),
    };

    state.bump_entry(dir_apparent, dir_allocated);
    if emit_updates {
        state.send_node_update(summary.clone());
    }
    ScanControl::Continue(Some(summary))
}

fn summarize_non_dir(
    path: &Path,
    kind: FsEntryKind,
    metadata: &fs::Metadata,
    emit_node_update: bool,
    state: &mut ScannerState<'_>,
) -> NodeSummary {
    let apparent = metadata.len();
    let allocated = allocated_size(path, metadata);
    state.bump_entry(apparent, allocated);

    let summary = NodeSummary {
        path: path.to_path_buf(),
        kind,
        apparent_bytes: apparent,
        allocated_bytes: allocated,
        children_count: 0,
        is_complete: true,
        last_updated: SystemTime::now(),
    };
    if emit_node_update {
        state.send_node_update(summary.clone());
    }

    summary
}

fn kind_from_non_dir(metadata: &fs::Metadata, is_symlink: bool) -> FsEntryKind {
    if is_symlink {
        FsEntryKind::Symlink
    } else if metadata.is_file() {
        FsEntryKind::File
    } else {
        FsEntryKind::Other
    }
}

#[cfg(unix)]
fn is_hidden(name: &std::ffi::OsStr) -> bool {
    use std::os::unix::ffi::OsStrExt;
    matches!(name.as_bytes().first(), Some(b'.'))
}

#[cfg(not(unix))]
fn is_hidden(name: &std::ffi::OsStr) -> bool {
    name.to_string_lossy().starts_with('.')
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    use crossbeam_channel::{RecvTimeoutError, bounded};
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

    #[test]
    fn parent_totals_include_hidden_and_depth_suppressed_entries() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path();
        fs::create_dir(root.join("visible-dir")).expect("create visible-dir");
        fs::write(root.join("visible-dir").join("deep.bin"), vec![0_u8; 128]).expect("deep file");
        fs::write(root.join(".hidden.bin"), vec![0_u8; 64]).expect("hidden file");
        fs::write(root.join("visible.bin"), vec![0_u8; 32]).expect("visible file");

        let full_events = run_scan_blocking(ScanOptions {
            root: root.to_path_buf(),
            one_file_system: true,
            follow_symlinks: false,
            show_hidden: true,
            show_files: true,
            max_depth: None,
        });

        let constrained_events = run_scan_blocking(ScanOptions {
            root: root.to_path_buf(),
            one_file_system: true,
            follow_symlinks: false,
            show_hidden: false,
            show_files: true,
            max_depth: Some(0),
        });

        let root_full = full_events
            .iter()
            .filter_map(|event| match event {
                ScanEvent::NodeUpdated(node) if node.path == root && node.is_complete => {
                    Some(node.clone())
                }
                _ => None,
            })
            .next_back()
            .expect("root summary from full scan");

        let root_constrained = constrained_events
            .iter()
            .filter_map(|event| match event {
                ScanEvent::NodeUpdated(node) if node.path == root && node.is_complete => {
                    Some(node.clone())
                }
                _ => None,
            })
            .next_back()
            .expect("root summary from constrained scan");

        assert_eq!(root_constrained.apparent_bytes, root_full.apparent_bytes);
        assert_eq!(root_constrained.allocated_bytes, root_full.allocated_bytes);
        assert!(!constrained_events.iter().any(|event| {
            matches!(
                event,
                ScanEvent::NodeUpdated(node) if node.path.ends_with(".hidden.bin")
            )
        }));
        assert!(!constrained_events.iter().any(|event| {
            matches!(
                event,
                ScanEvent::NodeUpdated(node) if node.path.ends_with("visible-dir")
            )
        }));
        assert!(!constrained_events.iter().any(|event| {
            matches!(
                event,
                ScanEvent::NodeUpdated(node) if node.path.ends_with("deep.bin")
            )
        }));
    }

    #[test]
    fn scanner_completes_under_backpressure_without_deadlock() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path();
        for index in 0..4_000 {
            fs::write(root.join(format!("file-{index:04}.bin")), [0_u8; 1]).expect("write file");
        }

        let options = ScanOptions {
            root: root.to_path_buf(),
            one_file_system: true,
            follow_symlinks: false,
            show_hidden: true,
            show_files: true,
            max_depth: None,
        };

        let (tx, rx) = bounded(4);
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_for_scan = Arc::clone(&cancel);

        let scanner = thread::spawn(move || {
            run_scan(options, tx, cancel_for_scan.as_ref());
        });

        let consumer = thread::spawn(move || {
            let mut saw_complete = false;
            let mut saw_backpressure_warning = false;
            let deadline = Instant::now() + Duration::from_secs(20);

            thread::sleep(Duration::from_millis(100));

            while Instant::now() < deadline {
                match rx.recv_timeout(Duration::from_millis(100)) {
                    Ok(event) => {
                        if let ScanEvent::Warning { message, .. } = &event
                            && message.contains("scanner backpressure")
                        {
                            saw_backpressure_warning = true;
                        }

                        match event {
                            ScanEvent::Complete(_) => {
                                saw_complete = true;
                                break;
                            }
                            ScanEvent::Cancelled | ScanEvent::Error(_) => break,
                            _ => {
                                thread::sleep(Duration::from_millis(2));
                            }
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }

            (saw_complete, saw_backpressure_warning)
        });

        scanner.join().expect("scanner join");
        let (saw_complete, saw_backpressure_warning) = consumer.join().expect("consumer join");

        assert!(saw_complete, "scan should complete under backpressure");
        assert!(
            saw_backpressure_warning,
            "scanner should emit a warning when non-critical events are dropped/coalesced"
        );
    }

    #[test]
    fn cancellation_is_observed_promptly() {
        let temp = TempDir::new().expect("temp dir");
        let root = temp.path();
        for dir_idx in 0..64 {
            let dir = root.join(format!("dir-{dir_idx:02}"));
            fs::create_dir(&dir).expect("create dir");
            for file_idx in 0..128 {
                fs::write(dir.join(format!("file-{file_idx:03}.bin")), [0_u8; 1])
                    .expect("write file");
            }
        }

        let options = ScanOptions {
            root: root.to_path_buf(),
            one_file_system: true,
            follow_symlinks: false,
            show_hidden: true,
            show_files: true,
            max_depth: None,
        };

        let (tx, rx) = bounded(64);
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_for_scan = Arc::clone(&cancel);
        let scanner = thread::spawn(move || {
            run_scan(options, tx, cancel_for_scan.as_ref());
        });

        let reset = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("receive reset event");
        assert!(matches!(reset, ScanEvent::Reset { .. }));

        cancel.store(true, Ordering::Release);

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut saw_cancelled = false;
        while Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(ScanEvent::Cancelled) => {
                    saw_cancelled = true;
                    break;
                }
                Ok(ScanEvent::Complete(_)) => break,
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }

        scanner.join().expect("scanner join");
        assert!(
            saw_cancelled,
            "scanner should emit Cancelled after cancel signal"
        );
    }
}
