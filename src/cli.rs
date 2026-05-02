use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "gwatch", version, about = "Realtime Git working-tree diff TUI")]
pub struct Cli {
    /// Directory inside the Git repo to watch.
    #[arg(long, value_name = "PATH")]
    pub repo: Option<PathBuf>,
}
