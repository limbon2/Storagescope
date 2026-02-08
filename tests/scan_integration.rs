use std::collections::HashMap;
use std::fs;
use std::path::Path;

use storagescope::model::{NodeSummary, ScanEvent, ScanOptions};
use storagescope::scanner::run_scan_blocking;
use tempfile::TempDir;

fn collect_nodes(events: &[ScanEvent]) -> HashMap<String, NodeSummary> {
    events
        .iter()
        .filter_map(|event| {
            if let ScanEvent::NodeUpdated(node) = event {
                Some((node.path.to_string_lossy().into_owned(), node.clone()))
            } else {
                None
            }
        })
        .collect()
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[test]
fn scan_completes_with_progress() {
    let temp = TempDir::new().expect("temp dir");
    fs::create_dir(temp.path().join("docs")).expect("create docs");
    fs::write(temp.path().join("docs").join("a.txt"), vec![1_u8; 256]).expect("write a");
    fs::write(temp.path().join("b.txt"), vec![1_u8; 128]).expect("write b");

    let events = run_scan_blocking(ScanOptions {
        root: temp.path().to_path_buf(),
        one_file_system: true,
        follow_symlinks: false,
        show_hidden: true,
        show_files: true,
        max_depth: None,
    });

    assert!(
        events
            .iter()
            .any(|event| matches!(event, ScanEvent::Complete(_)))
    );
    assert!(events.iter().any(
        |event| matches!(event, ScanEvent::Progress(progress) if progress.visited_entries >= 3)
    ));
}

#[test]
fn hidden_files_obey_flag() {
    let temp = TempDir::new().expect("temp dir");
    fs::write(temp.path().join(".secret"), vec![1_u8; 10]).expect("write hidden");

    let with_hidden = run_scan_blocking(ScanOptions {
        root: temp.path().to_path_buf(),
        one_file_system: true,
        follow_symlinks: false,
        show_hidden: true,
        show_files: true,
        max_depth: None,
    });
    let nodes_with_hidden = collect_nodes(&with_hidden);
    assert!(nodes_with_hidden.contains_key(&path_key(&temp.path().join(".secret"))));

    let without_hidden = run_scan_blocking(ScanOptions {
        root: temp.path().to_path_buf(),
        one_file_system: true,
        follow_symlinks: false,
        show_hidden: false,
        show_files: true,
        max_depth: None,
    });
    let nodes_without_hidden = collect_nodes(&without_hidden);
    assert!(!nodes_without_hidden.contains_key(&path_key(&temp.path().join(".secret"))));
}

#[test]
fn max_depth_zero_keeps_children_out() {
    let temp = TempDir::new().expect("temp dir");
    fs::write(temp.path().join("root-file.bin"), vec![1_u8; 64]).expect("write file");

    let events = run_scan_blocking(ScanOptions {
        root: temp.path().to_path_buf(),
        one_file_system: true,
        follow_symlinks: false,
        show_hidden: true,
        show_files: true,
        max_depth: Some(0),
    });

    let nodes = collect_nodes(&events);
    assert!(nodes.contains_key(&path_key(temp.path())));
    assert!(!nodes.contains_key(&path_key(&temp.path().join("root-file.bin"))));
}
