use crate::{app::App, pattern::Step};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph, Wrap},
};

pub fn draw(frame: &mut Frame, app: &App) {
    let editor = app.editor.as_ref().unwrap();
    let pattern = app.pattern;
    let rows = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(3),
        Constraint::Length(4),
        Constraint::Length(4),
    ])
    .margin(1)
    .split(frame.area());
    let pending = app.playing && app.active.pattern != pattern;
    frame.render_widget(
        Paragraph::new(vec![
            Line::styled(
                "R H Y T H M   E D I T O R",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Line::from(format!(
                "{} BPM · {} bars × {} beats × {} points · {}",
                app.bpm,
                pattern.bars,
                pattern.beats,
                pattern.density,
                if pattern.enabled {
                    "PATTERN ON"
                } else {
                    "NORMAL MODE"
                }
            )),
            Line::from(format!(
                "Cursor {}.{}.{} · {} · {}",
                editor.bar + 1,
                editor.beat + 1,
                editor.sub + 1,
                if app.playing { "PLAYING" } else { "PAUSED" },
                if app.silent_bar && app.playing {
                    "SILENT BAR"
                } else if app.muted {
                    "MUTED"
                } else {
                    "SOUND"
                }
            )),
            Line::from(if pending {
                "Edits queued for next bar"
            } else {
                "X accent · x normal · . rest (time continues)"
            }),
        ])
        .wrap(Wrap { trim: true }),
        rows[0],
    );

    // Scroll in whole-beat pages so dense grids remain editable in narrow terminals.
    let width = rows[1].width.saturating_sub(8);
    let visible_beats = (width / (u16::from(pattern.density) * 2 + 1))
        .max(1)
        .min(u16::from(pattern.beats)) as u8;
    let first_beat = editor.beat / visible_beats * visible_beats;
    let last_beat = (first_beat + visible_beats).min(pattern.beats);
    let visible_bars = rows[1]
        .height
        .saturating_sub(3)
        .max(1)
        .min(u16::from(pattern.bars)) as u8;
    let first_bar = editor.bar.saturating_sub(visible_bars - 1);
    let mut heading = String::from("Beat   ");
    for beat in first_beat..last_beat {
        heading.push_str(&format!(
            "{:<width$}",
            beat + 1,
            width = usize::from(pattern.density) * 2 + 1
        ));
    }
    let mut lines = vec![Line::from(heading)];
    for bar in first_bar..(first_bar + visible_bars).min(pattern.bars) {
        let mut points = vec![Span::raw(format!("Bar {}  ", bar + 1))];
        for beat in first_beat..last_beat {
            for sub in 0..pattern.density {
                let step = pattern.get(bar, beat, sub);
                let cursor = (editor.bar, editor.beat, editor.sub) == (bar, beat, sub);
                let playing = app.playing
                    && app.active.pattern.enabled
                    && app.active.pattern.beats == pattern.beats
                    && app.active.pattern.density == pattern.density
                    && (app.pattern_bar, app.beat, app.sub) == (bar, beat, sub);
                let mut style = Style::default().fg(match step {
                    Step::Off => Color::DarkGray,
                    Step::Normal => Color::Cyan,
                    Step::Accent => Color::Yellow,
                });
                if cursor {
                    style = style.bg(Color::DarkGray).add_modifier(Modifier::REVERSED);
                }
                if playing {
                    style = style
                        .add_modifier(Modifier::UNDERLINED | Modifier::BOLD)
                        .fg(Color::Green);
                }
                points.push(Span::styled(format!("{} ", step.symbol()), style));
            }
            points.push(Span::raw(" "));
        }
        lines.push(Line::from(points));
    }
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(format!(
            " Grid · beats {}–{} / {} · cursor highlighted, playback underlined ",
            first_beat + 1,
            last_beat,
            pattern.beats
        ))),
        rows[1],
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format!(
                "Play: bar {} · beat {} · point {} · training {} sound / {} silent",
                app.pattern_bar + 1,
                app.beat + 1,
                app.sub + 1,
                app.training.sound_bars,
                app.training.silent_bars
            )),
            Line::from(app.audio_status.as_str()),
            Line::from(app.config_status.as_str()),
            Line::styled(app.notice.as_str(), Style::default().fg(Color::Yellow)),
        ])
        .wrap(Wrap { trim: true }),
        rows[2],
    );
    frame.render_widget(Paragraph::new("←→ point  ↑↓ bar  Space on/off  A accent  Enter play/pause\nB dimensions  P mode  +/- BPM  V mixer  G training\nC clear bar  F fill bar  Y copy to next  U undo\nEsc return  : commands  ? help  Q quit")
        .wrap(Wrap { trim: true }), rows[3]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::Editor;
    use ratatui::{Terminal, backend::TestBackend};
    #[test]
    fn editing_cursor_and_playback_position_are_distinct() {
        let mut app = App {
            playing: true,
            editor: Some(Editor {
                sub: 1,
                ..Editor::default()
            }),
            ..App::default()
        };
        app.pattern.enabled = true;
        app.active = app.settings();
        let mut terminal = Terminal::new(TestBackend::new(100, 28)).unwrap();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let cells = &terminal.backend().buffer().content;
        assert!(
            cells
                .iter()
                .any(|cell| cell.modifier.contains(Modifier::REVERSED)
                    && !cell.modifier.contains(Modifier::UNDERLINED))
        );
        assert!(
            cells
                .iter()
                .any(|cell| cell.modifier.contains(Modifier::UNDERLINED)
                    && !cell.modifier.contains(Modifier::REVERSED))
        );
    }
    #[test]
    fn renders_and_scrolls_to_last_point_at_every_terminal_size() {
        for (width, height) in [(100, 28), (52, 22), (30, 10), (1, 1)] {
            let mut app = App {
                editor: Some(Editor {
                    bar: 3,
                    beat: 11,
                    sub: 11,
                    ..Editor::default()
                }),
                ..App::default()
            };
            app.pattern.resize(4, 12, 12);
            app.pattern.enabled = true;
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal.draw(|frame| draw(frame, &app)).unwrap();
            if width >= 52 {
                let text: String = terminal
                    .backend()
                    .buffer()
                    .content
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                assert!(text.contains("Bar 4"));
                assert!(text.contains("Cursor 4.12.12"));
            }
        }
    }
}
