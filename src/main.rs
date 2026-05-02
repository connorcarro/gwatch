use clap::Parser;

use gwatch::{cli::Cli, runtime::run_app};

fn main() -> anyhow::Result<()> {
    run_app(Cli::parse())
}
