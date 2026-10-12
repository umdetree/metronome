//! Exercise real OS mappings/locks between separate processes, not just threads.
use audio_bridge::{Block, Producer, Receiver, Source, Timing};
use std::{
    fs,
    process::Command,
    thread,
    time::{Duration, Instant},
};

#[test]
fn child_receiver() {
    let Some(path) = std::env::var_os("METRONOME_IPC_TEST_PATH") else {
        return;
    };
    let path = std::path::PathBuf::from(path);
    let source = Source {
        id: "test".into(),
        path: path.clone(),
    };
    let mut receiver = Receiver::connect(&source).unwrap();
    fs::write(path.with_extension("ready"), b"ready").unwrap();
    let start = Instant::now();
    let mut block = Block::default();
    while start.elapsed() < Duration::from_secs(5) {
        if receiver.try_read(&mut block).unwrap() {
            assert_eq!(block.frames, 4);
            assert_eq!(block.sample_rate, 48000);
            assert_eq!(block.timing.position, 1234);
            assert_eq!(
                &block.samples[..8],
                &[0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]
            );
            return;
        }
        thread::sleep(Duration::from_millis(2));
    }
    panic!("cross-process audio did not arrive");
}

#[test]
fn separate_process_receives_audio() {
    let directory = tempfile::tempdir().unwrap();
    let mut producer = Producer::create_in(directory.path()).unwrap();
    producer.activate(48000, 2);
    let path = producer.source().path.clone();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_receiver", "--nocapture"])
        .env("METRONOME_IPC_TEST_PATH", &path)
        .spawn()
        .unwrap();
    let start = Instant::now();
    while !path.with_extension("ready").exists() && start.elapsed() < Duration::from_secs(5) {
        thread::sleep(Duration::from_millis(2));
    }
    assert!(
        path.with_extension("ready").exists(),
        "child did not connect"
    );
    assert!(producer.push(
        4,
        2,
        Timing {
            position: 1234,
            ..Timing::default()
        },
        |frame, channel| (frame * 2 + channel) as f32
    ));
    assert!(child.wait().unwrap().success());
}
