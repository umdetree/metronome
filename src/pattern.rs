use serde::{Deserialize, Serialize};

pub const DENSITIES: [u8; 6] = [1, 2, 3, 4, 6, 12];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Step {
    #[default]
    Off,
    Normal,
    Accent,
}

impl Step {
    pub fn symbol(self) -> char {
        match self {
            Self::Off => '.',
            Self::Normal => 'x',
            Self::Accent => 'X',
        }
    }
}

// Two bits per point, with fixed strides: editing dimensions never changes indexing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "StoredPattern", into = "StoredPattern")]
pub struct Pattern {
    pub enabled: bool,
    pub bars: u8,
    pub beats: u8,
    pub density: u8,
    cells: [u8; 144],
}

impl Default for Pattern {
    fn default() -> Self {
        let mut pattern = Self {
            enabled: false,
            bars: 1,
            beats: 4,
            density: 4,
            cells: [0; 144],
        };
        for beat in 0..4 {
            pattern.set(
                0,
                beat,
                0,
                if beat == 0 {
                    Step::Accent
                } else {
                    Step::Normal
                },
            );
        }
        pattern
    }
}

impl Pattern {
    pub fn validate(self) -> Result<(), String> {
        if !(1..=4).contains(&self.bars)
            || !(1..=12).contains(&self.beats)
            || !DENSITIES.contains(&self.density)
        {
            return Err("pattern: bars 1–4, beats 1–12, density 1/2/3/4/6/12".into());
        }
        Ok(())
    }
    fn index(bar: u8, beat: u8, sub: u8) -> usize {
        assert!(bar < 4 && beat < 12 && sub < 12);
        usize::from(bar) * 144 + usize::from(beat) * 12 + usize::from(sub)
    }
    pub fn get(self, bar: u8, beat: u8, sub: u8) -> Step {
        let index = Self::index(bar, beat, sub);
        match (self.cells[index / 4] >> ((index % 4) * 2)) & 3 {
            1 => Step::Normal,
            2 => Step::Accent,
            _ => Step::Off,
        }
    }
    pub fn set(&mut self, bar: u8, beat: u8, sub: u8, step: Step) {
        let index = Self::index(bar, beat, sub);
        let shift = (index % 4) * 2;
        let value = match step {
            Step::Off => 0,
            Step::Normal => 1,
            Step::Accent => 2,
        };
        self.cells[index / 4] = (self.cells[index / 4] & !(3 << shift)) | (value << shift);
    }
    pub fn resize(&mut self, bars: u8, beats: u8, density: u8) {
        let mut next = Self {
            enabled: self.enabled,
            bars,
            beats,
            density,
            cells: [0; 144],
        };
        assert!(next.validate().is_ok());
        // Preserve only points that still fall at exactly the same musical position.
        for bar in 0..bars.min(self.bars) {
            for beat in 0..beats.min(self.beats) {
                for sub in 0..self.density {
                    let scaled = sub * density;
                    if scaled.is_multiple_of(self.density) {
                        next.set(bar, beat, scaled / self.density, self.get(bar, beat, sub));
                    }
                }
            }
        }
        *self = next;
    }
    pub fn clear_bar(&mut self, bar: u8) {
        for beat in 0..self.beats {
            for sub in 0..self.density {
                self.set(bar, beat, sub, Step::Off);
            }
        }
    }
    pub fn copy_bar(&mut self, from: u8, to: u8) {
        for beat in 0..self.beats {
            for sub in 0..self.density {
                self.set(to, beat, sub, self.get(from, beat, sub));
            }
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct StoredPattern {
    enabled: bool,
    bars: u8,
    beats: u8,
    density: u8,
    rows: Vec<String>,
}

impl Default for StoredPattern {
    fn default() -> Self {
        Pattern::default().into()
    }
}

impl From<Pattern> for StoredPattern {
    fn from(pattern: Pattern) -> Self {
        let rows = (0..pattern.bars)
            .map(|bar| {
                (0..pattern.beats)
                    .flat_map(|beat| {
                        (0..pattern.density).map(move |sub| pattern.get(bar, beat, sub).symbol())
                    })
                    .collect()
            })
            .collect();
        Self {
            enabled: pattern.enabled,
            bars: pattern.bars,
            beats: pattern.beats,
            density: pattern.density,
            rows,
        }
    }
}

impl TryFrom<StoredPattern> for Pattern {
    type Error = String;
    fn try_from(stored: StoredPattern) -> Result<Self, Self::Error> {
        let mut pattern = Self {
            enabled: stored.enabled,
            bars: stored.bars,
            beats: stored.beats,
            density: stored.density,
            cells: [0; 144],
        };
        pattern.validate()?;
        if stored.rows.len() != usize::from(stored.bars) {
            return Err("pattern rows must match bars".into());
        }
        for (bar, row) in stored.rows.iter().enumerate() {
            if row.len() != usize::from(stored.beats) * usize::from(stored.density) {
                return Err("pattern row length must match beats × density".into());
            }
            for (index, ch) in row.chars().enumerate() {
                let step = match ch {
                    '.' => Step::Off,
                    'x' => Step::Normal,
                    'X' => Step::Accent,
                    _ => return Err("pattern points must be '.', 'x' or 'X'".into()),
                };
                pattern.set(
                    bar as u8,
                    (index / usize::from(stored.density)) as u8,
                    (index % usize::from(stored.density)) as u8,
                    step,
                );
            }
        }
        Ok(pattern)
    }
}

#[derive(Clone, Debug, Default)]
pub struct Editor {
    pub bar: u8,
    pub beat: u8,
    pub sub: u8,
    pub undo: Vec<Pattern>,
}

impl Editor {
    pub fn clamp(&mut self, pattern: Pattern) {
        self.bar = self.bar.min(pattern.bars - 1);
        self.beat = self.beat.min(pattern.beats - 1);
        self.sub = self.sub.min(pattern.density - 1);
    }
    pub fn remember(&mut self, pattern: Pattern) {
        if self.undo.len() == 64 {
            self.undo.remove(0);
        }
        self.undo.push(pattern);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packed_points_do_not_affect_neighbors() {
        let mut pattern = Pattern::default();
        for bar in 0..4 {
            for beat in 0..12 {
                for sub in 0..12 {
                    pattern.set(bar, beat, sub, Step::Off);
                }
            }
        }
        pattern.set(3, 11, 11, Step::Accent);
        pattern.set(0, 0, 1, Step::Normal);
        assert_eq!(pattern.get(3, 11, 11), Step::Accent);
        assert_eq!(pattern.get(0, 0, 1), Step::Normal);
        assert_eq!(pattern.get(0, 0, 0), Step::Off);
    }
    #[test]
    fn resizing_preserves_time_positions_not_cell_numbers() {
        let mut pattern = Pattern::default();
        pattern.set(0, 0, 2, Step::Normal);
        pattern.resize(2, 4, 6);
        assert_eq!(pattern.get(0, 0, 3), Step::Normal);
        assert_eq!(pattern.get(0, 0, 2), Step::Off);
        assert_eq!(pattern.get(1, 0, 0), Step::Off);
        pattern.copy_bar(0, 1);
        assert_eq!(pattern.get(1, 0, 3), Step::Normal);
    }
    #[test]
    fn human_readable_serialization_round_trips_and_rejects_invalid_data() {
        let pattern = Pattern::default();
        let text = toml::to_string(&pattern).unwrap();
        assert!(text.contains("X...x...x...x..."));
        assert_eq!(toml::from_str::<Pattern>(&text).unwrap(), pattern);
        assert!(toml::from_str::<Pattern>(&text.replace("X...", "Z...")).is_err());
        assert!(toml::from_str::<Pattern>(&text.replace("bars = 1", "bars = 2")).is_err());
    }
}
