//! Audio reception/analysis never runs on the terminal thread.
use audio_bridge::{Block, Receiver, Source};
use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        mpsc::{self, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub const WAVE_POINTS: usize = 256;

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub status: String,
    pub source: Option<String>,
    pub sources: Vec<String>,
    pub sample_rate: u32,
    pub channels: usize,
    pub channel: usize,
    pub peaks: [f32; 2],
    pub waveform: Vec<(f32, f32)>,
    pub dropped: u64,
    pub discontinuities: u64,
    pub queued: u64,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            status: "Searching for CLAP audio bridge…".into(),
            source: None,
            sources: Vec::new(),
            sample_rate: 0,
            channels: 0,
            channel: 0,
            peaks: [0.0; 2],
            waveform: Vec::new(),
            dropped: 0,
            discontinuities: 0,
            queued: 0,
        }
    }
}

pub enum Command {
    NextSource,
    NextChannel,
    Refresh,
    Stop,
}
pub struct Monitor {
    commands: SyncSender<Command>,
    latest: Arc<Mutex<Snapshot>>,
    worker: Option<JoinHandle<()>>,
}
impl Monitor {
    pub fn start() -> Self {
        let (commands, receiver) = mpsc::sync_channel(8);
        let latest = Arc::new(Mutex::new(Snapshot::default()));
        let target = Arc::clone(&latest);
        let worker = thread::spawn(move || run(receiver, target));
        Self {
            commands,
            latest,
            worker: Some(worker),
        }
    }
    pub fn send(&self, command: Command) {
        let _ = self.commands.try_send(command);
    }
    pub fn snapshot(&self) -> Option<Snapshot> {
        self.latest.try_lock().ok().map(|snapshot| snapshot.clone())
    }
}
impl Drop for Monitor {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Analysis {
    rate: u32,
    channels: usize,
    previous: Option<(u64, u32, u64)>,
    discontinuities: u64,
    waves: [VecDeque<(f32, f32)>; 2],
    extrema: [(f32, f32); 2],
    peaks: [f32; 2],
    bucket_frames: usize,
    count: usize,
}
impl Default for Analysis {
    fn default() -> Self {
        Self {
            rate: 0,
            channels: 0,
            previous: None,
            discontinuities: 0,
            waves: std::array::from_fn(|_| VecDeque::with_capacity(WAVE_POINTS)),
            extrema: [(f32::INFINITY, f32::NEG_INFINITY); 2],
            peaks: [0.0; 2],
            bucket_frames: 1,
            count: 0,
        }
    }
}
impl Analysis {
    fn clear_wave(&mut self) {
        for wave in &mut self.waves {
            wave.clear();
        }
        self.count = 0;
        self.extrema = [(f32::INFINITY, f32::NEG_INFINITY); 2];
    }
    fn ingest(&mut self, block: &Block) {
        let gap = self.previous.is_some_and(|(sequence, generation, next)| {
            sequence.wrapping_add(1) != block.sequence
                || generation != block.generation
                || next != block.timing.position
        });
        if gap {
            self.discontinuities += 1;
        }
        if gap || self.rate != block.sample_rate || self.channels != block.channels {
            self.clear_wave();
            self.rate = block.sample_rate;
            self.channels = block.channels;
            self.bucket_frames = ((self.rate as usize * 2) / WAVE_POINTS).max(1);
        }
        self.previous = Some((
            block.sequence,
            block.generation,
            block.timing.position.wrapping_add(block.frames as u64),
        ));
        for frame in 0..block.frames {
            for channel in 0..block.channels {
                let raw = block.samples[frame * block.channels + channel];
                let sample = if raw.is_finite() { raw } else { 0.0 };
                self.peaks[channel] = self.peaks[channel].max(sample.abs());
                self.extrema[channel].0 = self.extrema[channel].0.min(sample);
                self.extrema[channel].1 = self.extrema[channel].1.max(sample);
            }
            self.count += 1;
            if self.count == self.bucket_frames {
                for channel in 0..block.channels {
                    if self.waves[channel].len() == WAVE_POINTS {
                        self.waves[channel].pop_front();
                    }
                    self.waves[channel].push_back(self.extrema[channel]);
                }
                self.extrema = [(f32::INFINITY, f32::NEG_INFINITY); 2];
                self.count = 0;
            }
        }
    }
}

fn run(commands: mpsc::Receiver<Command>, latest: Arc<Mutex<Snapshot>>) {
    let mut receiver: Option<Receiver> = None;
    let mut sources: Vec<Source> = Vec::new();
    let mut selected: Option<String> = None;
    let mut analysis = Analysis::default();
    let mut snapshot = Snapshot::default();
    let mut block = Block::default();
    let mut last_scan = Instant::now() - Duration::from_secs(2);
    let mut last_publish = Instant::now();
    let mut last_audio = Instant::now();
    let mut drop_base = 0;
    loop {
        while let Ok(command) = commands.try_recv() {
            match command {
                Command::Stop => return,
                Command::Refresh => {
                    last_scan = Instant::now() - Duration::from_secs(2);
                    receiver = None;
                    selected = None;
                    analysis = Analysis::default();
                }
                Command::NextChannel => {
                    snapshot.channel = (snapshot.channel + 1) % analysis.channels.max(1)
                }
                Command::NextSource => {
                    if !sources.is_empty() {
                        let index = selected
                            .as_ref()
                            .and_then(|id| sources.iter().position(|source| &source.id == id))
                            .map_or(0, |index| (index + 1) % sources.len());
                        selected = Some(sources[index].id.clone());
                        receiver = None;
                        analysis = Analysis::default();
                        last_scan = Instant::now() - Duration::from_secs(2);
                    }
                }
            }
        }
        if last_scan.elapsed() >= Duration::from_secs(1) {
            last_scan = Instant::now();
            match audio_bridge::discover() {
                Ok(found) => {
                    sources = found;
                    snapshot.sources = sources.iter().map(|source| source.id.clone()).collect();
                    if selected
                        .as_ref()
                        .is_some_and(|id| !sources.iter().any(|source| &source.id == id))
                    {
                        receiver = None;
                        selected = None;
                        analysis = Analysis::default();
                    }
                    if selected.is_none() && sources.len() == 1 {
                        selected = Some(sources[0].id.clone());
                    }
                    if receiver.is_none() {
                        if let Some(source) = selected
                            .as_ref()
                            .and_then(|id| sources.iter().find(|source| &source.id == id))
                        {
                            match Receiver::connect(source) {
                                Ok(connected) => {
                                    drop_base = connected.status().dropped;
                                    receiver = Some(connected);
                                    last_audio = Instant::now();
                                    snapshot.status = "Connected; waiting for host audio".into();
                                }
                                Err(error) => {
                                    snapshot.status = format!("Connection failed: {error}")
                                }
                            }
                        } else {
                            snapshot.status = if sources.is_empty() {
                                "No bridge found. Add Metronome Dry Audio Bridge in REAPER.".into()
                            } else {
                                format!(
                                    "{} sources found. Press N to choose an instance.",
                                    sources.len()
                                )
                            };
                        }
                    }
                }
                Err(error) => snapshot.status = format!("Discovery failed: {error}"),
            }
        }
        if let Some(connected) = &mut receiver {
            let mut read_error = None;
            // Bounded work per pass; never let a backlog block controls or UI publication.
            for _ in 0..audio_bridge::CAPACITY {
                match connected.try_read(&mut block) {
                    Ok(true) => {
                        analysis.ingest(&block);
                        last_audio = Instant::now();
                    }
                    Ok(false) => break,
                    Err(error) => {
                        read_error = Some(format!("Audio protocol error: {error}"));
                        break;
                    }
                }
            }
            let status = connected.status();
            snapshot.dropped = status.dropped.saturating_sub(drop_base);
            snapshot.queued = status.queued;
            snapshot.status = if let Some(error) = read_error {
                error
            } else if !status.alive {
                "Bridge instance closed".into()
            } else if !status.active {
                "Connected; plugin not active".into()
            } else if last_audio.elapsed() > Duration::from_secs(1) {
                "No audio callbacks for >1 s; check host processing/bypass".into()
            } else {
                "Receiving dry audio · no input monitoring · no timing score".into()
            };
        }
        if last_publish.elapsed() >= Duration::from_millis(33) {
            last_publish = Instant::now();
            snapshot.source = selected.clone();
            snapshot.sample_rate = analysis.rate;
            snapshot.channels = analysis.channels;
            snapshot.channel = snapshot.channel.min(analysis.channels.saturating_sub(1));
            snapshot.peaks = analysis.peaks;
            analysis.peaks = [0.0; 2];
            snapshot.discontinuities = analysis.discontinuities;
            if receiver.is_none() {
                snapshot.dropped = 0;
                snapshot.queued = 0;
            }
            snapshot.waveform = analysis.waves[snapshot.channel].iter().copied().collect();
            if let Ok(mut target) = latest.lock() {
                *target = snapshot.clone();
            }
        }
        thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn waveform_is_bounded_preserves_peaks_and_resets_on_gap() {
        let mut analysis = Analysis::default();
        let mut block = Block {
            channels: 1,
            frames: 1024,
            sample_rate: 8000,
            sequence: 1,
            ..Block::default()
        };
        block.samples[10] = 0.75;
        block.samples[20] = -0.5;
        analysis.ingest(&block);
        assert!(analysis.waves[0].iter().any(|point| point.1 == 0.75));
        assert!(analysis.waves[0].iter().any(|point| point.0 == -0.5));
        for index in 1..100 {
            block.sequence = index + 1;
            block.timing.position = index * 1024;
            analysis.ingest(&block);
        }
        assert!(analysis.waves[0].len() <= WAVE_POINTS);
        block.sequence += 2;
        block.timing.position += 2048;
        analysis.ingest(&block);
        assert_eq!(analysis.discontinuities, 1);
        assert!(analysis.waves[0].len() < WAVE_POINTS);
    }
}
