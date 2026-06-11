mod audio;
mod cli;
#[cfg(target_os = "macos")]
mod macos;
mod now_playing;
mod visualizer;

use std::sync::{Arc, RwLock};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use clap::Parser;
use cli::{Cli, Command};

use crate::audio::analyzer::{VisualState, spawn_analyzer};
use crate::audio::buffer::AudioRing;

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_target(false)
        .compact()
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Viz => run_viz(),
    }
}

fn run_viz() -> Result<()> {
    #[cfg(not(target_os = "macos"))]
    {
        anyhow::bail!("soundbar captures system audio with macOS Core Audio process taps");
    }

    #[cfg(target_os = "macos")]
    {
        let running = Arc::new(AtomicBool::new(true));
        let state = Arc::new(RwLock::new(VisualState::silence()));
        let now_playing = Arc::new(RwLock::new(None));
        let (producer, consumer) = AudioRing::new(48_000 * 4).split();

        let analyzer = spawn_analyzer(consumer, Arc::clone(&state), Arc::clone(&running));
        let media = macos::now_playing::spawn_monitor(
            Arc::clone(&now_playing),
            Arc::clone(&running),
        );
        let _capture = macos::capture::Capture::start(producer, Arc::clone(&running))?;

        let render_result = visualizer::terminal::run(state, now_playing, Arc::clone(&running));
        running.store(false, Ordering::Release);
        let _ = analyzer.join();
        let _ = media.join();

        render_result
    }
}
