use crate::{app::App, monitor::WAVE_POINTS};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{
        Block, Gauge, Paragraph, Wrap,
        canvas::{Canvas, Line as WaveLine},
    },
};

pub fn draw(frame: &mut Frame, app: &App) {
    let snapshot = &app.input_snapshot;
    let rows = Layout::vertical([
        Constraint::Length(4),
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(4),
        Constraint::Length(3),
    ])
    .margin(1)
    .split(frame.area());
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                "D R Y   A U D I O   I N P U T",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Line::from(snapshot.status.as_str()),
            Line::from(format!(
                "Source: {}",
                snapshot.source.as_deref().unwrap_or("none")
            )),
            Line::from(format!(
                "{} Hz · {} channel(s) · viewing channel {}",
                snapshot.sample_rate,
                snapshot.channels,
                snapshot.channel + 1
            )),
        ])
        .wrap(Wrap { trim: true }),
        rows[0],
    );
    let peak = snapshot.peaks[snapshot.channel];
    let db = if peak > 0.0 {
        20.0 * peak.log10()
    } else {
        -120.0
    };
    frame.render_widget(
        Gauge::default()
            .block(Block::bordered().title(" Input peak "))
            .ratio(f64::from(((db + 60.0) / 60.0).clamp(0.0, 1.0)))
            .gauge_style(Style::default().fg(if peak >= 1.0 {
                Color::Red
            } else {
                Color::Green
            }))
            .label(format!(
                "{db:.1} dBFS{}",
                if peak >= 1.0 { " · CLIP" } else { "" }
            )),
        rows[1],
    );
    frame.render_widget(
        Canvas::default()
            .block(Block::bordered().title(" Dry waveform · ~2 seconds · min/max envelope "))
            .x_bounds([0.0, WAVE_POINTS as f64])
            .y_bounds([-1.0, 1.0])
            .paint(|context| {
                context.draw(&WaveLine {
                    x1: 0.0,
                    y1: 0.0,
                    x2: WAVE_POINTS as f64,
                    y2: 0.0,
                    color: Color::DarkGray,
                });
                let offset = WAVE_POINTS.saturating_sub(snapshot.waveform.len());
                for (index, (min, max)) in snapshot.waveform.iter().enumerate() {
                    let x = (index + offset) as f64;
                    context.draw(&WaveLine {
                        x1: x,
                        x2: x,
                        y1: f64::from(min.clamp(-1.0, 1.0)),
                        y2: f64::from(max.clamp(-1.0, 1.0)),
                        color: Color::Cyan,
                    });
                }
            }),
        rows[2],
    );
    frame.render_widget(Paragraph::new(format!(
        "Sources {} · queued blocks {} · dropped since connect {} · gaps/resets {}\n{}\nNo recording or input playback. Timing calibration/scoring not implemented.",
        snapshot.sources.len(), snapshot.queued, snapshot.dropped, snapshot.discontinuities,
        if app.playing { "Independent TUI metronome is PLAYING (not synchronized to REAPER)" } else { "Independent TUI metronome is PAUSED" },
    )).wrap(Wrap { trim: true }), rows[3]);
    frame.render_widget(Paragraph::new("N next source  C next channel  R rescan/reconnect\nSpace metronome play/pause  +/- BPM  Esc/I return  ? help  Q quit")
        .wrap(Wrap { trim: true }), rows[4]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    #[test]
    fn draws_waveforms_including_tiny_terminal() {
        for (width, height) in [(100, 28), (52, 22), (1, 1)] {
            let mut app = App::default();
            app.input_snapshot.waveform = vec![(-0.5, 0.75); WAVE_POINTS];
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| draw(frame, &app)).unwrap();
        }
    }
}
