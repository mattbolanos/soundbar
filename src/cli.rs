use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "soundbar", about = "Terminal visualizer for macOS desktop audio")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Capture system audio and render the visualizer.
    Viz,
}

