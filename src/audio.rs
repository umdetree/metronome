use crate::app::{App, Settings, Voice};
use crate::pattern::Step;
use rodio::{OutputStream, Sink, Source, buffer::SamplesBuffer};
use std::{
    sync::mpsc::{self, Receiver, Sender},
    thread::{self, JoinHandle},
    time::Instant,
};

pub enum EngineEvent {
    Tick {
        beat: u8,
        sub: u8,
        at: Instant,
        settings: Settings,
        silent: bool,
        cycle_bar: u8,
        pattern_bar: u8,
    },
    AudioStatus(String),
}
enum Command {
    Update(Settings, bool),
    Shutdown,
}

pub struct Engine {
    commands: Sender<Command>,
    pub events: Receiver<EngineEvent>,
    worker: Option<JoinHandle<()>>,
}
impl Engine {
    pub fn start() -> Self {
        let (commands, receiver) = mpsc::channel();
        let (sender, events) = mpsc::channel();
        let worker = thread::spawn(move || run(receiver, sender));
        Self {
            commands,
            events,
            worker: Some(worker),
        }
    }
    pub fn update(&self, settings: Settings, reset: bool) {
        let _ = self.commands.send(Command::Update(settings, reset));
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

// Pure scheduler: rhythm changes wait for a bar boundary; other controls are immediate.
struct Clock {
    active: Settings,
    requested: Settings,
    beat: u8,
    sub: u8,
    next: Instant,
    cycle_bar: u8,
    pattern_bar: u8,
}
impl Clock {
    fn new(now: Instant) -> Self {
        let settings = App::default().settings();
        Self {
            active: settings,
            requested: settings,
            beat: 0,
            sub: 0,
            next: now,
            cycle_bar: 0,
            pattern_bar: 0,
        }
    }
    fn update(&mut self, settings: Settings, reset: bool, now: Instant) {
        if reset || !self.active.playing {
            self.active = settings;
            self.beat = 0;
            self.sub = 0;
            self.next = now;
            self.cycle_bar = 0;
            self.pattern_bar = 0;
        } else {
            if settings.bpm != self.active.bpm {
                let fraction = (self.next.saturating_duration_since(now).as_secs_f64()
                    / self.active.tick_interval().as_secs_f64())
                .min(1.0);
                self.next = now + settings.tick_interval_for(self.active).mul_f64(fraction);
            }
            self.active.bpm = settings.bpm;
            self.active.playing = settings.playing;
            self.active.muted = settings.muted;
            self.active.volumes = settings.volumes;
            self.active.voice = settings.voice;
        }
        self.requested = settings;
    }
    fn tick(&mut self, now: Instant) -> EngineEvent {
        if self.beat == 0 && self.sub == 0 {
            self.active.beats_per_bar = self.requested.beats_per_bar;
            self.active.subdivision = self.requested.subdivision;
            let previous = self.active.pattern;
            self.active.pattern = self.requested.pattern;
            if previous.enabled != self.active.pattern.enabled
                || previous.bars != self.active.pattern.bars
                || previous.beats != self.active.pattern.beats
                || previous.density != self.active.pattern.density
            {
                self.pattern_bar = 0;
            }
            if self.active.training != self.requested.training {
                self.active.training = self.requested.training;
                self.cycle_bar = 0;
            }
        }
        let event = EngineEvent::Tick {
            beat: self.beat,
            sub: self.sub,
            at: now,
            settings: self.active,
            silent: self.active.training.is_silent(self.cycle_bar),
            cycle_bar: self.cycle_bar,
            pattern_bar: self.pattern_bar,
        };
        self.sub += 1;
        if self.sub == self.active.points_per_beat() {
            self.sub = 0;
            self.beat = (self.beat + 1) % self.active.bar_beats();
            if self.beat == 0 {
                self.cycle_bar = (self.cycle_bar + 1) % self.active.training.cycle_length();
                self.pattern_bar = (self.pattern_bar + 1) % self.active.loop_bars();
            }
        }
        self.next += self.active.tick_interval();
        if self.next <= now {
            // Skip stale deadlines rather than producing a catch-up burst.
            self.next = now + self.active.tick_interval();
        }
        event
    }
}

impl Settings {
    fn tick_interval_for(self, active: Self) -> std::time::Duration {
        Self {
            subdivision: active.subdivision,
            pattern: active.pattern,
            ..self
        }
        .tick_interval()
    }
}

fn run(commands: Receiver<Command>, events: Sender<EngineEvent>) {
    // The output stream must live on the playback thread until shutdown.
    let audio = OutputStream::try_default()
        .map_err(|error| error.to_string())
        .and_then(|(stream, handle)| {
            Sink::try_new(&handle)
                .map(|sink| (stream, sink))
                .map_err(|error| error.to_string())
        });
    let status = match &audio {
        Ok(_) => "Audio ready".into(),
        Err(error) => format!("Visual only · audio unavailable: {error}"),
    };
    let _ = events.send(EngineEvent::AudioStatus(status));
    let sounds: Vec<_> = Voice::ALL
        .into_iter()
        .map(|voice| [click(voice, true), click(voice, false)])
        .collect();
    let mut clock = Clock::new(Instant::now());
    loop {
        let command = if clock.active.playing {
            match commands.recv_timeout(clock.next.saturating_duration_since(Instant::now())) {
                Ok(command) => Some(command),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match commands.recv() {
                Ok(command) => Some(command),
                Err(_) => break,
            }
        };
        match command {
            Some(Command::Shutdown) => break,
            Some(Command::Update(settings, reset)) => {
                clock.update(settings, reset, Instant::now());
                if let Ok((_, sink)) = &audio {
                    configure_sink(sink, settings);
                }
            }
            None => {
                let event = clock.tick(Instant::now());
                if let EngineEvent::Tick {
                    beat,
                    sub,
                    settings,
                    silent: false,
                    pattern_bar,
                    ..
                } = &event
                    && let Ok((_, sink)) = &audio
                {
                    let voice = Voice::ALL
                        .iter()
                        .position(|voice| *voice == settings.voice)
                        .unwrap();
                    let step = settings.step(*pattern_bar, *beat, *sub);
                    let sound = sounds[voice][usize::from(step != Step::Accent)].clone();
                    let relative = settings.step_gain(step, *sub);
                    let master = f32::from(settings.volumes.master) / 100.0;
                    if step != Step::Off {
                        // A sink plays sources sequentially: clicks must fit a grid tick.
                        let duration = settings
                            .tick_interval()
                            .min(sound.total_duration().unwrap());
                        sink.append(
                            sound
                                .take_duration(duration)
                                .fade_out(duration)
                                .amplify(if master > 0.0 { relative / master } else { 0.0 }),
                        );
                    }
                }
                if events.send(event).is_err() {
                    break;
                }
            }
        }
    }
}

fn configure_sink(sink: &Sink, settings: Settings) {
    // clear() also pauses the sink; appending sources alone does not resume it.
    if settings.playing {
        sink.play();
    } else {
        sink.clear();
    }
    // Sink-level master gain also silences an already queued click.
    sink.set_volume(if settings.muted {
        0.0
    } else {
        f32::from(settings.volumes.master) / 100.0
    });
}

fn click(voice: Voice, accent: bool) -> SamplesBuffer<f32> {
    let sample_rate = 44_100;
    let (frequency, decay) = match voice {
        Voice::Crisp => (if accent { 1760.0 } else { 1320.0 }, 150.0),
        Voice::Soft => (if accent { 880.0 } else { 660.0 }, 100.0),
        Voice::Wood => (if accent { 1200.0 } else { 900.0 }, 180.0),
    };
    let samples: Vec<f32> = (0..1543)
        .map(|index| {
            let time = index as f32 / sample_rate as f32;
            let envelope =
                (time / 0.001).min(1.0) * (-time * decay).exp() * (1.0 - index as f32 / 1543.0);
            let phase = std::f32::consts::TAU * frequency * time;
            let wave = if voice == Voice::Wood {
                phase.sin() * 0.7 + (phase * 2.7).sin() * 0.3
            } else {
                phase.sin()
            };
            wave * envelope * 0.45
        })
        .collect();
    SamplesBuffer::new(1, sample_rate, samples)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Subdivision;
    use std::time::Duration;

    #[test]
    fn pattern_rests_preserve_time_and_multiple_bars_loop() {
        for density in crate::pattern::DENSITIES {
            let now = Instant::now();
            let mut clock = Clock::new(now);
            let mut pattern = crate::pattern::Pattern::default();
            pattern.resize(3, 2, density);
            pattern.enabled = true;
            for bar in 0..3 {
                pattern.clear_bar(bar);
            }
            pattern.set(0, 0, 0, Step::Accent);
            pattern.set(2, 1, density - 1, Step::Normal);
            clock.update(
                Settings {
                    playing: true,
                    pattern,
                    ..clock.active
                },
                true,
                now,
            );
            let mut sounded = 0;
            for index in 0..(3 * 2 * u16::from(density)) {
                let deadline = clock.next;
                let EngineEvent::Tick {
                    pattern_bar,
                    beat,
                    sub,
                    settings,
                    ..
                } = clock.tick(deadline)
                else {
                    panic!()
                };
                assert_eq!(u16::from(pattern_bar), index / (2 * u16::from(density)));
                assert_eq!(u16::from(beat), index / u16::from(density) % 2);
                assert_eq!(u16::from(sub), index % u16::from(density));
                if settings.step(pattern_bar, beat, sub) != Step::Off {
                    sounded += 1;
                }
            }
            assert_eq!(sounded, 2);
            assert_eq!(clock.pattern_bar, 0);
            assert!(
                clock
                    .next
                    .duration_since(now)
                    .abs_diff(Duration::from_secs(3))
                    < Duration::from_nanos(100)
            );
        }
    }
    #[test]
    fn live_point_edits_wait_for_next_bar_without_restarting_pattern_loop() {
        let now = Instant::now();
        let mut clock = Clock::new(now);
        let mut settings = clock.active;
        settings.playing = true;
        settings.pattern.enabled = true;
        settings.pattern.resize(2, 1, 2);
        clock.update(settings, true, now);
        clock.tick(now);
        settings.pattern.set(1, 0, 0, Step::Accent);
        clock.update(settings, false, now);
        assert_eq!(clock.active.pattern.get(1, 0, 0), Step::Off);
        clock.tick(clock.next);
        let EngineEvent::Tick {
            pattern_bar,
            settings,
            ..
        } = clock.tick(clock.next)
        else {
            panic!()
        };
        assert_eq!(pattern_bar, 1);
        assert_eq!(settings.step(pattern_bar, 0, 0), Step::Accent);
    }
    #[test]
    fn pattern_and_training_cycles_advance_independently() {
        let now = Instant::now();
        let mut clock = Clock::new(now);
        let mut settings = clock.active;
        settings.playing = true;
        settings.pattern.enabled = true;
        settings.pattern.resize(3, 1, 1);
        settings.training = crate::app::Training {
            sound_bars: 1,
            silent_bars: 1,
        };
        clock.update(settings, true, now);
        for index in 0..12 {
            let EngineEvent::Tick {
                pattern_bar,
                silent,
                ..
            } = clock.tick(clock.next)
            else {
                panic!()
            };
            assert_eq!(pattern_bar, index % 3);
            assert_eq!(silent, index % 2 == 1);
        }
    }
    #[test]
    fn mode_switch_waits_for_bar_boundary_and_restores_normal_settings() {
        let now = Instant::now();
        let mut clock = Clock::new(now);
        let mut settings = clock.active;
        settings.playing = true;
        settings.pattern.enabled = true;
        settings.pattern.resize(2, 1, 2);
        clock.update(settings, true, now);
        clock.tick(now);
        settings.pattern.enabled = false;
        clock.update(settings, false, now);
        assert!(clock.active.pattern.enabled);
        clock.tick(clock.next);
        clock.tick(clock.next);
        assert!(!clock.active.pattern.enabled);
        assert_eq!(clock.active.bar_beats(), 4);
        assert_eq!(clock.active.points_per_beat(), 1);
    }
    #[test]
    fn high_density_clicks_fit_between_ticks() {
        let mut settings = App::default().settings();
        settings.bpm = 300;
        settings.pattern.enabled = true;
        settings.pattern.resize(1, 4, 12);
        let duration = settings.tick_interval();
        let sound = click(Voice::Crisp, false)
            .take_duration(duration)
            .fade_out(duration);
        let samples: Vec<_> = sound.collect();
        let actual = Duration::from_secs_f64(samples.len() as f64 / 44_100.0);
        assert!(actual.abs_diff(duration) < Duration::from_micros(30));
        assert!(samples.last().unwrap().abs() < 0.001);
    }
    #[test]
    fn silent_bar_cycles_keep_every_visual_tick_and_deadline() {
        for subdivision in Subdivision::ALL {
            let now = Instant::now();
            let mut clock = Clock::new(now);
            let settings = Settings {
                playing: true,
                subdivision,
                beats_per_bar: 2,
                training: crate::app::Training {
                    sound_bars: 2,
                    silent_bars: 2,
                },
                ..clock.active
            };
            clock.update(settings, true, now);
            for bar in 0..12 {
                for tick in 0..(2 * subdivision.count()) {
                    let deadline = clock.next;
                    let EngineEvent::Tick {
                        beat,
                        sub,
                        silent,
                        cycle_bar,
                        ..
                    } = clock.tick(deadline)
                    else {
                        panic!()
                    };
                    assert_eq!(beat, tick / subdivision.count());
                    assert_eq!(sub, tick % subdivision.count());
                    assert_eq!(cycle_bar, bar % 4);
                    assert_eq!(silent, bar % 4 >= 2);
                    assert!(clock.next > deadline);
                }
            }
            assert!(
                clock
                    .next
                    .duration_since(now)
                    .abs_diff(Duration::from_secs(12))
                    < Duration::from_nanos(100)
            );
        }
    }
    #[test]
    fn training_changes_wait_until_next_bar_then_restart_sound_phase() {
        let now = Instant::now();
        let mut clock = Clock::new(now);
        clock.update(
            Settings {
                playing: true,
                beats_per_bar: 2,
                training: crate::app::Training {
                    sound_bars: 1,
                    silent_bars: 2,
                },
                ..clock.active
            },
            true,
            now,
        );
        for _ in 0..3 {
            clock.tick(clock.next);
        }
        clock.update(
            Settings {
                training: crate::app::Training {
                    sound_bars: 3,
                    silent_bars: 1,
                },
                ..clock.active
            },
            false,
            clock.next,
        );
        let EngineEvent::Tick { silent, .. } = clock.tick(clock.next) else {
            panic!()
        };
        assert!(silent);
        let EngineEvent::Tick {
            silent,
            cycle_bar,
            settings,
            ..
        } = clock.tick(clock.next)
        else {
            panic!()
        };
        assert!(!silent);
        assert_eq!(cycle_bar, 0);
        assert_eq!(settings.training.sound_bars, 3);
    }
    #[test]
    fn disabled_training_never_silences_and_restart_resets_cycle() {
        let now = Instant::now();
        let mut clock = Clock::new(now);
        let settings = Settings {
            playing: true,
            beats_per_bar: 1,
            ..clock.active
        };
        clock.update(settings, true, now);
        for _ in 0..20 {
            let EngineEvent::Tick { silent, .. } = clock.tick(clock.next) else {
                panic!()
            };
            assert!(!silent);
        }
        let settings = Settings {
            training: crate::app::Training {
                sound_bars: 1,
                silent_bars: 1,
            },
            ..settings
        };
        clock.update(settings, true, now);
        clock.tick(clock.next);
        let EngineEvent::Tick { silent, .. } = clock.tick(clock.next) else {
            panic!()
        };
        assert!(silent);
        clock.update(settings, true, now);
        let EngineEvent::Tick {
            silent, cycle_bar, ..
        } = clock.tick(clock.next)
        else {
            panic!()
        };
        assert!(!silent);
        assert_eq!(cycle_bar, 0);
    }
    #[test]
    fn playback_resumes_after_paused_settings_and_stop() {
        // No audio hardware needed to exercise Rodio's pause state.
        let (sink, _queue) = Sink::new_idle();
        let mut settings = App::default().settings();
        configure_sink(&sink, settings);
        assert!(sink.is_paused());
        settings.bpm = 132;
        configure_sink(&sink, settings);
        settings.playing = true;
        configure_sink(&sink, settings);
        assert!(!sink.is_paused());
        assert_eq!(sink.volume(), 0.8);
        settings.playing = false;
        configure_sink(&sink, settings);
        assert!(sink.is_paused());
        settings.playing = true;
        settings.muted = true;
        configure_sink(&sink, settings);
        assert!(!sink.is_paused());
        assert_eq!(sink.volume(), 0.0);
        settings.muted = false;
        configure_sink(&sink, settings);
        assert_eq!(sink.volume(), 0.8);
    }
    #[test]
    fn clicks_are_short_mono_and_bounded() {
        for voice in Voice::ALL {
            for accent in [false, true] {
                let sound = click(voice, accent);
                assert_eq!(sound.channels(), 1);
                assert_eq!(sound.sample_rate(), 44_100);
                let samples: Vec<_> = sound.collect();
                assert_eq!(samples.len(), 1543);
                assert!(
                    samples
                        .iter()
                        .all(|sample| sample.is_finite() && sample.abs() <= 0.45)
                );
                assert!(samples.iter().any(|sample| sample.abs() > 0.1));
            }
        }
    }
    #[test]
    fn all_subdivisions_preserve_quarter_note_tempo() {
        for subdivision in Subdivision::ALL {
            let now = Instant::now();
            let mut clock = Clock::new(now);
            let settings = Settings {
                playing: true,
                subdivision,
                beats_per_bar: 3,
                ..clock.active
            };
            clock.update(settings, true, now);
            for index in 0..(3 * subdivision.count()) {
                let deadline = clock.next;
                let EngineEvent::Tick { beat, sub, .. } = clock.tick(deadline) else {
                    panic!()
                };
                assert_eq!(beat, index / subdivision.count());
                assert_eq!(sub, index % subdivision.count());
            }
            assert_eq!((clock.beat, clock.sub), (0, 0));
            assert!(
                clock
                    .next
                    .duration_since(now)
                    .abs_diff(Duration::from_millis(1500))
                    < Duration::from_nanos(10)
            );
        }
    }
    #[test]
    fn rhythm_changes_wait_until_next_bar() {
        let now = Instant::now();
        let mut clock = Clock::new(now);
        clock.update(
            Settings {
                playing: true,
                ..clock.active
            },
            true,
            now,
        );
        clock.tick(now);
        clock.update(
            Settings {
                beats_per_bar: 3,
                subdivision: Subdivision::Triplet,
                ..clock.active
            },
            false,
            now,
        );
        for _ in 0..3 {
            clock.tick(clock.next);
            assert_eq!(clock.active.beats_per_bar, 4);
        }
        clock.tick(clock.next);
        assert_eq!(clock.active.beats_per_bar, 3);
        assert_eq!(clock.active.subdivision, Subdivision::Triplet);
    }
    #[test]
    fn tempo_change_keeps_fraction_until_next_tick() {
        let now = Instant::now();
        let mut clock = Clock::new(now);
        clock.update(
            Settings {
                playing: true,
                ..clock.active
            },
            true,
            now,
        );
        clock.tick(now);
        clock.update(
            Settings {
                bpm: 240,
                ..clock.active
            },
            false,
            now + Duration::from_millis(250),
        );
        assert_eq!(clock.next.duration_since(now), Duration::from_millis(375));
    }
    #[test]
    fn stalled_clock_does_not_emit_catch_up_burst() {
        let now = Instant::now();
        let mut clock = Clock::new(now);
        let later = now + Duration::from_secs(10);
        clock.tick(later);
        assert!(clock.next > later);
    }
    #[test]
    fn restarting_applies_pending_rhythm_and_returns_to_first_beat() {
        let now = Instant::now();
        let mut clock = Clock::new(now);
        clock.update(
            Settings {
                playing: true,
                ..clock.active
            },
            true,
            now,
        );
        clock.tick(now);
        let requested = Settings {
            subdivision: Subdivision::Sixteenth,
            beats_per_bar: 7,
            ..clock.active
        };
        clock.update(requested, false, now);
        assert_eq!(clock.active.subdivision, Subdivision::Quarter);
        clock.update(
            Settings {
                playing: false,
                ..requested
            },
            true,
            now,
        );
        clock.update(requested, true, now);
        let EngineEvent::Tick {
            beat,
            sub,
            settings,
            ..
        } = clock.tick(now)
        else {
            panic!()
        };
        assert_eq!((beat, sub), (0, 0));
        assert_eq!(settings.subdivision, Subdivision::Sixteenth);
        assert_eq!(settings.beats_per_bar, 7);
    }
    #[test]
    fn latest_pending_rhythm_wins_without_delaying_volume_changes() {
        let now = Instant::now();
        let mut clock = Clock::new(now);
        clock.update(
            Settings {
                playing: true,
                ..clock.active
            },
            true,
            now,
        );
        clock.tick(now);
        clock.update(
            Settings {
                subdivision: Subdivision::Triplet,
                ..clock.active
            },
            false,
            now,
        );
        let mut requested = Settings {
            subdivision: Subdivision::Eighth,
            ..clock.requested
        };
        requested.volumes.subdivision = 0;
        clock.update(requested, false, now);
        assert_eq!(clock.active.volumes.subdivision, 0);
        for _ in 0..4 {
            clock.tick(clock.next);
        }
        assert_eq!(clock.active.subdivision, Subdivision::Eighth);
    }
    #[test]
    fn volume_layers_and_silent_subdivisions() {
        let mut volumes = App::default().volumes;
        assert!(volumes.gain(0, 0) > volumes.gain(1, 0));
        assert!(volumes.gain(1, 0) > volumes.gain(1, 1));
        volumes.subdivision = 0;
        assert_eq!(volumes.gain(0, 1), 0.0);
        volumes.master = 0;
        assert_eq!(volumes.gain(0, 0), 0.0);
    }
}
