mod audio;
mod cli;
#[cfg(target_os = "macos")]
mod macos;
mod now_playing;
mod visualizer;

use std::env;
use std::io::{self, IsTerminal, Write};
use std::process::Command as ProcessCommand;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use anyhow::{Result, bail};
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

        let _capture = match macos::capture::Capture::start(producer, Arc::clone(&running)) {
            Ok(capture) => capture,
            Err(error) => {
                running.store(false, Ordering::Release);
                maybe_prompt_for_audio_permission();
                return Err(error).map_err(|error| error.context(capture_permission_message()));
            }
        };
        let analyzer = spawn_analyzer(consumer, Arc::clone(&state), Arc::clone(&running));
        let media = macos::now_playing::spawn_monitor(
            Arc::clone(&now_playing),
            Arc::clone(&running),
        );

        let render_result = visualizer::terminal::run(state, now_playing, Arc::clone(&running));
        running.store(false, Ordering::Release);
        let _ = analyzer.join();
        let _ = media.join();

        render_result
    }
}

fn maybe_prompt_for_audio_permission() {
    if !io::stdin().is_terminal() || !io::stderr().is_terminal() {
        return;
    }

    eprintln!(
        "\nsoundbar could not start macOS system audio capture.\n\
         The terminal app that launches soundbar needs Screen & System Audio Recording permission."
    );
    eprint!("Open System Settings to the privacy permissions page now? [Y/n] ");
    let _ = io::stderr().flush();

    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).is_err() {
        return;
    }

    let answer = answer.trim().to_ascii_lowercase();
    if answer.is_empty() || answer == "y" || answer == "yes" {
        if let Err(error) = open_privacy_settings() {
            eprintln!("Could not open System Settings automatically: {error}");
        }
    }
}

fn open_privacy_settings() -> Result<()> {
    let status = ProcessCommand::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
        .status()?;

    if status.success() {
        Ok(())
    } else {
        bail!("open exited with status {status}");
    }
}

fn capture_permission_message() -> String {
    format!(
        "failed to start macOS system audio capture. {terminal_context} Grant your launching terminal permission in System Settings > Privacy & Security > Screen & System Audio Recording, then fully quit and reopen that terminal.",
        terminal_context = terminal_context(),
    )
}

fn terminal_context() -> String {
    let term_program = env::var("TERM_PROGRAM").unwrap_or_else(|_| "unknown".to_string());
    let term = env::var("TERM").unwrap_or_else(|_| "unknown".to_string());

    format!("Detected TERM_PROGRAM={term_program}, TERM={term}.")
}
