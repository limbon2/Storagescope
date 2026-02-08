use anyhow::Context;
use clap::Parser;

use storagescope::app::App;
use storagescope::cli::{Cli, Config};

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let config = Config::from_cli(cli).context("failed to build configuration")?;

    let mut app = App::new(config);
    app.run().context("application runtime failed")?;

    Ok(())
}
