use crate::app::{App, MAX_BPM, MIN_BPM, Subdivision, Training, Voice, Volumes};
use crate::pattern::Pattern;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    version: u16,
    bpm: u16,
    beats_per_bar: u8,
    subdivision: Subdivision,
    voice: Voice,
    volumes: Volumes,
    training: Training,
    pattern: Pattern,
}

impl Default for Config {
    fn default() -> Self {
        Self::from_app(&App::default())
    }
}

impl Config {
    pub fn from_app(app: &App) -> Self {
        Self {
            version: 1,
            bpm: app.bpm,
            beats_per_bar: app.beats_per_bar,
            subdivision: app.subdivision,
            voice: app.voice,
            volumes: app.volumes,
            training: app.training,
            pattern: app.pattern,
        }
    }
    fn validate(&self) -> Result<(), String> {
        if self.version != 1 {
            return Err("unsupported config version".into());
        }
        if !(MIN_BPM..=MAX_BPM).contains(&self.bpm) {
            return Err("bpm must be 30–300".into());
        }
        if !(1..=12).contains(&self.beats_per_bar) {
            return Err("beats_per_bar must be 1–12".into());
        }
        if [
            self.volumes.master,
            self.volumes.accent,
            self.volumes.beat,
            self.volumes.subdivision,
        ]
        .into_iter()
        .any(|volume| volume > 100)
        {
            return Err("volumes must be 0–100".into());
        }
        if !(1..=32).contains(&self.training.sound_bars) || self.training.silent_bars > 32 {
            return Err("sound_bars must be 1–32; silent_bars must be 0–32".into());
        }
        self.pattern.validate()?;
        Ok(())
    }
    fn apply(&self, app: &mut App) {
        app.bpm = self.bpm;
        app.beats_per_bar = self.beats_per_bar;
        app.subdivision = self.subdivision;
        app.voice = self.voice;
        app.volumes = self.volumes;
        app.training = self.training;
        app.pattern = self.pattern;
        app.playing = false;
        app.muted = false;
        app.active = app.settings();
    }
}

fn load(path: &Path) -> Result<Option<Config>, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let config: Config = toml::from_str(&text).map_err(|error| error.to_string())?;
    config.validate()?;
    Ok(Some(config))
}

fn save(path: &Path, config: &Config) -> Result<(), String> {
    config.validate()?;
    let text = toml::to_string_pretty(config).map_err(|error| error.to_string())?;
    let parent = path.parent().ok_or("config path has no parent")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    // Same-directory temporary file + atomic replacement, including on Windows.
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    file.write_all(text.as_bytes())
        .map_err(|error| error.to_string())?;
    file.as_file()
        .sync_all()
        .map_err(|error| error.to_string())?;
    file.persist(path).map_err(|error| error.to_string())?;
    Ok(())
}

pub struct Store {
    path: Option<PathBuf>,
    observed: Config,
    saved: Option<Config>,
    dirty_since: Option<Instant>,
    blocked: bool,
}

impl Store {
    pub fn open(app: &mut App) -> Self {
        let path = ProjectDirs::from("", "", "metronome")
            .map(|dirs| dirs.config_dir().join("config.toml"));
        Self::open_path(app, path, Instant::now())
    }
    fn open_path(app: &mut App, path: Option<PathBuf>, now: Instant) -> Self {
        let mut saved = None;
        let mut blocked = false;
        if let Some(path) = &path {
            match load(path) {
                Ok(config) => {
                    if let Some(config) = &config {
                        config.apply(app);
                    }
                    saved = config;
                    app.config_status = format!("Config: {}", path.display());
                }
                Err(error) => {
                    blocked = true;
                    app.config_status = format!(
                        "Config error: {error} · saving disabled; fix {} and restart",
                        path.display()
                    );
                }
            }
        } else {
            blocked = true;
            app.config_status = "Config unavailable: cannot locate user config directory".into();
        }
        Self {
            path,
            observed: Config::from_app(app),
            dirty_since: if saved.is_none() && !blocked {
                Some(now)
            } else {
                None
            },
            saved,
            blocked,
        }
    }
    pub fn observe(&mut self, app: &App, now: Instant) {
        let config = Config::from_app(app);
        if config != self.observed {
            self.observed = config;
            self.dirty_since = Some(now);
        }
    }
    pub fn flush(&mut self, app: &mut App, now: Instant, force: bool) {
        if self.blocked || self.saved.as_ref() == Some(&self.observed) {
            return;
        }
        if !force
            && !self
                .dirty_since
                .is_some_and(|since| now.duration_since(since) >= Duration::from_millis(500))
        {
            return;
        }
        self.dirty_since = None;
        let Some(path) = &self.path else {
            return;
        };
        match save(path, &self.observed) {
            Ok(()) => {
                self.saved = Some(self.observed.clone());
                app.config_status = format!("Config saved: {}", path.display());
            }
            Err(error) => {
                app.config_status = format!("Config save failed: {error} · {}", path.display())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saves_and_atomically_replaces_then_restores_only_preferences() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let mut app = App {
            bpm: 132,
            playing: true,
            muted: true,
            subdivision: Subdivision::Triplet,
            training: Training {
                sound_bars: 3,
                silent_bars: 2,
            },
            ..App::default()
        };
        save(&path, &Config::from_app(&app)).unwrap();
        app.bpm = 144;
        save(&path, &Config::from_app(&app)).unwrap();
        let mut restored = App::default();
        load(&path).unwrap().unwrap().apply(&mut restored);
        assert_eq!(restored.bpm, 144);
        assert_eq!(restored.subdivision, Subdivision::Triplet);
        assert_eq!(restored.training, app.training);
        assert!(!restored.playing && !restored.muted);
        let text = fs::read_to_string(path).unwrap();
        assert!(!text.contains("playing") && !text.contains("muted"));
    }
    #[test]
    fn pattern_preferences_round_trip_without_editor_cursor() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        let mut app = App::default();
        app.pattern.enabled = true;
        app.pattern.resize(4, 3, 12);
        app.pattern.set(3, 2, 11, crate::pattern::Step::Accent);
        save(&path, &Config::from_app(&app)).unwrap();
        let mut restored = App::default();
        load(&path).unwrap().unwrap().apply(&mut restored);
        assert_eq!(restored.pattern, app.pattern);
        assert!(restored.editor.is_none());
        let old: Config = toml::from_str("version = 1\nbpm = 132").unwrap();
        assert_eq!(old.pattern, Pattern::default());
    }
    #[test]
    fn invalid_config_is_preserved_and_does_not_disable_app() {
        for text in [
            "bpm = 0",
            "version = 2",
            "[training]\nsound_bars = 0",
            "[volumes]\nmaster = 101",
            "[pattern]\ndensity = 5",
            "[pattern]\nrows = [\"invalid\"]",
            "not toml!",
        ] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("config.toml");
            fs::write(&path, text).unwrap();
            let now = Instant::now();
            let mut app = App::default();
            let mut store = Store::open_path(&mut app, Some(path.clone()), now);
            assert_eq!(app.bpm, 120);
            assert!(app.config_status.contains("saving disabled"));
            app.bpm = 150;
            store.observe(&app, now);
            store.flush(&mut app, now, true);
            assert_eq!(fs::read_to_string(path).unwrap(), text);
        }
    }
    #[test]
    fn autosave_debounces_and_exit_flushes_pending_changes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/config.toml");
        let now = Instant::now();
        let mut app = App::default();
        let mut store = Store::open_path(&mut app, Some(path.clone()), now);
        store.flush(&mut app, now, false);
        assert!(!path.exists());
        store.flush(&mut app, now + Duration::from_millis(500), false);
        assert!(path.exists());
        app.bpm = 132;
        store.observe(&app, now + Duration::from_secs(1));
        store.flush(&mut app, now + Duration::from_millis(1200), false);
        assert_eq!(load(&path).unwrap().unwrap().bpm, 120);
        store.flush(&mut app, now + Duration::from_millis(1200), true);
        assert_eq!(load(&path).unwrap().unwrap().bpm, 132);
    }
    #[test]
    fn partial_config_uses_defaults_and_save_failure_is_visible() {
        let config: Config = toml::from_str("bpm = 140").unwrap();
        config.validate().unwrap();
        assert_eq!(config.training, Training::default());
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("file");
        fs::write(&file, "not a directory").unwrap();
        let now = Instant::now();
        let mut app = App::default();
        let mut store = Store::open_path(&mut app, Some(directory.path().join("valid.toml")), now);
        store.path = Some(file.join("config.toml"));
        store.flush(&mut app, now, true);
        assert!(app.config_status.contains("save failed"));
    }
}
