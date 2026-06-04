use std::io;
use std::sync::{Arc, RwLock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::audio::analyzer::VisualState;

pub fn run(state: Arc<RwLock<VisualState>>, running: Arc<AtomicBool>) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let result = run_loop(&mut terminal, state, running.clone());

    running.store(false, Ordering::Release);
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: Arc<RwLock<VisualState>>,
    running: Arc<AtomicBool>,
) -> Result<()> {
    let mut last_tick = Instant::now();

    while running.load(Ordering::Acquire) {
        let snapshot = state
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| VisualState::silence());

        terminal.draw(|frame| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(8),
                    Constraint::Length(9),
                ])
                .split(frame.area());

            let title = Paragraph::new(Line::from(vec![
                Span::styled("soundbar", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                Span::raw("  "),
                Span::styled("system audio", Style::default().fg(Color::Magenta)),
                Span::raw("  q to quit"),
            ]))
            .block(Block::default().borders(Borders::BOTTOM));
            frame.render_widget(title, chunks[0]);

            let spectrum = Paragraph::new(render_spectrum(&snapshot.spectrum, chunks[1].height))
                .style(Style::default().fg(Color::Magenta))
                .block(Block::default().borders(Borders::NONE));
            frame.render_widget(spectrum, chunks[1]);

            let waveform = Paragraph::new(render_waveform(&snapshot.waveform, chunks[2].height))
                .style(Style::default().fg(Color::Cyan))
                .block(Block::default().borders(Borders::TOP));
            frame.render_widget(waveform, chunks[2]);
        })?;

        let timeout = Duration::from_millis(16).saturating_sub(last_tick.elapsed());
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc) {
                    running.store(false, Ordering::Release);
                }
            }
        }

        last_tick = Instant::now();
    }

    Ok(())
}

fn render_spectrum(values: &[f32], height: u16) -> String {
    let height = height.max(1) as usize;
    let mut lines = Vec::with_capacity(height);

    for row in (0..height).rev() {
        let threshold = row as f32 / height as f32;
        let mut line = String::with_capacity(values.len());
        for &value in values {
            line.push(if value >= threshold { '█' } else { ' ' });
        }
        lines.push(line);
    }

    lines.join("\n")
}

fn render_waveform(values: &[f32], height: u16) -> String {
    let height = height.max(3) as usize;
    let mid = height / 2;
    let mut rows = vec![vec![' '; values.len()]; height];

    for (x, &value) in values.iter().enumerate() {
        let y = ((1.0 - (value + 1.0) * 0.5) * (height - 1) as f32).round() as usize;
        rows[y.min(height - 1)][x] = '▄';
    }

    for x in 0..values.len() {
        if rows[mid][x] == ' ' {
            rows[mid][x] = '─';
        }
    }

    rows.into_iter()
        .map(|row| row.into_iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

