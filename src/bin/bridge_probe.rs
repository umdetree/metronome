//! Read-only CLI diagnostic for the CLAP bridge, without opening an audio device.
use audio_bridge::{Block, Receiver};
use std::{
    thread,
    time::{Duration, Instant},
};

fn main() {
    if let Err(error) = run() {
        eprintln!("Bridge probe: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut seconds = 10_u64;
    let mut source_id = None;
    let mut list = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--list" => list = true,
            "--source" => source_id = Some(args.next().ok_or("--source needs an instance ID")?),
            "--seconds" => seconds = args.next().ok_or("--seconds needs a value")?.parse()?,
            "--help" | "-h" => {
                println!(
                    "bridge-probe [--list] [--source ID] [--seconds 1–300]\nNo audio device access, no playback, no recording."
                );
                return Ok(());
            }
            _ => return Err(format!("unknown option: {arg}").into()),
        }
    }
    if !(1..=300).contains(&seconds) {
        return Err("seconds must be 1–300".into());
    }
    let sources = audio_bridge::discover()?;
    if list {
        for source in &sources {
            println!("{}", source.id);
        }
        println!("{} active bridge instance(s)", sources.len());
        return Ok(());
    }
    let source = if let Some(id) = source_id {
        sources
            .iter()
            .find(|source| source.id == id)
            .ok_or("source not found")?
    } else if sources.len() == 1 {
        &sources[0]
    } else {
        return Err(
            "expected one bridge; add plugin in REAPER, or select with --list/--source".into(),
        );
    };
    let mut receiver = Receiver::connect(source)?;
    let drop_base = receiver.status().dropped;
    println!("Connected: {}", source.id);
    let start = Instant::now();
    let mut report = Instant::now();
    let mut block = Block::default();
    let mut count = 0_u64;
    let mut peaks = [0_f32; 2];
    while start.elapsed() < Duration::from_secs(seconds) {
        for _ in 0..audio_bridge::CAPACITY {
            if !receiver.try_read(&mut block)? {
                break;
            }
            count += 1;
            for frame in 0..block.frames {
                for (channel, peak) in peaks.iter_mut().enumerate().take(block.channels) {
                    let value = block.samples[frame * block.channels + channel].abs();
                    if value.is_finite() {
                        *peak = peak.max(value);
                    }
                }
            }
        }
        if report.elapsed() >= Duration::from_secs(1) {
            report = Instant::now();
            let status = receiver.status();
            println!(
                "blocks {count} | {} Hz | peak {:?} | queued {} | dropped {} | active {}",
                block.sample_rate,
                peaks,
                status.queued,
                status.dropped.saturating_sub(drop_base),
                status.active
            );
            peaks = [0.0; 2];
            if !status.alive {
                return Err("plugin instance closed".into());
            }
        }
        thread::sleep(Duration::from_millis(2));
    }
    if count == 0 {
        return Err(
            "connected but no audio blocks; check REAPER processing/monitoring/bypass".into(),
        );
    }
    println!(
        "Received {count} blocks; dropped since connect {}. Reception is not a calibrated timing measurement.",
        receiver.status().dropped.saturating_sub(drop_base)
    );
    Ok(())
}
