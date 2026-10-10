use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Converts Euro Truck Simulator 2 maps to OMSI 2 maps")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Writes a one-tile test map (a road and a curve) into a folder, e.g.
    /// "<OMSI 2>/maps/ets2-convert-test".
    TestMap { out: PathBuf },
}

fn main() -> Result<()> {
    match Args::parse().command {
        Command::TestMap { out } => {
            ets2_convert::test_map::build().write(&out)?;
            println!("test map written to {}", out.display());
        }
    }
    Ok(())
}
