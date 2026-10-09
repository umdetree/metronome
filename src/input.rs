use crate::app::{App, MAX_BPM, MIN_BPM, Modal, Subdivision, Voice};
use crate::pattern::{DENSITIES, Editor, Step};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Default)]
pub struct Action {
    pub quit: bool,
    pub reset: bool,
}

pub fn handle(app: &mut App, key: KeyEvent) -> Action {
    let mut action = Action::default();
    if key.kind == KeyEventKind::Release {
        return action;
    }
    if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
        action.quit = true;
        return action;
    }
    // Modal input never leaks into global shortcuts.
    if app.modal != Modal::None {
        modal_key(app, key.code, &mut action);
        return action;
    }
    if app.editor.is_some() {
        editor_key(app, key, &mut action);
        return action;
    }
    let repeat = key.kind == KeyEventKind::Repeat;
    match key.code {
        KeyCode::Char('q') | KeyCode::Esc => action.quit = true,
        KeyCode::Up | KeyCode::Char('+') | KeyCode::Char('=') => app.adjust_bpm(1),
        KeyCode::Down | KeyCode::Char('-') => app.adjust_bpm(-1),
        KeyCode::PageUp => app.adjust_bpm(10),
        KeyCode::PageDown => app.adjust_bpm(-10),
        KeyCode::Left => app.selected = (app.selected + 3) % 4,
        KeyCode::Right | KeyCode::Tab => app.selected = (app.selected + 1) % 4,
        KeyCode::Enter => {
            app.modal = match app.selected {
                0 => Modal::Edit(app.bpm.to_string()),
                1 => Modal::Edit(
                    if app.pattern.enabled {
                        app.pattern.beats
                    } else {
                        app.beats_per_bar
                    }
                    .to_string(),
                ),
                2 if app.pattern.enabled => Modal::PatternSetup(2),
                2 => Modal::Subdivision(usize::from(app.subdivision.count() - 1)),
                _ => Modal::Voice(
                    Voice::ALL
                        .iter()
                        .position(|voice| *voice == app.voice)
                        .unwrap(),
                ),
            }
        }
        KeyCode::Char(' ') if !repeat => {
            toggle(app);
            action.reset = true;
        }
        KeyCode::Char('t') if !repeat => app.tap(std::time::Instant::now()),
        KeyCode::Char('m') if !repeat => app.muted = !app.muted,
        KeyCode::Char('s') if !repeat => {
            app.modal = if app.pattern.enabled {
                Modal::PatternSetup(2)
            } else {
                Modal::Subdivision(usize::from(app.subdivision.count() - 1))
            }
        }
        KeyCode::Char('v') if !repeat => app.modal = Modal::Mixer(0),
        KeyCode::Char('g') if !repeat => app.modal = Modal::Training(0),
        KeyCode::Char('e') if !repeat => open_editor(app),
        KeyCode::Char('p') if !repeat => app.pattern.enabled = !app.pattern.enabled,
        KeyCode::Char('?') if !repeat => app.modal = Modal::Help,
        KeyCode::Char(':') if !repeat => {
            app.notice.clear();
            app.modal = Modal::Command(String::new());
        }
        KeyCode::Char('r') if !repeat => {
            reset(app);
            action.reset = true;
        }
        _ => {}
    }
    action
}

fn toggle(app: &mut App) {
    app.playing = !app.playing;
    app.beat = 0;
    app.sub = 0;
    app.cycle_bar = 0;
    app.pattern_bar = 0;
    app.silent_bar = false;
    app.last_beat = None;
}

fn open_editor(app: &mut App) {
    app.pattern.enabled = true;
    app.editor = Some(Editor::default());
    app.notice.clear();
}

fn resize_pattern(app: &mut App, bars: u8, beats: u8, density: u8) {
    let before = app.pattern;
    app.pattern.resize(bars, beats, density);
    if let Some(editor) = &mut app.editor {
        if before != app.pattern {
            editor.remember(before);
        }
        editor.clamp(app.pattern);
    }
}

fn editor_key(app: &mut App, key: KeyEvent, action: &mut Action) {
    let repeat = key.kind == KeyEventKind::Repeat;
    let pattern = app.pattern;
    let editor = app.editor.as_mut().unwrap();
    editor.clamp(pattern);
    match key.code {
        KeyCode::Left | KeyCode::Right => {
            let count = u16::from(pattern.beats) * u16::from(pattern.density);
            let position =
                u16::from(editor.beat) * u16::from(pattern.density) + u16::from(editor.sub);
            let next = if key.code == KeyCode::Right {
                (position + 1) % count
            } else {
                (position + count - 1) % count
            };
            editor.beat = (next / u16::from(pattern.density)) as u8;
            editor.sub = (next % u16::from(pattern.density)) as u8;
        }
        KeyCode::Up => editor.bar = (editor.bar + pattern.bars - 1) % pattern.bars,
        KeyCode::Down => editor.bar = (editor.bar + 1) % pattern.bars,
        KeyCode::Home => {
            editor.beat = 0;
            editor.sub = 0;
        }
        KeyCode::End => {
            editor.beat = pattern.beats - 1;
            editor.sub = pattern.density - 1;
        }
        KeyCode::Char(' ') | KeyCode::Char('a') if !repeat => {
            editor.remember(pattern);
            let previous = pattern.get(editor.bar, editor.beat, editor.sub);
            let next = if key.code == KeyCode::Char('a') {
                if previous == Step::Accent {
                    Step::Normal
                } else {
                    Step::Accent
                }
            } else if previous == Step::Off {
                Step::Normal
            } else {
                Step::Off
            };
            app.pattern.set(editor.bar, editor.beat, editor.sub, next);
        }
        KeyCode::Char('c') if !repeat => {
            editor.remember(pattern);
            app.pattern.clear_bar(editor.bar);
        }
        KeyCode::Char('f') if !repeat => {
            editor.remember(pattern);
            for beat in 0..pattern.beats {
                for sub in 0..pattern.density {
                    app.pattern.set(
                        editor.bar,
                        beat,
                        sub,
                        if beat == 0 && sub == 0 {
                            Step::Accent
                        } else {
                            Step::Normal
                        },
                    );
                }
            }
        }
        KeyCode::Char('y') if !repeat => {
            if pattern.bars > 1 {
                editor.remember(pattern);
                app.pattern
                    .copy_bar(editor.bar, (editor.bar + 1) % pattern.bars);
            } else {
                app.notice = "Add another bar with B before copying".into();
            }
        }
        KeyCode::Char('u') if !repeat => {
            if let Some(mut previous) = editor.undo.pop() {
                // Undo edits, not an independent mode switch.
                previous.enabled = app.pattern.enabled;
                app.pattern = previous;
                editor.clamp(app.pattern);
            }
        }
        KeyCode::Char('b') if !repeat => app.modal = Modal::PatternSetup(0),
        KeyCode::Char('p') if !repeat => app.pattern.enabled = !app.pattern.enabled,
        KeyCode::Enter if !repeat => {
            toggle(app);
            action.reset = true;
        }
        KeyCode::Esc | KeyCode::Char('e') if !repeat => {
            app.editor = None;
            app.notice.clear();
        }
        KeyCode::Char('q') => action.quit = true,
        KeyCode::Char('+') | KeyCode::Char('=') => app.adjust_bpm(1),
        KeyCode::Char('-') => app.adjust_bpm(-1),
        KeyCode::PageUp => app.adjust_bpm(10),
        KeyCode::PageDown => app.adjust_bpm(-10),
        KeyCode::Char('t') if !repeat => app.tap(std::time::Instant::now()),
        KeyCode::Char('m') if !repeat => app.muted = !app.muted,
        KeyCode::Char('v') if !repeat => app.modal = Modal::Mixer(0),
        KeyCode::Char('g') if !repeat => app.modal = Modal::Training(0),
        KeyCode::Char('?') if !repeat => app.modal = Modal::Help,
        KeyCode::Char(':') if !repeat => {
            app.notice.clear();
            app.modal = Modal::Command(String::new());
        }
        _ => {}
    }
}

fn reset(app: &mut App) {
    let status = std::mem::take(&mut app.audio_status);
    let config_status = std::mem::take(&mut app.config_status);
    *app = App::default();
    app.audio_status = status;
    app.config_status = config_status;
}

fn modal_key(app: &mut App, code: KeyCode, action: &mut Action) {
    if code == KeyCode::Esc {
        app.modal = Modal::None;
        app.notice.clear();
        return;
    }
    match app.modal.clone() {
        Modal::Help => {
            if code == KeyCode::Enter || code == KeyCode::Char('?') {
                app.modal = Modal::None;
            }
        }
        Modal::Mixer(mut row) => {
            match code {
                KeyCode::Up => row = (row + 3) % 4,
                KeyCode::Down | KeyCode::Tab => row = (row + 1) % 4,
                KeyCode::Left | KeyCode::Right => {
                    let value = match row {
                        0 => &mut app.volumes.master,
                        1 => &mut app.volumes.accent,
                        2 => &mut app.volumes.beat,
                        _ => &mut app.volumes.subdivision,
                    };
                    *value = (i16::from(*value) + if code == KeyCode::Right { 5 } else { -5 })
                        .clamp(0, 100) as u8;
                }
                KeyCode::Enter => {
                    app.modal = Modal::None;
                    return;
                }
                _ => {}
            }
            app.modal = Modal::Mixer(row);
        }
        Modal::Training(mut row) => {
            match code {
                KeyCode::Up | KeyCode::Down | KeyCode::Tab => row = 1 - row,
                KeyCode::Left | KeyCode::Right => {
                    let value = if row == 0 {
                        &mut app.training.sound_bars
                    } else {
                        &mut app.training.silent_bars
                    };
                    *value = (i16::from(*value) + if code == KeyCode::Right { 1 } else { -1 })
                        .clamp(if row == 0 { 1 } else { 0 }, 32) as u8;
                }
                KeyCode::Enter => {
                    app.modal = Modal::None;
                    return;
                }
                _ => {}
            }
            app.modal = Modal::Training(row);
        }
        Modal::PatternSetup(mut row) => {
            match code {
                KeyCode::Up => row = (row + 2) % 3,
                KeyCode::Down | KeyCode::Tab => row = (row + 1) % 3,
                KeyCode::Left | KeyCode::Right => {
                    let delta = if code == KeyCode::Right { 1 } else { -1 };
                    let pattern = app.pattern;
                    let (mut bars, mut beats, mut density) =
                        (pattern.bars, pattern.beats, pattern.density);
                    match row {
                        0 => bars = (i16::from(bars) + delta).clamp(1, 4) as u8,
                        1 => beats = (i16::from(beats) + delta).clamp(1, 12) as u8,
                        _ => {
                            let index = DENSITIES
                                .iter()
                                .position(|value| *value == density)
                                .unwrap();
                            density = DENSITIES[(index as i16 + delta).clamp(0, 5) as usize];
                        }
                    }
                    resize_pattern(app, bars, beats, density);
                }
                KeyCode::Enter => {
                    app.modal = Modal::None;
                    return;
                }
                _ => {}
            }
            app.modal = Modal::PatternSetup(row);
        }
        Modal::Subdivision(mut row) => {
            match code {
                KeyCode::Up => row = (row + 3) % 4,
                KeyCode::Down => row = (row + 1) % 4,
                KeyCode::Enter => {
                    app.subdivision = Subdivision::ALL[row];
                    app.modal = Modal::None;
                    return;
                }
                _ => {}
            }
            app.modal = Modal::Subdivision(row);
        }
        Modal::Voice(mut row) => {
            match code {
                KeyCode::Up => row = (row + 2) % 3,
                KeyCode::Down => row = (row + 1) % 3,
                KeyCode::Enter => {
                    app.voice = Voice::ALL[row];
                    app.modal = Modal::None;
                    return;
                }
                _ => {}
            }
            app.modal = Modal::Voice(row);
        }
        Modal::Edit(mut text) | Modal::Command(mut text) => {
            let command_mode = matches!(app.modal, Modal::Command(_));
            match code {
                KeyCode::Backspace => {
                    text.pop();
                }
                KeyCode::Char(ch) if text.len() < 100 && (command_mode || ch.is_ascii_digit()) => {
                    text.push(ch)
                }
                KeyCode::Tab if command_mode => {
                    let matches = suggestions(&text);
                    if let Some(first) = matches.first() {
                        text = first.to_string();
                    }
                }
                KeyCode::Enter => {
                    let result = if command_mode {
                        command(app, &text, action)
                    } else {
                        let prefix = if app.selected == 0 {
                            "bpm"
                        } else if app.pattern.enabled {
                            "pattern beats"
                        } else {
                            "beats"
                        };
                        command(app, &format!("{prefix} {text}"), action)
                    };
                    match result {
                        Ok(()) => {
                            app.modal = Modal::None;
                            app.notice.clear();
                            return;
                        }
                        Err(error) => app.notice = error,
                    }
                }
                _ => {}
            }
            app.modal = if command_mode {
                Modal::Command(text)
            } else {
                Modal::Edit(text)
            };
        }
        Modal::None => {}
    }
}

pub const COMMANDS: &[&str] = &[
    "bpm",
    "beats",
    "subdivision quarter",
    "subdivision eighth",
    "subdivision triplet",
    "subdivision sixteenth",
    "volume master",
    "volume accent",
    "volume beat",
    "volume subdivision",
    "voice crisp",
    "voice soft",
    "voice wood",
    "training off",
    "training",
    "pattern edit",
    "pattern on",
    "pattern off",
    "pattern bars",
    "pattern beats",
    "pattern density",
    "play",
    "pause",
    "mute",
    "reset",
    "quit",
];

pub fn suggestions(text: &str) -> Vec<&'static str> {
    let text = text.to_ascii_lowercase();
    COMMANDS
        .iter()
        .copied()
        .filter(|command| command.contains(text.trim()))
        .collect()
}

fn number(text: Option<&&str>, min: u16, max: u16) -> Result<u16, String> {
    text.and_then(|text| text.parse::<u16>().ok())
        .filter(|value| (min..=max).contains(value))
        .ok_or_else(|| format!("Enter a number from {min} to {max}"))
}

fn command(app: &mut App, text: &str, action: &mut Action) -> Result<(), String> {
    let lower = text.to_ascii_lowercase();
    let parts: Vec<_> = lower.split_whitespace().collect();
    match parts.as_slice() {
        ["bpm", _] => {
            let bpm = number(parts.get(1), MIN_BPM, MAX_BPM)?;
            app.adjust_bpm(bpm as i16 - app.bpm as i16);
        }
        ["beats", _] => app.beats_per_bar = number(parts.get(1), 1, 12)? as u8,
        ["subdivision", value] => {
            app.subdivision = Subdivision::ALL
                .into_iter()
                .find(|sub| sub.name().eq_ignore_ascii_case(value))
                .ok_or("Choose quarter, eighth, triplet or sixteenth")?
        }
        ["voice", value] => {
            app.voice = Voice::ALL
                .into_iter()
                .find(|voice| voice.name().eq_ignore_ascii_case(value))
                .ok_or("Choose crisp, soft or wood")?
        }
        ["training", "off"] => app.training.silent_bars = 0,
        ["training", _, _] => {
            let sound_bars = number(parts.get(1), 1, 32)? as u8;
            let silent_bars = number(parts.get(2), 0, 32)? as u8;
            app.training = crate::app::Training {
                sound_bars,
                silent_bars,
            };
        }
        ["pattern", "edit"] => open_editor(app),
        ["pattern", "on"] => app.pattern.enabled = true,
        ["pattern", "off"] => app.pattern.enabled = false,
        ["pattern", "bars", _] => {
            let bars = number(parts.get(2), 1, 4)? as u8;
            resize_pattern(app, bars, app.pattern.beats, app.pattern.density);
        }
        ["pattern", "beats", _] => {
            let beats = number(parts.get(2), 1, 12)? as u8;
            resize_pattern(app, app.pattern.bars, beats, app.pattern.density);
        }
        ["pattern", "density", _] => {
            let density = number(parts.get(2), 1, 12)? as u8;
            if !DENSITIES.contains(&density) {
                return Err("Choose density 1, 2, 3, 4, 6 or 12".into());
            }
            resize_pattern(app, app.pattern.bars, app.pattern.beats, density);
        }
        ["volume", target, _] => {
            let value = number(parts.get(2), 0, 100)? as u8;
            match *target {
                "master" => app.volumes.master = value,
                "accent" => app.volumes.accent = value,
                "beat" => app.volumes.beat = value,
                "subdivision" => app.volumes.subdivision = value,
                _ => return Err("Choose master, accent, beat or subdivision".into()),
            }
        }
        ["play"] => {
            if !app.playing {
                toggle(app);
                action.reset = true;
            }
        }
        ["pause"] => {
            if app.playing {
                toggle(app);
                action.reset = true;
            }
        }
        ["mute"] => app.muted = !app.muted,
        ["reset"] => {
            reset(app);
            action.reset = true;
        }
        ["quit"] => action.quit = true,
        _ => return Err("Unknown command or missing argument · Tab completes commands".into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn press(app: &mut App, code: KeyCode) -> Action {
        handle(app, KeyEvent::new(code, KeyModifiers::NONE))
    }
    #[test]
    fn modal_input_does_not_trigger_shortcuts() {
        let mut app = App {
            modal: Modal::Command(String::new()),
            ..App::default()
        };
        assert!(!press(&mut app, KeyCode::Char('q')).quit);
        press(&mut app, KeyCode::Char('m'));
        assert!(!app.muted);
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.modal, Modal::None);
    }
    #[test]
    fn validates_commands_without_changing_settings() {
        let mut app = App::default();
        let mut action = Action::default();
        assert!(command(&mut app, "bpm 0", &mut action).is_err());
        assert_eq!(app.bpm, 120);
        command(&mut app, "subdivision triplet", &mut action).unwrap();
        command(&mut app, "volume subdivision 0", &mut action).unwrap();
        assert_eq!(app.subdivision, Subdivision::Triplet);
        assert_eq!(app.volumes.subdivision, 0);
    }
    #[test]
    fn editor_space_edits_while_enter_controls_playback_and_escape_returns() {
        let mut app = App::default();
        press(&mut app, KeyCode::Char('e'));
        assert!(app.pattern.enabled);
        assert!(app.editor.is_some());
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.pattern.get(0, 0, 0), Step::Off);
        assert!(!app.playing);
        press(&mut app, KeyCode::Char('a'));
        assert_eq!(app.pattern.get(0, 0, 0), Step::Accent);
        assert!(press(&mut app, KeyCode::Enter).reset);
        assert!(app.playing);
        assert!(!press(&mut app, KeyCode::Esc).quit);
        assert!(app.editor.is_none());
        assert!(app.playing && app.pattern.enabled);
    }
    #[test]
    fn editor_copy_clear_and_undo_preserve_original_points() {
        let mut app = App::default();
        open_editor(&mut app);
        resize_pattern(&mut app, 2, 4, 4);
        press(&mut app, KeyCode::Char('y'));
        assert_eq!(app.pattern.get(1, 0, 0), Step::Accent);
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Char('c'));
        assert_eq!(app.pattern.get(1, 0, 0), Step::Off);
        press(&mut app, KeyCode::Char('u'));
        assert_eq!(app.pattern.get(1, 0, 0), Step::Accent);
        press(&mut app, KeyCode::Char('u'));
        assert_eq!(app.pattern.get(1, 0, 0), Step::Off);
        press(&mut app, KeyCode::Char('u'));
        assert_eq!(app.pattern.bars, 1);
        assert_eq!(app.editor.as_ref().unwrap().bar, 0);
    }
    #[test]
    fn grid_dimension_commands_validate_and_cursor_stays_in_bounds() {
        let mut app = App::default();
        open_editor(&mut app);
        let mut action = Action::default();
        command(&mut app, "pattern bars 4", &mut action).unwrap();
        command(&mut app, "pattern beats 12", &mut action).unwrap();
        command(&mut app, "pattern density 12", &mut action).unwrap();
        assert!(command(&mut app, "pattern density 5", &mut action).is_err());
        assert!(command(&mut app, "pattern bars 5", &mut action).is_err());
        assert_eq!(app.pattern.density, 12);
        press(&mut app, KeyCode::End);
        assert_eq!(app.editor.as_ref().unwrap().sub, 11);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.editor.as_ref().unwrap().beat, 0);
        press(&mut app, KeyCode::Left);
        assert_eq!(app.editor.as_ref().unwrap().beat, 11);
        resize_pattern(&mut app, 1, 1, 1);
        assert_eq!(app.editor.as_ref().unwrap().sub, 0);
    }
    #[test]
    fn training_commands_validate_both_counts_before_applying() {
        let mut app = App::default();
        let mut action = Action::default();
        command(&mut app, "training 4 2", &mut action).unwrap();
        assert_eq!(app.training.silent_bars, 2);
        assert!(command(&mut app, "training 7 33", &mut action).is_err());
        assert_eq!(app.training.sound_bars, 4);
        assert!(command(&mut app, "training 0 2", &mut action).is_err());
        command(&mut app, "training off", &mut action).unwrap();
        assert_eq!(app.training.silent_bars, 0);
    }
    #[test]
    fn training_popup_clamps_counts_without_interrupting_playback() {
        let mut app = App {
            playing: true,
            modal: Modal::Training(0),
            ..App::default()
        };
        for _ in 0..40 {
            press(&mut app, KeyCode::Left);
        }
        assert_eq!(app.training.sound_bars, 1);
        press(&mut app, KeyCode::Down);
        for _ in 0..40 {
            press(&mut app, KeyCode::Right);
        }
        assert_eq!(app.training.silent_bars, 32);
        press(&mut app, KeyCode::Esc);
        assert!(app.playing);
        assert_eq!(app.modal, Modal::None);
    }
    #[test]
    fn mixer_clamps_and_preserves_playback() {
        let mut app = App {
            playing: true,
            modal: Modal::Mixer(1),
            ..App::default()
        };
        press(&mut app, KeyCode::Right);
        assert_eq!(app.volumes.accent, 100);
        press(&mut app, KeyCode::Esc);
        assert!(app.playing);
    }
    #[test]
    fn numeric_editor_confirms_and_cancels() {
        let mut app = App {
            modal: Modal::Edit(String::new()),
            ..App::default()
        };
        for ch in "132".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.bpm, 132);
        assert_eq!(app.modal, Modal::None);
        app.selected = 1;
        app.modal = Modal::Edit("7".into());
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.beats_per_bar, 4);
    }
    #[test]
    fn holding_space_does_not_toggle_playback_repeatedly() {
        let mut app = App::default();
        assert!(press(&mut app, KeyCode::Char(' ')).reset);
        handle(
            &mut app,
            KeyEvent::new_with_kind(KeyCode::Char(' '), KeyModifiers::NONE, KeyEventKind::Repeat),
        );
        assert!(app.playing);
    }
    #[test]
    fn search_and_completion_are_discoverable() {
        assert!(suggestions("trip").contains(&"subdivision triplet"));
        let mut app = App {
            modal: Modal::Command("trip".into()),
            ..App::default()
        };
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.modal, Modal::Command("subdivision triplet".into()));
    }
}
