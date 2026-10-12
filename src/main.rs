mod app;
mod audio;
mod config;
mod grid;
mod input;
mod input_view;
mod monitor;
mod pattern;
mod ui;

use std::{
    io,
    time::{Duration, Instant},
};

use app::App;
use crossterm::event::{self, Event};

fn main() -> io::Result<()> {
    let engine = audio::Engine::start();
    let monitor = monitor::Monitor::start();
    let mut app = App::default();
    let mut config = config::Store::open(&mut app);
    engine.update(app.settings(), true);
    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app, &engine, &mut config, &monitor);
    ratatui::restore();
    config.observe(&app, Instant::now());
    config.flush(&mut app, Instant::now(), true);
    if app.config_status.starts_with("Config error")
        || app.config_status.starts_with("Config save failed")
        || app.config_status.starts_with("Config unavailable")
    {
        eprintln!("{}", app.config_status);
    }
    result
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    engine: &audio::Engine,
    config: &mut config::Store,
    monitor: &monitor::Monitor,
) -> io::Result<()> {
    loop {
        if let Some(snapshot) = monitor.snapshot() {
            app.input_snapshot = snapshot;
        }
        while let Ok(message) = engine.events.try_recv() {
            match message {
                audio::EngineEvent::Tick {
                    beat,
                    sub,
                    at,
                    settings,
                    silent,
                    cycle_bar,
                    pattern_bar,
                } if app.playing => {
                    app.beat = beat;
                    app.sub = sub;
                    app.active = settings;
                    app.silent_bar = silent;
                    app.cycle_bar = cycle_bar;
                    app.pattern_bar = pattern_bar;
                    app.last_beat = Some(at);
                }
                audio::EngineEvent::AudioStatus(status) => app.audio_status = status,
                _ => {}
            }
        }
        config.observe(app, Instant::now());
        config.flush(app, Instant::now(), false);
        terminal.draw(|frame| ui::draw(frame, app))?;
        if !event::poll(Duration::from_millis(if app.input_view { 33 } else { 16 }))? {
            continue;
        }
        let Event::Key(key) = event::read()? else {
            continue;
        };
        let before = app.settings();
        let action = input::handle(app, key);
        if let Some(command) = action.monitor {
            monitor.send(command);
        }
        if action.quit {
            return Ok(());
        }
        if before != app.settings() || action.reset {
            engine.update(app.settings(), action.reset);
        }
    }
}
