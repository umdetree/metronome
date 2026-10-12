//! CLAP passthrough effect with a cross-process dry-audio tap.
#![deny(unsafe_op_in_unsafe_fn)]

use audio_bridge::{
    FLAG_PLAYING, FLAG_STEADY_VALID, FLAG_TRANSPORT_VALID, MAX_FRAMES, Producer, Timing,
};
use clap_sys::{
    entry::clap_plugin_entry,
    events::*,
    ext::audio_ports::*,
    factory::plugin_factory::*,
    host::clap_host,
    plugin::*,
    process::*,
    version::{CLAP_VERSION, clap_version_is_compatible},
};
use std::{
    ffi::{CStr, c_char, c_void},
    ptr,
};

const ID: &CStr = c"org.metronome.dry-audio-bridge";
struct Features([*const c_char; 3]);
// SAFETY: all pointers refer to immutable static C strings or are null.
unsafe impl Sync for Features {}
static FEATURES: Features = Features([c"audio-effect".as_ptr(), c"analyzer".as_ptr(), ptr::null()]);
static DESCRIPTOR: clap_plugin_descriptor = clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: ID.as_ptr(),
    name: c"Metronome Dry Audio Bridge".as_ptr(),
    vendor: c"Metronome".as_ptr(),
    url: c"".as_ptr(),
    manual_url: c"".as_ptr(),
    support_url: c"".as_ptr(),
    version: c"0.1.0".as_ptr(),
    description: c"Zero-latency passthrough with a bounded dry-audio tap for the Metronome TUI"
        .as_ptr(),
    features: FEATURES.0.as_ptr(),
};

struct Instance {
    plugin: clap_plugin,
    producer: Option<Producer>,
    rate: u32,
    position: u64,
    expected_steady: Option<i64>,
    expected_transport: Option<f64>,
    max_frames: u32,
    processing: bool,
}

// SAFETY at every callback: CLAP supplies a live instance created by our factory,
// serializes lifecycle calls, and exclusively runs process/reset on the audio thread.
unsafe fn instance<'a>(plugin: *const clap_plugin) -> &'a mut Instance {
    unsafe { &mut *((*plugin).plugin_data.cast::<Instance>()) }
}

unsafe extern "C" fn init(plugin: *const clap_plugin) -> bool {
    let instance = unsafe { instance(plugin) };
    match Producer::create() {
        Ok(producer) => {
            instance.producer = Some(producer);
            true
        }
        Err(_) => false,
    }
}
unsafe extern "C" fn destroy(plugin: *const clap_plugin) {
    if !plugin.is_null() {
        // SAFETY: plugin_data is the exact Box allocation made by create().
        unsafe {
            drop(Box::from_raw((*plugin).plugin_data.cast::<Instance>()));
        }
    }
}
unsafe extern "C" fn activate(plugin: *const clap_plugin, rate: f64, min: u32, max: u32) -> bool {
    if !rate.is_finite()
        || !(8000.0..=384000.0).contains(&rate)
        || rate.fract() != 0.0
        || min == 0
        || max < min
    {
        return false;
    }
    let instance = unsafe { instance(plugin) };
    instance.rate = rate as u32;
    instance.position = 0;
    instance.expected_steady = None;
    instance.expected_transport = None;
    instance.max_frames = max;
    if let Some(producer) = &instance.producer {
        producer.activate(instance.rate, 2);
    }
    true
}
unsafe extern "C" fn deactivate(plugin: *const clap_plugin) {
    let instance = unsafe { instance(plugin) };
    instance.processing = false;
    if let Some(producer) = &instance.producer {
        producer.deactivate();
    }
}
unsafe extern "C" fn start(plugin: *const clap_plugin) -> bool {
    let instance = unsafe { instance(plugin) };
    instance.processing = true;
    if let Some(producer) = &instance.producer {
        producer.activate(instance.rate, 2);
    }
    true
}
unsafe extern "C" fn stop(plugin: *const clap_plugin) {
    let instance = unsafe { instance(plugin) };
    instance.processing = false;
    if let Some(producer) = &instance.producer {
        producer.deactivate();
    }
}
unsafe extern "C" fn reset(plugin: *const clap_plugin) {
    let instance = unsafe { instance(plugin) };
    instance.expected_steady = None;
    instance.expected_transport = None;
    if let Some(producer) = &instance.producer {
        producer.discontinuity();
    }
}

unsafe extern "C" fn process(
    plugin: *const clap_plugin,
    process: *const clap_process,
) -> clap_process_status {
    if process.is_null() {
        return CLAP_PROCESS_ERROR;
    }
    let state = unsafe { instance(plugin) };
    let process = unsafe { &*process };
    if !state.processing
        || process.frames_count > state.max_frames
        || process.audio_inputs_count != 1
        || process.audio_outputs_count != 1
        || process.audio_inputs.is_null()
        || process.audio_outputs.is_null()
    {
        return CLAP_PROCESS_ERROR;
    }
    let input = unsafe { &*process.audio_inputs };
    let output = unsafe { &mut *process.audio_outputs };
    let frames = process.frames_count as usize;
    if input.channel_count == 0 || output.channel_count == 0 {
        return CLAP_PROCESS_ERROR;
    }
    let double = input.data32.is_null();
    if (double && (input.data64.is_null() || output.data64.is_null()))
        || (!double && output.data32.is_null())
    {
        return CLAP_PROCESS_ERROR;
    }

    // Raw copies support CLAP's in-place buffers without creating aliasing slices.
    for channel in 0..output.channel_count as usize {
        if double {
            let destination = unsafe { *output.data64.add(channel) };
            if destination.is_null() {
                return CLAP_PROCESS_ERROR;
            }
            if channel < input.channel_count as usize {
                let source = unsafe { *input.data64.add(channel) };
                if source.is_null() {
                    return CLAP_PROCESS_ERROR;
                }
                unsafe {
                    ptr::copy(source, destination, frames);
                }
            } else {
                unsafe {
                    ptr::write_bytes(destination, 0, frames);
                }
            }
        } else {
            let destination = unsafe { *output.data32.add(channel) };
            if destination.is_null() {
                return CLAP_PROCESS_ERROR;
            }
            if channel < input.channel_count as usize {
                let source = unsafe { *input.data32.add(channel) };
                if source.is_null() {
                    return CLAP_PROCESS_ERROR;
                }
                unsafe {
                    ptr::copy(source, destination, frames);
                }
            } else {
                unsafe {
                    ptr::write_bytes(destination, 0, frames);
                }
            }
        }
    }
    output.constant_mask = input.constant_mask;
    let mut flags = 0;
    if process.steady_time >= 0 {
        flags |= FLAG_STEADY_VALID;
        if state
            .expected_steady
            .is_some_and(|expected| expected != process.steady_time)
            && let Some(producer) = &state.producer
        {
            producer.discontinuity();
        }
        state.expected_steady = process
            .steady_time
            .checked_add(i64::from(process.frames_count));
    } else {
        state.expected_steady = None;
    }
    let mut seconds = 0.0;
    if !process.transport.is_null() {
        let transport = unsafe { &*process.transport };
        if transport.flags & CLAP_TRANSPORT_HAS_SECONDS_TIMELINE != 0 {
            flags |= FLAG_TRANSPORT_VALID;
            seconds = transport.song_pos_seconds as f64 / (1_u64 << 31) as f64;
        }
        if transport.flags & CLAP_TRANSPORT_IS_PLAYING != 0 {
            flags |= FLAG_PLAYING;
        }
    }
    if flags & FLAG_TRANSPORT_VALID != 0 {
        if state
            .expected_transport
            .is_some_and(|expected| (expected - seconds).abs() > 2.0 / f64::from(state.rate))
            && let Some(producer) = &state.producer
        {
            producer.discontinuity();
        }
        state.expected_transport = Some(
            seconds
                + if flags & FLAG_PLAYING != 0 {
                    frames as f64 / f64::from(state.rate)
                } else {
                    0.0
                },
        );
    } else {
        state.expected_transport = None;
    }
    if let Some(producer) = &mut state.producer {
        let channels = input.channel_count.min(2) as usize;
        for offset in (0..frames).step_by(MAX_FRAMES) {
            let count = (frames - offset).min(MAX_FRAMES);
            let timing = Timing {
                position: state.position.wrapping_add(offset as u64),
                steady: if process.steady_time >= 0 {
                    process.steady_time.saturating_add(offset as i64)
                } else {
                    -1
                },
                transport_seconds: seconds
                    + if flags & FLAG_PLAYING != 0 {
                        offset as f64 / f64::from(state.rate)
                    } else {
                        0.0
                    },
                flags,
            };
            producer.push(count, channels, timing, |frame, channel| {
                if double {
                    unsafe { *(*input.data64.add(channel)).add(offset + frame) as f32 }
                } else {
                    unsafe { *(*input.data32.add(channel)).add(offset + frame) }
                }
            });
        }
    }
    state.position = state.position.wrapping_add(frames as u64);
    // Forward events without allocation; the tap is not an event processor.
    if !process.in_events.is_null() && !process.out_events.is_null() {
        let input = unsafe { &*process.in_events };
        let output = unsafe { &*process.out_events };
        if let (Some(size), Some(get), Some(push)) = (input.size, input.get, output.try_push) {
            for index in 0..unsafe { size(process.in_events) } {
                let event = unsafe { get(process.in_events, index) };
                if !event.is_null() {
                    unsafe {
                        push(process.out_events, event);
                    }
                }
            }
        }
    }
    CLAP_PROCESS_CONTINUE
}

unsafe extern "C" fn port_count(_plugin: *const clap_plugin, _input: bool) -> u32 {
    1
}
unsafe extern "C" fn port_get(
    _plugin: *const clap_plugin,
    index: u32,
    input: bool,
    info: *mut clap_audio_port_info,
) -> bool {
    if index != 0 || info.is_null() {
        return false;
    }
    let mut name = [0; 256];
    let label = if input {
        b"Dry input".as_slice()
    } else {
        b"Unchanged output".as_slice()
    };
    for (destination, source) in name.iter_mut().zip(label) {
        *destination = *source as c_char;
    }
    unsafe {
        *info = clap_audio_port_info {
            id: if input { 0 } else { 1 },
            name,
            flags: CLAP_AUDIO_PORT_IS_MAIN
                | CLAP_AUDIO_PORT_SUPPORTS_64BITS
                | CLAP_AUDIO_PORT_REQUIRES_COMMON_SAMPLE_SIZE,
            channel_count: 2,
            port_type: CLAP_PORT_STEREO.as_ptr(),
            in_place_pair: if input { 1 } else { 0 },
        };
    }
    true
}
static PORTS: clap_plugin_audio_ports = clap_plugin_audio_ports {
    count: Some(port_count),
    get: Some(port_get),
};
unsafe extern "C" fn extension(_plugin: *const clap_plugin, id: *const c_char) -> *const c_void {
    if !id.is_null() && unsafe { CStr::from_ptr(id) } == CLAP_EXT_AUDIO_PORTS {
        (&raw const PORTS).cast()
    } else {
        ptr::null()
    }
}
unsafe extern "C" fn main_thread(_plugin: *const clap_plugin) {}
unsafe extern "C" fn count(_factory: *const clap_plugin_factory) -> u32 {
    1
}
unsafe extern "C" fn descriptor(
    _factory: *const clap_plugin_factory,
    index: u32,
) -> *const clap_plugin_descriptor {
    if index == 0 {
        &raw const DESCRIPTOR
    } else {
        ptr::null()
    }
}
unsafe extern "C" fn create(
    _factory: *const clap_plugin_factory,
    host: *const clap_host,
    id: *const c_char,
) -> *const clap_plugin {
    if host.is_null()
        || id.is_null()
        || unsafe { CStr::from_ptr(id) } != ID
        || !clap_version_is_compatible(unsafe { (*host).clap_version })
    {
        return ptr::null();
    }
    let plugin = clap_plugin {
        desc: &DESCRIPTOR,
        plugin_data: ptr::null_mut(),
        init: Some(init),
        destroy: Some(destroy),
        activate: Some(activate),
        deactivate: Some(deactivate),
        start_processing: Some(start),
        stop_processing: Some(stop),
        reset: Some(reset),
        process: Some(process),
        get_extension: Some(extension),
        on_main_thread: Some(main_thread),
    };
    let state = Box::into_raw(Box::new(Instance {
        plugin,
        producer: None,
        rate: 0,
        position: 0,
        expected_steady: None,
        expected_transport: None,
        max_frames: 0,
        processing: false,
    }));
    unsafe {
        (*state).plugin.plugin_data = state.cast();
        &raw const (*state).plugin
    }
}
static FACTORY: clap_plugin_factory = clap_plugin_factory {
    get_plugin_count: Some(count),
    get_plugin_descriptor: Some(descriptor),
    create_plugin: Some(create),
};
unsafe extern "C" fn entry_init(_path: *const c_char) -> bool {
    true
}
unsafe extern "C" fn entry_deinit() {}
unsafe extern "C" fn factory(id: *const c_char) -> *const c_void {
    if !id.is_null() && unsafe { CStr::from_ptr(id) } == CLAP_PLUGIN_FACTORY_ID {
        (&raw const FACTORY).cast()
    } else {
        ptr::null()
    }
}

#[unsafe(no_mangle)]
pub static clap_entry: clap_plugin_entry = clap_plugin_entry {
    clap_version: CLAP_VERSION,
    init: Some(entry_init),
    deinit: Some(entry_deinit),
    get_factory: Some(factory),
};

#[cfg(test)]
mod tests {
    use super::*;
    use audio_bridge::{Block, Receiver};
    use clap_sys::audio_buffer::clap_audio_buffer;
    #[test]
    fn double_precision_in_place_audio_is_unchanged_when_reader_stalls() {
        unsafe {
            let host: clap_host = std::mem::zeroed();
            let host = clap_host {
                clap_version: CLAP_VERSION,
                ..host
            };
            let plugin = create(&FACTORY, &host, ID.as_ptr());
            let directory = tempfile::tempdir().unwrap();
            let producer = Producer::create_in(directory.path()).unwrap();
            let mut receiver = Receiver::connect(producer.source()).unwrap();
            instance(plugin).producer = Some(producer);
            assert!(activate(plugin, 44100.0, 1, 128));
            start(plugin);
            let mut left = [0.123456789012345_f64; 128];
            let mut right = [-0.987654321012345_f64; 128];
            let original_left = left;
            let original_right = right;
            let mut channels = [left.as_mut_ptr(), right.as_mut_ptr()];
            let input = clap_audio_buffer {
                data32: ptr::null_mut(),
                data64: channels.as_mut_ptr(),
                channel_count: 2,
                latency: 0,
                constant_mask: 3,
            };
            let mut output = input;
            let mut callback = clap_process {
                steady_time: 0,
                frames_count: 128,
                transport: ptr::null(),
                audio_inputs: &input,
                audio_outputs: &mut output,
                audio_inputs_count: 1,
                audio_outputs_count: 1,
                in_events: ptr::null(),
                out_events: ptr::null(),
            };
            for index in 0..100 {
                callback.steady_time = index * 128;
                assert_eq!(process(plugin, &callback), CLAP_PROCESS_CONTINUE);
            }
            assert_eq!(left, original_left);
            assert_eq!(right, original_right);
            assert!(receiver.status().dropped > 0);
            let mut block = Block::default();
            assert!(receiver.try_read(&mut block).unwrap());
            assert_eq!(block.samples[0], original_left[0] as f32);
            stop(plugin);
            assert!(!receiver.status().active);
            deactivate(plugin);
            destroy(plugin);
        }
    }
    #[test]
    fn clap_factory_and_32bit_passthrough_publish_original_audio() {
        unsafe {
            let host: clap_host = std::mem::zeroed();
            let host = clap_host {
                clap_version: CLAP_VERSION,
                ..host
            };
            let plugin = create(&FACTORY, &host, ID.as_ptr());
            assert!(!plugin.is_null());
            let directory = tempfile::tempdir().unwrap();
            let producer = Producer::create_in(directory.path()).unwrap();
            let mut receiver = Receiver::connect(producer.source()).unwrap();
            instance(plugin).producer = Some(producer);
            assert!(activate(plugin, 48000.0, 1, 4096));
            assert!(start(plugin));
            let mut left = vec![0.25_f32; 2048];
            let mut right = vec![-0.5_f32; 2048];
            let mut out_left = vec![0.0_f32; 2048];
            let mut out_right = vec![0.0_f32; 2048];
            let mut inputs = [left.as_mut_ptr(), right.as_mut_ptr()];
            let mut outputs = [out_left.as_mut_ptr(), out_right.as_mut_ptr()];
            let input = clap_audio_buffer {
                data32: inputs.as_mut_ptr(),
                data64: ptr::null_mut(),
                channel_count: 2,
                latency: 0,
                constant_mask: 0,
            };
            let mut output = clap_audio_buffer {
                data32: outputs.as_mut_ptr(),
                ..input
            };
            let callback = clap_process {
                steady_time: 100,
                frames_count: 2048,
                transport: ptr::null(),
                audio_inputs: &input,
                audio_outputs: &mut output,
                audio_inputs_count: 1,
                audio_outputs_count: 1,
                in_events: ptr::null(),
                out_events: ptr::null(),
            };
            assert_eq!(process(plugin, &callback), CLAP_PROCESS_CONTINUE);
            assert_eq!(out_left, left);
            assert_eq!(out_right, right);
            let mut block = Block::default();
            assert!(receiver.try_read(&mut block).unwrap());
            assert_eq!(block.frames, 1024);
            assert_eq!(&block.samples[..2], &[0.25, -0.5]);
            assert_eq!(block.timing.steady, 100);
            assert!(receiver.try_read(&mut block).unwrap());
            assert_eq!(block.timing.position, 1024);
            assert_eq!(block.timing.steady, 1124);
            stop(plugin);
            deactivate(plugin);
            destroy(plugin);
            assert!(!receiver.status().alive);
        }
    }
}
