use crate::pattern::{Editor, Pattern, Step};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

pub const MIN_BPM: u16 = 30;
pub const MAX_BPM: u16 = 300;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Subdivision {
    #[default]
    Quarter,
    Eighth,
    Triplet,
    Sixteenth,
}

impl Subdivision {
    pub const ALL: [Self; 4] = [Self::Quarter, Self::Eighth, Self::Triplet, Self::Sixteenth];
    pub fn count(self) -> u8 {
        match self {
            Self::Quarter => 1,
            Self::Eighth => 2,
            Self::Triplet => 3,
            Self::Sixteenth => 4,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Quarter => "Quarter",
            Self::Eighth => "Eighth",
            Self::Triplet => "Triplet",
            Self::Sixteenth => "Sixteenth",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Voice {
    #[default]
    Crisp,
    Soft,
    Wood,
}

impl Voice {
    pub const ALL: [Self; 3] = [Self::Crisp, Self::Soft, Self::Wood];
    pub fn name(self) -> &'static str {
        match self {
            Self::Crisp => "Crisp",
            Self::Soft => "Soft",
            Self::Wood => "Wood",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Volumes {
    pub master: u8,
    pub accent: u8,
    pub beat: u8,
    pub subdivision: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Training {
    pub sound_bars: u8,
    pub silent_bars: u8,
}

impl Default for Training {
    fn default() -> Self {
        Self {
            sound_bars: 4,
            silent_bars: 0,
        }
    }
}

impl Training {
    pub fn is_silent(self, cycle_bar: u8) -> bool {
        self.silent_bars > 0 && cycle_bar >= self.sound_bars
    }
    pub fn cycle_length(self) -> u8 {
        self.sound_bars + self.silent_bars
    }
}

impl Default for Volumes {
    fn default() -> Self {
        Self {
            master: 80,
            accent: 100,
            beat: 75,
            subdivision: 40,
        }
    }
}

impl Volumes {
    pub fn gain(self, beat: u8, sub: u8) -> f32 {
        let relative = if sub != 0 {
            self.subdivision
        } else if beat == 0 {
            self.accent
        } else {
            self.beat
        };
        f32::from(self.master) * f32::from(relative) / 10_000.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub bpm: u16,
    pub beats_per_bar: u8,
    pub playing: bool,
    pub muted: bool,
    pub subdivision: Subdivision,
    pub volumes: Volumes,
    pub voice: Voice,
    pub training: Training,
    pub pattern: Pattern,
}

impl Settings {
    pub fn interval(self) -> Duration {
        Duration::from_secs_f64(60.0 / f64::from(self.bpm))
    }
    pub fn tick_interval(self) -> Duration {
        self.interval().div_f64(f64::from(self.points_per_beat()))
    }
    pub fn points_per_beat(self) -> u8 {
        if self.pattern.enabled {
            self.pattern.density
        } else {
            self.subdivision.count()
        }
    }
    pub fn bar_beats(self) -> u8 {
        if self.pattern.enabled {
            self.pattern.beats
        } else {
            self.beats_per_bar
        }
    }
    pub fn loop_bars(self) -> u8 {
        if self.pattern.enabled {
            self.pattern.bars
        } else {
            1
        }
    }
    pub fn step(self, bar: u8, beat: u8, sub: u8) -> Step {
        if self.pattern.enabled {
            self.pattern.get(bar, beat, sub)
        } else if beat == 0 && sub == 0 {
            Step::Accent
        } else {
            Step::Normal
        }
    }
    pub fn step_gain(self, step: Step, sub: u8) -> f32 {
        match step {
            Step::Off => 0.0,
            Step::Accent => self.volumes.gain(0, 0),
            Step::Normal => self.volumes.gain(1, sub),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Modal {
    #[default]
    None,
    Mixer(usize),
    Subdivision(usize),
    Voice(usize),
    Edit(String),
    Command(String),
    Help,
    Training(usize),
    PatternSetup(usize),
}

pub struct App {
    pub bpm: u16,
    pub beats_per_bar: u8,
    pub playing: bool,
    pub muted: bool,
    pub beat: u8,
    pub sub: u8,
    pub subdivision: Subdivision,
    pub volumes: Volumes,
    pub voice: Voice,
    pub training: Training,
    pub pattern: Pattern,
    pub editor: Option<Editor>,
    pub pattern_bar: u8,
    pub silent_bar: bool,
    pub cycle_bar: u8,
    pub config_status: String,
    pub active: Settings,
    pub selected: usize,
    pub modal: Modal,
    pub notice: String,
    pub last_beat: Option<Instant>,
    pub audio_status: String,
    pub(crate) taps: VecDeque<Instant>,
}

impl Default for App {
    fn default() -> Self {
        Self {
            bpm: 120,
            beats_per_bar: 4,
            playing: false,
            muted: false,
            beat: 0,
            sub: 0,
            subdivision: Subdivision::Quarter,
            volumes: Volumes::default(),
            voice: Voice::Crisp,
            training: Training::default(),
            pattern: Pattern::default(),
            editor: None,
            pattern_bar: 0,
            silent_bar: false,
            cycle_bar: 0,
            config_status: String::new(),
            active: Settings {
                bpm: 120,
                beats_per_bar: 4,
                playing: false,
                muted: false,
                subdivision: Subdivision::Quarter,
                volumes: Volumes::default(),
                voice: Voice::Crisp,
                training: Training::default(),
                pattern: Pattern::default(),
            },
            selected: 0,
            modal: Modal::None,
            notice: String::new(),
            last_beat: None,
            audio_status: "Opening audio output…".into(),
            taps: VecDeque::new(),
        }
    }
}

impl App {
    pub fn settings(&self) -> Settings {
        Settings {
            bpm: self.bpm,
            beats_per_bar: self.beats_per_bar,
            playing: self.playing,
            muted: self.muted,
            subdivision: self.subdivision,
            volumes: self.volumes,
            voice: self.voice,
            training: self.training,
            pattern: self.pattern,
        }
    }

    pub fn adjust_bpm(&mut self, delta: i16) {
        self.bpm = (self.bpm as i16 + delta).clamp(MIN_BPM as i16, MAX_BPM as i16) as u16;
        self.taps.clear();
    }

    pub fn tap(&mut self, now: Instant) {
        if self
            .taps
            .back()
            .is_some_and(|previous| now.duration_since(*previous) > Duration::from_secs(2))
        {
            self.taps.clear();
        }
        self.taps.push_back(now);
        if self.taps.len() > 6 {
            self.taps.pop_front();
        }
        if self.taps.len() >= 2 {
            let elapsed = now
                .duration_since(*self.taps.front().unwrap())
                .as_secs_f64();
            if elapsed > 0.0 {
                let bpm = (60.0 * (self.taps.len() - 1) as f64 / elapsed).round();
                self.bpm = bpm.clamp(f64::from(MIN_BPM), f64::from(MAX_BPM)) as u16;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_points_use_explicit_accents_and_position_based_normal_volumes() {
        let settings = App::default().settings();
        assert_eq!(settings.step_gain(Step::Off, 0), 0.0);
        assert_eq!(
            settings.step_gain(Step::Accent, 3),
            settings.volumes.gain(0, 0)
        );
        assert_eq!(
            settings.step_gain(Step::Normal, 0),
            settings.volumes.gain(1, 0)
        );
        assert_eq!(
            settings.step_gain(Step::Normal, 3),
            settings.volumes.gain(1, 3)
        );
    }
    #[test]
    fn tempo_is_clamped() {
        let mut app = App::default();
        app.adjust_bpm(-200);
        assert_eq!(app.bpm, MIN_BPM);
        app.adjust_bpm(500);
        assert_eq!(app.bpm, MAX_BPM);
    }

    #[test]
    fn interval_at_120_bpm_is_half_a_second() {
        assert_eq!(
            App::default().settings().interval(),
            Duration::from_millis(500)
        );
    }

    #[test]
    fn tap_tempo_averages_recent_intervals() {
        let mut app = App::default();
        let start = Instant::now();
        for index in 0..8 {
            app.tap(start + Duration::from_millis(index * 400));
        }
        assert_eq!(app.bpm, 150);
        assert_eq!(app.taps.len(), 6);
    }

    #[test]
    fn tap_sequence_restarts_after_a_pause() {
        let mut app = App::default();
        let start = Instant::now();
        app.tap(start);
        app.tap(start + Duration::from_millis(400));
        app.tap(start + Duration::from_secs(4));
        assert_eq!(app.taps.len(), 1);
        assert_eq!(app.bpm, 150);
    }
}
