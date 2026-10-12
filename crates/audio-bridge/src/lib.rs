//! Bounded, single-producer/single-consumer cross-process audio transport.
//! Version 1: native-endian atomics, IEEE f32 interleaved audio, one reader per source.
//! Both processes must run on the same machine/architecture. No Rust pointers cross IPC.

use directories::ProjectDirs;
use fs2::FileExt;
use memmap2::{MmapMut, MmapOptions};
use std::{
    fs::{self, File, OpenOptions},
    io,
    mem::size_of,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(not(target_has_atomic = "64"))]
compile_error!("Audio bridge requires native 64-bit atomics");

pub const VERSION: u32 = 1;
pub const MAX_FRAMES: usize = 1024;
pub const MAX_CHANNELS: usize = 2;
pub const CAPACITY: usize = 32;
pub const FLAG_STEADY_VALID: u32 = 1;
pub const FLAG_TRANSPORT_VALID: u32 = 2;
pub const FLAG_PLAYING: u32 = 4;
const MAGIC: u32 = 0x4D425231;

#[repr(C, align(64))]
struct Header {
    magic: AtomicU32,
    version: AtomicU32,
    bytes: AtomicU32,
    alive: AtomicU32,
    active: AtomicU32,
    generation: AtomicU32,
    sample_rate: AtomicU32,
    channels: AtomicU32,
    write: AtomicU64,
    read: AtomicU64,
    dropped: AtomicU64,
}

#[repr(C, align(64))]
struct Slot {
    sequence: AtomicU64,
    position: AtomicU64,
    steady: AtomicU64,
    transport_seconds: AtomicU64,
    frames: AtomicU32,
    channels: AtomicU32,
    rate: AtomicU32,
    generation: AtomicU32,
    flags: AtomicU32,
    samples: [AtomicU32; MAX_FRAMES * MAX_CHANNELS],
}

#[repr(C, align(64))]
struct Shared {
    header: Header,
    slots: [Slot; CAPACITY],
}

pub fn directory() -> io::Result<PathBuf> {
    ProjectDirs::from("", "", "metronome")
        .map(|dirs| dirs.data_local_dir().join("audio-bridge-v1"))
        .ok_or_else(|| io::Error::other("cannot locate user data directory"))
}

fn open_lock(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
}

fn sidecar(path: &Path, extension: &str) -> PathBuf {
    path.with_extension(extension)
}

// SAFETY: a mapping has exactly Shared's size and page-aligned address, and all fields
// are initialized atomic integer representations. Shared has no non-atomic payload.
fn shared(map: &MmapMut) -> &Shared {
    unsafe { &*map.as_ptr().cast::<Shared>() }
}

fn map_file(file: &File) -> io::Result<MmapMut> {
    if file.metadata()?.len() != size_of::<Shared>() as u64 {
        return Err(io::Error::other("incompatible bridge mapping size"));
    }
    // SAFETY: bridge owners never truncate a published mapping; all concurrent
    // accesses use atomics. Only cooperative local clients are supported.
    unsafe { MmapOptions::new().len(size_of::<Shared>()).map_mut(file) }
}

#[derive(Clone, Debug)]
pub struct Source {
    pub id: String,
    pub path: PathBuf,
}

pub fn discover() -> io::Result<Vec<Source>> {
    discover_in(&directory()?)
}

pub fn discover_in(directory: &Path) -> io::Result<Vec<Source>> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut sources = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "bridge") {
            continue;
        }
        let Ok(owner) = OpenOptions::new()
            .read(true)
            .write(true)
            .open(sidecar(&path, "owner"))
        else {
            continue;
        };
        match FileExt::try_lock_shared(&owner) {
            Ok(()) => {
                let _ = FileExt::unlock(&owner);
                continue;
            } // stale/crashed instance
            Err(error) if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() => {}
            Err(_) => continue,
        }
        let Ok(file) = OpenOptions::new().read(true).write(true).open(&path) else {
            continue;
        };
        let Ok(map) = map_file(&file) else {
            continue;
        };
        if validate(shared(&map)).is_err() || shared(&map).header.alive.load(Ordering::Acquire) == 0
        {
            continue;
        }
        if let Some(id) = path.file_stem().and_then(|id| id.to_str()) {
            sources.push(Source {
                id: id.into(),
                path,
            });
        }
    }
    sources.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(sources)
}

fn validate(shared: &Shared) -> io::Result<()> {
    if shared.header.magic.load(Ordering::Acquire) != MAGIC
        || shared.header.version.load(Ordering::Relaxed) != VERSION
        || shared.header.bytes.load(Ordering::Relaxed) != size_of::<Shared>() as u32
    {
        return Err(io::Error::other(
            "unsupported or uninitialized audio bridge protocol",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Timing {
    pub position: u64,
    pub steady: i64,
    pub transport_seconds: f64,
    pub flags: u32,
}

pub struct Producer {
    source: Source,
    map: Option<MmapMut>,
    owner: File,
    sequence: u64,
}

impl Producer {
    pub fn create() -> io::Result<Self> {
        Self::create_in(&directory()?)
    }
    pub fn create_in(directory: &Path) -> io::Result<Self> {
        fs::create_dir_all(directory)?;
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let id = format!(
            "{}-{time:x}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let path = directory.join(format!("{id}.bridge"));
        let owner = open_lock(&sidecar(&path, "owner"))?;
        FileExt::try_lock_exclusive(&owner)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.set_len(size_of::<Shared>() as u64)?;
        let mut map = map_file(&file)?;
        map.fill(0); // All-zero is a valid initial representation of every atomic field.
        let header = &shared(&map).header;
        header.version.store(VERSION, Ordering::Relaxed);
        header
            .bytes
            .store(size_of::<Shared>() as u32, Ordering::Relaxed);
        header.alive.store(1, Ordering::Relaxed);
        header.magic.store(MAGIC, Ordering::Release); // Publish initialization last.
        Ok(Self {
            source: Source { id, path },
            map: Some(map),
            owner,
            sequence: 0,
        })
    }
    pub fn source(&self) -> &Source {
        &self.source
    }
    pub fn activate(&self, sample_rate: u32, channels: u32) {
        let header = &shared(self.map.as_ref().unwrap()).header;
        header.generation.fetch_add(1, Ordering::Relaxed);
        header.sample_rate.store(sample_rate, Ordering::Relaxed);
        header.channels.store(channels, Ordering::Relaxed);
        header.active.store(1, Ordering::Release);
    }
    pub fn deactivate(&self) {
        shared(self.map.as_ref().unwrap())
            .header
            .active
            .store(0, Ordering::Release);
    }
    pub fn discontinuity(&self) {
        shared(self.map.as_ref().unwrap())
            .header
            .generation
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Audio-thread safe: bounded loops, atomic accesses, no allocation/locks/I/O.
    /// The sample closure returns an interleaved frame/channel's normalized value.
    pub fn push(
        &mut self,
        frames: usize,
        channels: usize,
        timing: Timing,
        mut sample: impl FnMut(usize, usize) -> f32,
    ) -> bool {
        self.sequence = self.sequence.wrapping_add(1);
        let memory = shared(self.map.as_ref().unwrap());
        let header = &memory.header;
        let write = header.write.load(Ordering::Relaxed);
        let read = header.read.load(Ordering::Acquire);
        if frames == 0
            || frames > MAX_FRAMES
            || channels == 0
            || channels > MAX_CHANNELS
            || write.wrapping_sub(read) >= CAPACITY as u64
        {
            header.dropped.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let slot = &memory.slots[write as usize % CAPACITY];
        slot.sequence.store(self.sequence, Ordering::Relaxed);
        slot.position.store(timing.position, Ordering::Relaxed);
        slot.steady.store(timing.steady as u64, Ordering::Relaxed);
        slot.transport_seconds
            .store(timing.transport_seconds.to_bits(), Ordering::Relaxed);
        slot.frames.store(frames as u32, Ordering::Relaxed);
        slot.channels.store(channels as u32, Ordering::Relaxed);
        slot.rate.store(
            header.sample_rate.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
        slot.generation
            .store(header.generation.load(Ordering::Relaxed), Ordering::Relaxed);
        slot.flags.store(timing.flags, Ordering::Relaxed);
        for frame in 0..frames {
            for channel in 0..channels {
                slot.samples[frame * channels + channel]
                    .store(sample(frame, channel).to_bits(), Ordering::Relaxed);
            }
        }
        header.write.store(write.wrapping_add(1), Ordering::Release);
        true
    }
}

impl Drop for Producer {
    fn drop(&mut self) {
        shared(self.map.as_ref().unwrap())
            .header
            .alive
            .store(0, Ordering::Release);
        self.map.take();
        let _ = FileExt::unlock(&self.owner);
        // Windows may retain the mapping while a reader is attached. Stale files
        // are ignored using the owner lock; never truncate/reuse them.
        let _ = fs::remove_file(&self.source.path);
        let _ = fs::remove_file(sidecar(&self.source.path, "owner"));
        let _ = fs::remove_file(sidecar(&self.source.path, "reader"));
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Status {
    pub alive: bool,
    pub active: bool,
    pub dropped: u64,
    pub queued: u64,
}

pub struct Block {
    pub sequence: u64,
    pub generation: u32,
    pub sample_rate: u32,
    pub frames: usize,
    pub channels: usize,
    pub timing: Timing,
    pub samples: [f32; MAX_FRAMES * MAX_CHANNELS],
}
impl Default for Block {
    fn default() -> Self {
        Self {
            sequence: 0,
            generation: 0,
            sample_rate: 0,
            frames: 0,
            channels: 0,
            timing: Timing::default(),
            samples: [0.0; MAX_FRAMES * MAX_CHANNELS],
        }
    }
}

pub struct Receiver {
    map: MmapMut,
    _claim: File,
}
impl Receiver {
    pub fn connect(source: &Source) -> io::Result<Self> {
        let claim = open_lock(&sidecar(&source.path, "reader"))?;
        FileExt::try_lock_exclusive(&claim).map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("source already has a reader, or cannot lock it: {error}"),
            )
        })?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&source.path)?;
        let map = map_file(&file)?;
        validate(shared(&map))?;
        let header = &shared(&map).header;
        if header.alive.load(Ordering::Acquire) == 0 {
            return Err(io::Error::other("source is offline"));
        }
        // Start at current publication point, not at old buffered audio.
        header
            .read
            .store(header.write.load(Ordering::Acquire), Ordering::Release);
        Ok(Self { map, _claim: claim })
    }
    pub fn status(&self) -> Status {
        let header = &shared(&self.map).header;
        Status {
            alive: header.alive.load(Ordering::Acquire) != 0,
            active: header.active.load(Ordering::Acquire) != 0,
            dropped: header.dropped.load(Ordering::Relaxed),
            queued: header
                .write
                .load(Ordering::Acquire)
                .wrapping_sub(header.read.load(Ordering::Relaxed)),
        }
    }
    pub fn try_read(&mut self, block: &mut Block) -> io::Result<bool> {
        let memory = shared(&self.map);
        let read = memory.header.read.load(Ordering::Relaxed);
        let write = memory.header.write.load(Ordering::Acquire);
        if read == write {
            return Ok(false);
        }
        if write.wrapping_sub(read) > CAPACITY as u64 {
            return Err(io::Error::other("invalid audio bridge queue counters"));
        }
        let slot = &memory.slots[read as usize % CAPACITY];
        let frames = slot.frames.load(Ordering::Relaxed) as usize;
        let channels = slot.channels.load(Ordering::Relaxed) as usize;
        let rate = slot.rate.load(Ordering::Relaxed);
        if !(1..=MAX_FRAMES).contains(&frames)
            || !(1..=MAX_CHANNELS).contains(&channels)
            || !(8000..=384000).contains(&rate)
        {
            return Err(io::Error::other("invalid audio bridge block dimensions"));
        }
        block.sequence = slot.sequence.load(Ordering::Relaxed);
        block.generation = slot.generation.load(Ordering::Relaxed);
        block.sample_rate = rate;
        block.frames = frames;
        block.channels = channels;
        block.timing = Timing {
            position: slot.position.load(Ordering::Relaxed),
            steady: slot.steady.load(Ordering::Relaxed) as i64,
            transport_seconds: f64::from_bits(slot.transport_seconds.load(Ordering::Relaxed)),
            flags: slot.flags.load(Ordering::Relaxed),
        };
        for index in 0..frames * channels {
            block.samples[index] = f32::from_bits(slot.samples[index].load(Ordering::Relaxed));
        }
        memory
            .header
            .read
            .store(read.wrapping_add(1), Ordering::Release);
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_queue_drops_without_overwriting_unread_audio() {
        let directory = tempfile::tempdir().unwrap();
        let mut producer = Producer::create_in(directory.path()).unwrap();
        producer.activate(48000, 2);
        let mut receiver = Receiver::connect(producer.source()).unwrap();
        for index in 0..CAPACITY {
            assert!(producer.push(
                2,
                2,
                Timing {
                    position: index as u64 * 2,
                    ..Timing::default()
                },
                |_, channel| channel as f32
            ));
        }
        assert!(!producer.push(2, 2, Timing::default(), |_, _| 99.0));
        assert_eq!(receiver.status().dropped, 1);
        let mut block = Block::default();
        for index in 0..CAPACITY {
            assert!(receiver.try_read(&mut block).unwrap());
            assert_eq!(block.timing.position, index as u64 * 2);
            assert_eq!(&block.samples[..4], &[0.0, 1.0, 0.0, 1.0]);
        }
        assert!(!receiver.try_read(&mut block).unwrap());
    }
    #[test]
    fn reader_claim_reconnect_and_discovery_handle_lifecycle() {
        let directory = tempfile::tempdir().unwrap();
        let mut producer = Producer::create_in(directory.path()).unwrap();
        assert_eq!(discover_in(directory.path()).unwrap().len(), 1);
        let receiver = Receiver::connect(producer.source()).unwrap();
        assert!(Receiver::connect(producer.source()).is_err());
        drop(receiver);
        assert!(producer.push(1, 1, Timing::default(), |_, _| 0.5));
        let mut receiver = Receiver::connect(producer.source()).unwrap();
        assert!(!receiver.try_read(&mut Block::default()).unwrap());
        drop(producer);
        assert!(!receiver.status().alive);
        assert!(discover_in(directory.path()).unwrap().is_empty());
    }
    #[test]
    fn invalid_protocol_version_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let producer = Producer::create_in(directory.path()).unwrap();
        shared(producer.map.as_ref().unwrap())
            .header
            .version
            .store(99, Ordering::Relaxed);
        assert!(Receiver::connect(producer.source()).is_err());
    }
}
