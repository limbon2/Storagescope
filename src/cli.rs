use std::path::PathBuf;

use clap::{ArgAction, Parser, ValueEnum};

use crate::model::{ScanOptions, SizeMetric};

#[derive(Debug, Parser)]
#[command(name = "storagescope")]
#[command(about = "TreeSize-like terminal disk usage analyzer")]
pub struct Cli {
    /// Root path to scan
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Stay on the same filesystem/mount
    #[arg(long, default_value_t = true, action = ArgAction::Set)]
    pub one_file_system: bool,

    /// Follow symbolic links during traversal
    #[arg(long, default_value_t = false, action = ArgAction::Set)]
    pub follow_symlinks: bool,

    /// Include hidden files and directories
    #[arg(long, default_value_t = true, action = ArgAction::Set)]
    pub show_hidden: bool,

    /// Include regular files in table rows (off keeps UI fast on very large trees)
    #[arg(long, default_value_t = false, action = ArgAction::Set)]
    pub show_files: bool,

    /// Initial size metric
    #[arg(long, value_enum, default_value_t = MetricArg::Allocated)]
    pub metric: MetricArg,

    /// Maximum traversal depth (0 means root only)
    #[arg(long)]
    pub max_depth: Option<usize>,

    /// Disable delete action in the TUI
    #[arg(long, default_value_t = false)]
    pub no_delete: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum MetricArg {
    Allocated,
    Apparent,
}

impl MetricArg {
    pub fn into_metric(self) -> SizeMetric {
        match self {
            Self::Allocated => SizeMetric::Allocated,
            Self::Apparent => SizeMetric::Apparent,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub startup_root: PathBuf,
    pub scan_options: ScanOptions,
    pub initial_metric: SizeMetric,
    pub no_delete: bool,
}

impl Config {
    pub fn from_cli(cli: Cli) -> std::io::Result<Self> {
        let startup_root = std::fs::canonicalize(cli.path)?;
        Ok(Self {
            startup_root: startup_root.clone(),
            scan_options: ScanOptions {
                root: startup_root.clone(),
                one_file_system: cli.one_file_system,
                follow_symlinks: cli.follow_symlinks,
                show_hidden: cli.show_hidden,
                show_files: cli.show_files,
                max_depth: cli.max_depth,
            },
            initial_metric: cli.metric.into_metric(),
            no_delete: cli.no_delete,
        })
    }
}
