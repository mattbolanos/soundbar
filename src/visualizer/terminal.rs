use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Terminal;

use crate::audio::analyzer::VisualState;
use crate::now_playing::NowPlaying;

pub fn run(
    state: Arc<RwLock<VisualState>>,
    now_playing: Arc<RwLock<Option<NowPlaying>>>,
    running: Arc<AtomicBool>,
) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let result = run_loop(&mut terminal, state, now_playing, running.clone());

    running.store(false, Ordering::Release);
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: Arc<RwLock<VisualState>>,
    now_playing: Arc<RwLock<Option<NowPlaying>>>,
    running: Arc<AtomicBool>,
) -> Result<()> {
    let mut last_tick = Instant::now();

    while running.load(Ordering::Acquire) {
        let snapshot = state
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_else(|_| VisualState::silence());
        let media = now_playing.read().ok().and_then(|guard| guard.clone());

        terminal.draw(|frame| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),
                    Constraint::Min(8),
                ])
                .split(frame.area());

            let title = Paragraph::new(header_line(media.as_ref(), chunks[0].width))
                .block(Block::default().borders(Borders::BOTTOM));
            frame.render_widget(title, chunks[0]);

            let spectrum = Paragraph::new(render_spectrum(
                &snapshot.spectrum,
                chunks[1].height,
                chunks[1].width,
            ))
            .block(Block::default().borders(Borders::NONE));
            frame.render_widget(spectrum, chunks[1]);
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

fn header_line(now_playing: Option<&NowPlaying>, width: u16) -> Line<'static> {
    let mut spans = vec![
        Span::styled(
            "soundbar",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("system audio", Style::default().fg(Color::Magenta)),
    ];

    if let Some(now_playing) = now_playing {
        let label = media_label(now_playing);
        if !label.is_empty() {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(label, Style::default().fg(Color::Gray)));
        }
    }

    let used_width = spans.iter().map(|span| span.content.chars().count()).sum::<usize>();
    if width as usize > used_width + 12 {
        spans.push(Span::raw("  "));
        spans.push(Span::styled("q to quit", Style::default().fg(Color::DarkGray)));
    }

    Line::from(spans)
}

fn media_label(now_playing: &NowPlaying) -> String {
    let status = if now_playing.playing { ">" } else { "||" };
    let track = match (&now_playing.title, &now_playing.artist) {
        (Some(title), Some(artist)) => format!("{title} - {artist}"),
        (Some(title), None) => title.clone(),
        (None, Some(artist)) => artist.clone(),
        (None, None) => now_playing
            .album
            .clone()
            .or_else(|| now_playing.app.clone())
            .unwrap_or_default(),
    };

    if track.is_empty() {
        String::new()
    } else {
        format!("{status} {track}")
    }
}

fn render_spectrum(values: &[f32], height: u16, width: u16) -> Text<'static> {
    let height = height.max(1) as usize;
    let width = width.max(1) as usize;
    let values = resample_spectrum(values, width);
    let mut lines = Vec::with_capacity(height);

    for row in (0..height).rev() {
        let threshold = row as f32 / height.saturating_sub(1).max(1) as f32;
        let mut spans = Vec::with_capacity(width);
        for (column, &value) in values.iter().enumerate() {
            let glyph = if value >= threshold { "█" } else { " " };
            let color = magma_color(column as f32 / width.saturating_sub(1).max(1) as f32);
            spans.push(Span::styled(glyph, Style::default().fg(color)));
        }
        lines.push(Line::from(spans));
    }

    Text::from(lines)
}

fn resample_spectrum(values: &[f32], width: usize) -> Vec<f32> {
    if values.is_empty() {
        return vec![0.0; width];
    }

    let mut resampled = vec![0.0; width];
    let source_max = values.len().saturating_sub(1) as f32;
    let target_max = width.saturating_sub(1).max(1) as f32;

    for (index, slot) in resampled.iter_mut().enumerate() {
        let source = index as f32 / target_max * source_max;
        let left = source.floor() as usize;
        let right = (left + 1).min(values.len() - 1);
        let mix = source - left as f32;
        let value = values[left] + (values[right] - values[left]) * smoothstep(mix);
        let edge_fade = edge_fade(index, width);
        *slot = (value * edge_fade).clamp(0.0, 1.0);
    }

    soften_columns(&mut resampled);
    balance_valleys(&mut resampled);
    resampled
}

fn soften_columns(values: &mut [f32]) {
    if values.len() < 3 {
        return;
    }

    let previous = values.to_vec();
    for index in 0..values.len() {
        let center = previous[index];
        let left = index
            .checked_sub(1)
            .and_then(|left| previous.get(left))
            .copied()
            .unwrap_or(center);
        let right = previous.get(index + 1).copied().unwrap_or(center);
        let bridged = center.max((left + right) * 0.38);
        values[index] = (bridged * 0.72 + center * 0.28).clamp(0.0, 1.0);
    }
}

fn balance_valleys(values: &mut [f32]) {
    if values.len() < 5 {
        return;
    }

    let previous = values.to_vec();
    let radius = (values.len() / 10).clamp(6, 24);

    for index in 0..values.len() {
        let center = previous[index];
        let left_peak = previous[index.saturating_sub(radius)..=index]
            .iter()
            .copied()
            .fold(0.0_f32, f32::max);
        let right_peak = previous[index..=(index + radius).min(previous.len() - 1)]
            .iter()
            .copied()
            .fold(0.0_f32, f32::max);
        let local_floor = left_peak.min(right_peak);

        if local_floor <= 0.08 {
            continue;
        }

        let bridge = (local_floor * 0.54).min(center + 0.24);
        values[index] = center
            .max(center + (bridge - center).max(0.0) * 0.42)
            .clamp(0.0, 1.0);
    }
}

fn smoothstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * (3.0 - 2.0 * value)
}

fn edge_fade(index: usize, width: usize) -> f32 {
    let position = index as f32 / width.saturating_sub(1).max(1) as f32;
    let left = (position / 0.025).clamp(0.0, 1.0);
    let right = ((1.0 - position) / 0.025).clamp(0.0, 1.0);
    smoothstep(left.min(right))
}

fn magma_color(position: f32) -> Color {
    const STOPS: &[(f32, (u8, u8, u8))] = &[
        (0.0, (42, 0, 70)),
        (0.28, (120, 18, 104)),
        (0.5, (201, 45, 75)),
        (0.72, (247, 111, 50)),
        (1.0, (255, 221, 102)),
    ];

    let position = position.clamp(0.0, 1.0);
    let ((start, from), (end, to)) = STOPS
        .windows(2)
        .find_map(|window| {
            let from = window[0];
            let to = window[1];
            (position >= from.0 && position <= to.0).then_some((from, to))
        })
        .unwrap_or((STOPS[0], STOPS[STOPS.len() - 1]));

    let mix = ((position - start) / (end - start)).clamp(0.0, 1.0);
    let lerp = |from: u8, to: u8| from as f32 + (to as f32 - from as f32) * mix;

    Color::Rgb(
        lerp(from.0, to.0) as u8,
        lerp(from.1, to.1) as u8,
        lerp(from.2, to.2) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::balance_valleys;

    #[test]
    fn balancing_softens_local_valleys_between_peaks() {
        let mut values = vec![0.0; 80];
        values[24] = 0.68;
        values[25] = 0.64;
        values[40] = 0.61;
        values[41] = 0.66;

        balance_valleys(&mut values);

        assert!(values[32] > 0.08);
    }

    #[test]
    fn balancing_does_not_lift_a_quiet_tail_without_a_right_peak() {
        let mut values = vec![0.0; 80];
        values[10] = 0.7;
        values[11] = 0.65;
        values[18] = 0.08;
        values[24] = 0.04;

        balance_valleys(&mut values);

        let tail = &values[50..70];
        let max_tail = tail.iter().copied().fold(0.0_f32, f32::max);

        assert!(max_tail < 0.08);
    }
}
