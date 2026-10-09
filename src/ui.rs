use crate::app::{App, Modal, Subdivision, Voice};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, Paragraph, Wrap},
};
use std::time::Duration;

fn highlight() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

pub fn draw(frame: &mut Frame, app: &App) {
    if app.editor.is_some() {
        crate::grid::draw(frame, app);
        draw_modal(frame, app);
        return;
    }
    let area = frame.area();
    let active = if app.playing {
        app.active
    } else {
        app.settings()
    };
    if area.width < 52 || area.height < 22 {
        frame.render_widget(Paragraph::new(format!(
            "METRONOME\n{} BPM · {} beats/bar · {}\n{} · beat {}.{}\nTraining: {} sound / {} silent · {}\n{}\nSpace play/pause · G training · ? help\nResize to 52×22 for full view",
             app.bpm, active.bar_beats(), if active.pattern.enabled { "Pattern" } else { active.subdivision.name() },
            if app.playing { "PLAYING" } else { "PAUSED" }, app.beat + 1, app.sub + 1,
            app.training.sound_bars, app.training.silent_bars, if app.playing && app.silent_bar { "SILENT BAR" } else { "SOUND" }, app.config_status,
        )).wrap(Wrap { trim: true }), area);
    } else {
        let rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(5),
            Constraint::Length(4),
            Constraint::Length(3),
            Constraint::Min(4),
            Constraint::Length(3),
        ])
        .margin(1)
        .split(area);
        frame.render_widget(
            Paragraph::new("M E T R O N O M E")
                .alignment(Alignment::Center)
                .style(highlight()),
            rows[0],
        );
        let labels = [
            format!("{} BPM", app.bpm),
            format!("{} beats/bar", app.settings().bar_beats()),
            if app.pattern.enabled {
                format!("{} pts/beat", app.pattern.density)
            } else {
                app.subdivision.name().into()
            },
            app.voice.name().into(),
        ];
        let mut parameters = Vec::new();
        for (index, label) in labels.into_iter().enumerate() {
            parameters.push(Span::styled(
                format!(" {} ", label),
                if index == app.selected {
                    highlight().bg(Color::DarkGray)
                } else {
                    Style::default()
                },
            ));
        }
        let pending = app.playing
            && (active.beats_per_bar != app.beats_per_bar
                || active.subdivision != app.subdivision
                || active.training != app.training
                || active.pattern != app.pattern);
        let training_status = if active.training.silent_bars == 0 {
            "Training off".to_owned()
        } else {
            let silent = app.playing && app.silent_bar;
            let phase_bar = if silent {
                app.cycle_bar - active.training.sound_bars + 1
            } else {
                app.cycle_bar + 1
            };
            format!(
                "{} {}/{}",
                if silent { "SILENT" } else { "SOUND" },
                phase_bar,
                if silent {
                    active.training.silent_bars
                } else {
                    active.training.sound_bars
                }
            )
        };
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(parameters),
                Line::from(format!(
                    "{} · {} · {} · {}",
                    if app.playing { "PLAYING" } else { "PAUSED" },
                    if app.muted { "MUTED" } else { "SOUND ON" },
                    training_status,
                    if active.pattern.enabled {
                        format!("PATTERN {}/{}", app.pattern_bar + 1, active.pattern.bars)
                    } else {
                        "NORMAL".into()
                    },
                )),
                Line::from(if pending {
                    "Rhythm / training change queued for next bar"
                } else {
                    "Quarter-note BPM · Enter edits selected parameter"
                }),
            ])
            .alignment(Alignment::Center)
            .block(Block::bordered().title(" Practice ")),
            rows[1],
        );
        let mut beats = Vec::new();
        for beat in 0..active.bar_beats() {
            let selected = app.playing && beat == app.beat;
            beats.push(Span::styled(
                format!(" {} ", beat + 1),
                Style::default()
                    .fg(if selected {
                        if beat == 0 {
                            Color::Yellow
                        } else {
                            Color::Cyan
                        }
                    } else {
                        Color::DarkGray
                    })
                    .add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            ));
        }
        let subdivisions: Vec<_> = (0..active.points_per_beat())
            .map(|sub| {
                Span::styled(
                    if app.playing && sub == app.sub {
                        " ● "
                    } else {
                        " ○ "
                    },
                    if app.playing && sub == app.sub {
                        highlight()
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                )
            })
            .collect();
        frame.render_widget(
            Paragraph::new(vec![Line::from(beats), Line::from(subdivisions)])
                .alignment(Alignment::Center)
                .block(Block::bordered().title(" Beat / subdivision ")),
            rows[2],
        );
        let elapsed = app.last_beat.map(|at| at.elapsed());
        let progress = if app.playing {
            elapsed.map_or(0.0, |elapsed| {
                (elapsed.as_secs_f64() / active.tick_interval().as_secs_f64()).min(1.0)
            })
        } else {
            0.0
        };
        let flashing =
            app.playing && elapsed.is_some_and(|elapsed| elapsed < Duration::from_millis(70));
        frame.render_widget(
            Gauge::default()
                .block(Block::bordered().title(" Pulse "))
                .gauge_style(Style::default().fg(if app.beat == 0 && app.sub == 0 {
                    Color::Yellow
                } else {
                    Color::Cyan
                }))
                .ratio(progress)
                .label(if flashing { "●" } else { "" }),
            rows[3],
        );
        frame.render_widget(
            Paragraph::new(format!(
                "Volume: main {}% · accent {}% · beat {}% · sub {}%\n{}\n{}",
                app.volumes.master,
                app.volumes.accent,
                app.volumes.beat,
                app.volumes.subdivision,
                app.audio_status,
                app.config_status,
            ))
            .wrap(Wrap { trim: true })
            .style(Style::default().fg(Color::Gray)),
            rows[4],
        );
        frame.render_widget(Paragraph::new("Space play/pause  ↑↓ tempo  ←→ select  Enter edit\nE grid  P mode  S subdivision  V mixer  G training  : commands  ? help  Q quit")
            .wrap(Wrap { trim: true }).block(Block::default().borders(Borders::TOP)), rows[5]);
    }
    draw_modal(frame, app);
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

fn draw_modal(frame: &mut Frame, app: &App) {
    let (title, lines, height) = match &app.modal {
        Modal::None => return,
        Modal::Help if app.editor.is_some() => (
            " Editor help ",
            vec![
                Line::from("←→ point · ↑↓ bar · Home/End first/last point"),
                Line::from("Space toggle sound · A toggle accent"),
                Line::from("Enter play/pause · +/- tempo · PgUp/PgDn ±10"),
                Line::from("B dimensions · P pattern/normal mode"),
                Line::from("C clear bar · F fill bar · Y copy to next bar"),
                Line::from("U undo up to 64 edits, including dimensions"),
                Line::from("V mixer · G training · T tap tempo · M mute"),
                Line::from(": commands · Q quit · Esc return"),
                Line::from("Cursor is highlighted; playback is underlined."),
                Line::from("Edits apply next bar; rests still take time."),
            ],
            14,
        ),
        Modal::Help => (
            " Help ",
            vec![
                Line::from("Space       play / pause"),
                Line::from("↑↓ / +/-    BPM ±1    PgUp/PgDn ±10"),
                Line::from("←→ / Tab    select parameter; Enter edits"),
                Line::from("S           subdivision chooser"),
                Line::from("V           volume mixer (live adjustment)"),
                Line::from("G           sound / silent bar training"),
                Line::from("E           rhythm editor; P pattern/normal mode"),
                Line::from("T           tap tempo; M mute; R reset"),
                Line::from(":           command search; Tab completes"),
                Line::from("Q / Esc     quit from main screen"),
                Line::from("Esc         close popup; Ctrl+C always quits"),
                Line::from("Rhythm changes apply at the next bar."),
                Line::from("Relative volumes multiply the master volume."),
            ],
            17,
        ),
        Modal::Mixer(row) => {
            let values = [
                ("Master", app.volumes.master),
                ("Accent", app.volumes.accent),
                ("Beat", app.volumes.beat),
                ("Subdivision", app.volumes.subdivision),
            ];
            let mut lines: Vec<_> = values
                .iter()
                .enumerate()
                .map(|(index, (label, value))| {
                    let filled = usize::from(*value) / 10;
                    Line::styled(
                        format!(
                            "{} {:<12} {}{} {:>3}%",
                            if index == *row { ">" } else { " " },
                            label,
                            "█".repeat(filled),
                            "░".repeat(10 - filled),
                            value
                        ),
                        if index == *row {
                            highlight()
                        } else {
                            Style::default()
                        },
                    )
                })
                .collect();
            lines.push(Line::from(
                "↑↓ select · ←→ ±5% · Esc closes (keeps changes)",
            ));
            (" Mixer · live ", lines, 9)
        }
        Modal::PatternSetup(row) => {
            let values = [
                ("Bars", app.pattern.bars),
                ("Beats per bar", app.pattern.beats),
                ("Points per beat", app.pattern.density),
            ];
            let mut lines: Vec<_> = values
                .iter()
                .enumerate()
                .map(|(index, (label, value))| {
                    Line::styled(
                        format!(
                            "{} {:<16} {}",
                            if index == *row { ">" } else { " " },
                            label,
                            value
                        ),
                        if index == *row {
                            highlight()
                        } else {
                            Style::default()
                        },
                    )
                })
                .collect();
            lines.push(Line::from("Bars 1–4 · Beats 1–12 · Points 1/2/3/4/6/12"));
            lines.push(Line::from("Resizing keeps exact time positions only."));
            lines.push(Line::from(
                "Removed points are discarded; U undoes in editor.",
            ));
            lines.push(Line::from(
                "↑↓ select · ←→ resize · Esc closes (keeps changes)",
            ));
            (" Pattern dimensions ", lines, 11)
        }
        Modal::Training(row) => {
            let lines = vec![
                Line::styled(
                    format!(
                        "{} Sound bars   {:>2}",
                        if *row == 0 { ">" } else { " " },
                        app.training.sound_bars
                    ),
                    if *row == 0 {
                        highlight()
                    } else {
                        Style::default()
                    },
                ),
                Line::styled(
                    format!(
                        "{} Silent bars  {:>2}",
                        if *row == 1 { ">" } else { " " },
                        app.training.silent_bars
                    ),
                    if *row == 1 {
                        highlight()
                    } else {
                        Style::default()
                    },
                ),
                Line::from("Sound: 1–32 · Silent: 0–32 (0 disables training)"),
                Line::from("Changes start a fresh cycle at the next bar."),
                Line::from("Visual beats continue during silent bars."),
                Line::from("↑↓ select · ←→ adjust · Esc closes (keeps changes)"),
            ];
            (" Silent bar training ", lines, 10)
        }
        Modal::Subdivision(row) => {
            let mut lines: Vec<_> = Subdivision::ALL
                .iter()
                .enumerate()
                .map(|(index, sub)| {
                    Line::styled(
                        format!(
                            "{} {:<12} {} click(s) per quarter note",
                            if index == *row { ">" } else { " " },
                            sub.name(),
                            sub.count()
                        ),
                        if index == *row {
                            highlight()
                        } else {
                            Style::default()
                        },
                    )
                })
                .collect();
            lines.push(Line::from("↑↓ select · Enter confirm · Esc cancel"));
            (" Subdivision ", lines, 9)
        }
        Modal::Voice(row) => {
            let mut lines: Vec<_> = Voice::ALL
                .iter()
                .enumerate()
                .map(|(index, voice)| {
                    Line::styled(
                        format!("{} {}", if index == *row { ">" } else { " " }, voice.name()),
                        if index == *row {
                            highlight()
                        } else {
                            Style::default()
                        },
                    )
                })
                .collect();
            lines.push(Line::from("↑↓ select · Enter confirm · Esc cancel"));
            (" Sound preset ", lines, 8)
        }
        Modal::Edit(text) => (
            " Edit parameter ",
            vec![
                Line::styled(format!("> {text}█"), highlight()),
                Line::from(if app.selected == 0 {
                    "BPM: 30–300"
                } else {
                    "Beats per bar: 1–12"
                }),
                Line::from("Enter confirm · Esc cancel"),
                Line::styled(app.notice.as_str(), Style::default().fg(Color::Yellow)),
            ],
            8,
        ),
        Modal::Command(text) => {
            let mut lines = vec![
                Line::styled(format!(":{text}█"), highlight()),
                Line::from("Enter run · Tab complete · Esc cancel"),
            ];
            let matches = crate::input::suggestions(text);
            for suggestion in matches.iter().take(8) {
                lines.push(Line::from(format!("  {suggestion}")));
            }
            if matches.is_empty() {
                lines.push(Line::from("  No matching actions"));
            }
            lines.push(Line::from("Examples: bpm 132 · volume subdivision 35"));
            lines.push(Line::styled(
                app.notice.as_str(),
                Style::default().fg(Color::Yellow),
            ));
            (" Commands ", lines, 16)
        }
    };
    let popup = centered(frame.area(), 60, height);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .block(Block::bordered().title(title).border_style(highlight())),
        popup,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    #[test]
    fn renders_every_modal_and_tiny_terminals() {
        for (width, height) in [(80, 24), (52, 22), (30, 10), (1, 1)] {
            for modal in [
                Modal::None,
                Modal::Help,
                Modal::Mixer(3),
                Modal::Training(1),
                Modal::PatternSetup(2),
                Modal::Subdivision(2),
                Modal::Voice(1),
                Modal::Edit("120".into()),
                Modal::Command("sub".into()),
            ] {
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                let app = App {
                    modal,
                    ..App::default()
                };
                terminal.draw(|frame| draw(frame, &app)).unwrap();
            }
        }
    }
    #[test]
    fn silent_phase_and_config_warning_are_visible() {
        let mut terminal = Terminal::new(TestBackend::new(100, 26)).unwrap();
        let mut app = App {
            playing: true,
            silent_bar: true,
            cycle_bar: 4,
            training: crate::app::Training {
                sound_bars: 4,
                silent_bars: 2,
            },
            config_status: "Config save failed: access denied".into(),
            ..App::default()
        };
        app.active = app.settings();
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("SILENT 1/2"));
        assert!(text.contains("Config save failed"));
    }
    #[test]
    fn command_popup_displays_matching_action() {
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        let app = App {
            modal: Modal::Command("trip".into()),
            ..App::default()
        };
        terminal.draw(|frame| draw(frame, &app)).unwrap();
        let text: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(text.contains("subdivision triplet"));
    }
}
