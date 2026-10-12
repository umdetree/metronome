//! Standalone ASIO multi-client experiment. Does not load metronome configuration.
use std::{env, process::ExitCode};

#[derive(Debug, PartialEq)]
struct Options {
    driver: String,
    list: bool,
    describe: bool,
    duplex: bool,
    seconds: u64,
    delay: u64,
    input_channel: usize,
    bpm: u16,
    level: f32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            driver: "Yamaha Steinberg USB ASIO".into(),
            list: false,
            describe: false,
            duplex: false,
            seconds: 20,
            delay: 10,
            input_channel: 2,
            bpm: 120,
            level: 0.03,
        }
    }
}

const HELP: &str = "ASIO multi-client probe (Windows only)
Usage: asio-probe [options]
  --list                 List registered ASIO driver names; do not open them
  --describe             Open the selected driver and report its current configuration
  --driver NAME          Exact driver name (default: Yamaha Steinberg USB ASIO)
  --duplex               Also send quiet clicks to all output channels
  --seconds N            Test duration, 1–300 (default: 20)
  --delay N              Wait before opening driver, 0–60 seconds (default: 10)
  --input-channel N      1-based channel to check for guitar (default: 2)
  --bpm N                Click tempo, 30–300 (default: 120)
  --level N              Click peak amplitude, 0–0.1 (default: 0.03)
  --help                 Show help without opening audio

Keep REAPER's ASIO engine active. Disable its 'close audio device when stopped
and application is inactive' option, or keep REAPER foreground during the test.
This tool never monitors input, records audio, or changes sample rate/buffer size.
Callbacks alone do NOT prove REAPER remained active or that outputs were mixed.";

fn parse(args: impl IntoIterator<Item = String>) -> Result<Option<Options>, String> {
    let mut args = args.into_iter();
    let mut options = Options::default();
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--help" | "-h" => return Ok(None),
            "--list" => options.list = true,
            "--describe" => options.describe = true,
            "--duplex" => options.duplex = true,
            "--driver" => options.driver = args.next().ok_or("--driver requires a name")?,
            "--seconds" | "--delay" | "--input-channel" | "--bpm" | "--level" => {
                let value = args
                    .next()
                    .ok_or_else(|| format!("{flag} requires a value"))?;
                match flag.as_str() {
                    "--seconds" => {
                        options.seconds = value.parse().map_err(|_| "invalid seconds")?
                    }
                    "--delay" => options.delay = value.parse().map_err(|_| "invalid delay")?,
                    "--input-channel" => {
                        options.input_channel =
                            value.parse().map_err(|_| "invalid input channel")?
                    }
                    "--bpm" => options.bpm = value.parse().map_err(|_| "invalid BPM")?,
                    _ => options.level = value.parse().map_err(|_| "invalid level")?,
                }
            }
            _ => return Err(format!("unknown option: {flag}")),
        }
    }
    if !(1..=300).contains(&options.seconds)
        || options.delay > 60
        || !(1..=64).contains(&options.input_channel)
        || !(30..=300).contains(&options.bpm)
        || !options.level.is_finite()
        || !(0.0..=0.1).contains(&options.level)
    {
        return Err("argument outside allowed range; see --help".into());
    }
    if options.driver.trim().is_empty() || (options.list && options.describe) {
        return Err("invalid driver or conflicting list/describe options".into());
    }
    Ok(Some(options))
}

fn main() -> ExitCode {
    let options = match parse(env::args().skip(1)) {
        Ok(Some(options)) => options,
        Ok(None) => {
            println!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("{error}\n\n{HELP}");
            return ExitCode::FAILURE;
        }
    };
    match run(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("PROBE FAILED: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(not(windows))]
fn run(_options: Options) -> Result<(), String> {
    Err("ASIO probe is Windows-only; the normal metronome remains cross-platform".into())
}

#[cfg(windows)]
fn run(options: Options) -> Result<(), String> {
    windows::run(options)
}

#[cfg(windows)]
mod windows {
    use super::Options;
    use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig, traits::DeviceTrait};
    use std::{
        sync::{
            Arc, Mutex,
            atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };

    struct Meter {
        peaks: Vec<AtomicU32>,
        callbacks: AtomicU64,
        frames: AtomicU64,
        block_frames: AtomicU64,
        max_gap_us: AtomicU64,
        errors: AtomicU64,
    }

    impl Meter {
        fn new(channels: u16) -> Self {
            Self {
                peaks: (0..channels).map(|_| AtomicU32::new(0)).collect(),
                callbacks: AtomicU64::new(0),
                frames: AtomicU64::new(0),
                block_frames: AtomicU64::new(0),
                max_gap_us: AtomicU64::new(0),
                errors: AtomicU64::new(0),
            }
        }
    }

    fn input<T>(
        device: &cpal::platform::AsioDevice,
        config: &StreamConfig,
        meter: Arc<Meter>,
    ) -> Result<cpal::platform::AsioStream, String>
    where
        T: SizedSample,
        f32: FromSample<T>,
    {
        let channels = usize::from(config.channels);
        let errors = Arc::clone(&meter);
        let mut previous = None;
        device
            .build_input_stream(
                config,
                move |data: &[T], _| {
                    let now = Instant::now();
                    if let Some(previous) = previous {
                        meter.max_gap_us.fetch_max(
                            now.duration_since(previous).as_micros() as u64,
                            Ordering::Relaxed,
                        );
                    }
                    previous = Some(now);
                    for (channel, peak) in meter.peaks.iter().enumerate() {
                        let max = data
                            .iter()
                            .skip(channel)
                            .step_by(channels)
                            .map(|sample| sample.to_sample::<f32>().abs())
                            .filter(|value| value.is_finite())
                            .fold(0.0_f32, f32::max);
                        peak.fetch_max(max.to_bits(), Ordering::Relaxed);
                    }
                    meter.callbacks.fetch_add(1, Ordering::Relaxed);
                    meter
                        .frames
                        .fetch_add((data.len() / channels) as u64, Ordering::Relaxed);
                    meter
                        .block_frames
                        .store((data.len() / channels) as u64, Ordering::Relaxed);
                },
                move |_| {
                    errors.errors.fetch_add(1, Ordering::Relaxed);
                },
                None,
            )
            .map_err(|error| format!("create input stream: {error}"))
    }

    fn output<T>(
        device: &cpal::platform::AsioDevice,
        config: &StreamConfig,
        meter: Arc<Meter>,
        options: &Options,
    ) -> Result<cpal::platform::AsioStream, String>
    where
        T: SizedSample + FromSample<f32>,
    {
        let channels = usize::from(config.channels);
        let rate = config.sample_rate.0;
        let interval = (f64::from(rate) * 60.0 / f64::from(options.bpm)).round() as u64;
        let click_frames = (f64::from(rate) * 0.025) as u64;
        let level = options.level;
        let mut position = 0_u64;
        let errors = Arc::clone(&meter);
        device
            .build_output_stream(
                config,
                move |data: &mut [T], _| {
                    for frame in data.chunks_mut(channels) {
                        let offset = position % interval;
                        let sample = if offset < click_frames {
                            let time = offset as f32 / rate as f32;
                            let envelope = (time / 0.001).min(1.0)
                                * (-time * 160.0).exp()
                                * (1.0 - offset as f32 / click_frames as f32);
                            (std::f32::consts::TAU * 1320.0 * time).sin() * envelope * level
                        } else {
                            0.0
                        };
                        frame.fill(T::from_sample(sample));
                        position += 1;
                    }
                    meter.callbacks.fetch_add(1, Ordering::Relaxed);
                    meter
                        .frames
                        .fetch_add((data.len() / channels) as u64, Ordering::Relaxed);
                },
                move |_| {
                    errors.errors.fetch_add(1, Ordering::Relaxed);
                },
                None,
            )
            .map_err(|error| format!("create output stream: {error}"))
    }

    pub fn run(options: Options) -> Result<(), String> {
        if !options.list {
            println!(
                "Mode: {}. Opening driver in {} seconds; switch back to REAPER now.",
                if options.describe {
                    "describe"
                } else if options.duplex {
                    "input + quiet clicks"
                } else {
                    "input only (no monitoring/output)"
                },
                options.delay
            );
            println!("Keep REAPER audio active; this tool will not change your REAPER settings.");
            thread::sleep(Duration::from_secs(options.delay));
        }
        let asio = asio_sys::Asio::new();
        let names = asio.driver_names();
        if options.list {
            for name in names {
                println!("{name}");
            }
            return Ok(());
        }
        if !names.contains(&options.driver) {
            return Err(format!(
                "driver '{}' not registered; use --list",
                options.driver
            ));
        }
        println!("Loading driver: {}", options.driver);
        let driver = asio
            .load_driver(&options.driver)
            .map_err(|error| format!("load driver: {error}"))?;
        // Exact-name loading avoids CPAL enumeration initializing unrelated hardware.
        let device = cpal::platform::AsioDevice {
            driver: Arc::new(driver),
            asio_streams: Arc::new(Mutex::new(asio_sys::AsioStreams {
                input: None,
                output: None,
            })),
            current_buffer_index: Arc::new(AtomicI32::new(-1)),
        };
        let input_config = device
            .default_input_config()
            .map_err(|error| format!("query input config: {error}"))?;
        println!(
            "Input: {} channels, {} Hz, {:?}, buffer range {:?}",
            input_config.channels(),
            input_config.sample_rate().0,
            input_config.sample_format(),
            input_config.buffer_size()
        );
        if options.describe {
            let output_config = device
                .default_output_config()
                .map_err(|error| format!("query output config: {error}"))?;
            println!(
                "Output: {} channels, {} Hz, {:?}, buffer range {:?}",
                output_config.channels(),
                output_config.sample_rate().0,
                output_config.sample_format(),
                output_config.buffer_size()
            );
            println!(
                "Driver opened. No buffers started; this is NOT a multi-client streaming result."
            );
            return Ok(());
        }
        if options.input_channel > usize::from(input_config.channels()) {
            return Err("selected input channel does not exist".into());
        }
        let input_meter = Arc::new(Meter::new(input_config.channels()));
        let config = input_config.config(); // Current sample rate, preferred driver buffer size.
        let input_stream = match input_config.sample_format() {
            SampleFormat::I16 => input::<i16>(&device, &config, Arc::clone(&input_meter)),
            SampleFormat::I32 => input::<i32>(&device, &config, Arc::clone(&input_meter)),
            SampleFormat::F32 => input::<f32>(&device, &config, Arc::clone(&input_meter)),
            format => Err(format!("unsupported ASIO input sample format: {format}")),
        }?;
        let output_meter = Arc::new(Meter::new(0));
        let output_stream = if options.duplex {
            let supported = device
                .default_output_config()
                .map_err(|error| format!("query output config: {error}"))?;
            if supported.sample_rate() != input_config.sample_rate() {
                return Err(
                    "input/output sample rates differ; will not change device settings".into(),
                );
            }
            println!(
                "Output: {} channels, {} Hz, {:?}; click level {}",
                supported.channels(),
                supported.sample_rate().0,
                supported.sample_format(),
                options.level
            );
            let config = supported.config();
            Some(match supported.sample_format() {
                SampleFormat::I16 => {
                    output::<i16>(&device, &config, Arc::clone(&output_meter), &options)
                }
                SampleFormat::I32 => {
                    output::<i32>(&device, &config, Arc::clone(&output_meter), &options)
                }
                SampleFormat::F32 => {
                    output::<f32>(&device, &config, Arc::clone(&output_meter), &options)
                }
                format => Err(format!("unsupported ASIO output sample format: {format}")),
            }?)
        } else {
            None
        };
        input_stream
            .play()
            .map_err(|error| format!("start input: {error}"))?;
        if let Some(stream) = &output_stream {
            stream
                .play()
                .map_err(|error| format!("start output: {error}"))?;
        }
        println!(
            "Streams started. Test {} seconds; guitar input channel {}. No input monitoring.",
            options.seconds, options.input_channel
        );
        let start = Instant::now();
        let mut active_intervals = 0;
        let mut signal_intervals = 0;
        let mut previous_callbacks = 0;
        for second in 1..=options.seconds {
            thread::sleep(
                (start + Duration::from_secs(second)).saturating_duration_since(Instant::now()),
            );
            let callbacks = input_meter.callbacks.load(Ordering::Relaxed);
            if callbacks > previous_callbacks {
                active_intervals += 1;
            }
            let mut peaks = Vec::new();
            for peak in &input_meter.peaks {
                peaks.push(f32::from_bits(peak.swap(0, Ordering::Relaxed)));
            }
            if peaks[options.input_channel - 1] > 0.001 {
                signal_intervals += 1;
            }
            let levels: Vec<_> = peaks
                .iter()
                .enumerate()
                .map(|(index, peak)| {
                    format!(
                        "ch{} {:>6.1} dBFS",
                        index + 1,
                        if *peak > 0.0 {
                            20.0 * peak.log10()
                        } else {
                            -120.0
                        }
                    )
                })
                .collect();
            println!(
                "{second:>3}s | in {} blocks (+{}) | {} | block {} frames | out {} blocks",
                callbacks,
                callbacks - previous_callbacks,
                levels.join(" | "),
                input_meter.block_frames.load(Ordering::Relaxed),
                output_meter.callbacks.load(Ordering::Relaxed)
            );
            previous_callbacks = callbacks;
        }
        input_stream
            .pause()
            .map_err(|error| format!("pause input: {error}"))?;
        if let Some(stream) = &output_stream {
            stream
                .pause()
                .map_err(|error| format!("pause output: {error}"))?;
        }
        println!(
            "SUMMARY: input active {active_intervals}/{} seconds; selected-channel signal {signal_intervals}/{} seconds; max callback gap {:.2} ms; reported errors {}",
            options.seconds,
            options.seconds,
            input_meter.max_gap_us.load(Ordering::Relaxed) as f64 / 1000.0,
            input_meter.errors.load(Ordering::Relaxed)
                + output_meter.errors.load(Ordering::Relaxed)
        );
        println!(
            "Manual check required: Did REAPER keep processing wet guitar? In duplex, did wet guitar AND clicks remain audible together?"
        );
        println!(
            "This CPAL ASIO backend does not report every driver error/dropout; callback gaps are diagnostic, not latency measurements."
        );
        if active_intervals < options.seconds
            || (options.duplex && output_meter.callbacks.load(Ordering::Relaxed) == 0)
        {
            return Err(
                "callbacks stalled or did not start; multi-client streaming is not confirmed"
                    .into(),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }
    #[test]
    fn defaults_do_not_output_sound() {
        let options = parse(Vec::new()).unwrap().unwrap();
        assert!(!options.duplex);
        assert_eq!(options.input_channel, 2);
    }
    #[test]
    fn validates_cli_before_opening_hardware() {
        for values in [
            vec!["--seconds", "0"],
            vec!["--input-channel", "0"],
            vec!["--level", "NaN"],
            vec!["--level", "0.5"],
            vec!["--unknown"],
        ] {
            assert!(parse(args(&values)).is_err());
        }
        assert!(parse(args(&["--help"])).unwrap().is_none());
        let options = parse(args(&["--duplex", "--seconds", "10", "--delay", "5"]))
            .unwrap()
            .unwrap();
        assert!(options.duplex);
        assert_eq!(options.seconds, 10);
    }
}
