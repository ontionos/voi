//! System audio capture on macOS via a Core Audio process tap (macOS 14.4+).
//!
//! Uses `objc2-core-audio` rather than `cidre`: cidre's build script shells out
//! to `xcodebuild`, which makes a full Xcode install a hard build requirement.
//! These bindings compile against the Command Line Tools alone.
//!
//! The aggregate device is built from the tap **alone**. Listing both
//! `sub_device_list` (the output device) and `tap_list` (a tap of that same
//! device) captures every sample twice and produces audible echo — a bug
//! meetily hit and documented, preserved here as a warning.
//!
//! Requires `NSAudioCaptureUsageDescription` in the host bundle's Info.plist.
//! Without the permission the tap still runs but delivers digital silence, so
//! callers should check for an all-zero signal rather than reporting an
//! inexplicably empty transcript.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use anyhow::{anyhow, Result};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::AnyThread;
use objc2_core_audio::{
    kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceIsStackedKey,
    kAudioAggregateDeviceMainSubDeviceKey, kAudioAggregateDeviceNameKey,
    kAudioAggregateDeviceTapAutoStartKey, kAudioAggregateDeviceTapListKey,
    kAudioAggregateDeviceUIDKey, kAudioDevicePropertyDeviceIsAlive, kAudioDevicePropertyDeviceUID,
    kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject, kAudioSubTapUIDKey,
    kAudioTapPropertyFormat, AudioDeviceCreateIOProcIDWithBlock, AudioDeviceDestroyIOProcID,
    AudioDeviceIOProcID, AudioDeviceStart, AudioDeviceStop, AudioHardwareCreateAggregateDevice,
    AudioHardwareCreateProcessTap, AudioHardwareDestroyAggregateDevice,
    AudioHardwareDestroyProcessTap, AudioObjectGetPropertyData, AudioObjectID,
    AudioObjectPropertyAddress, AudioObjectPropertySelector, CATapDescription,
};
use objc2_core_audio_types::AudioStreamBasicDescription;
use objc2_core_foundation::{CFDictionary, CFString};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString};
use ringbuf::{
    traits::{Consumer, Producer, Split},
    HeapCons, HeapProd,
    HeapRb,
};
use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::{AudioChunk, AudioStream};

/// Signature Core Audio invokes for each IO cycle.
type IoBlock = RcBlock<
    dyn Fn(
        NonNull<objc2_core_audio_types::AudioTimeStamp>,
        NonNull<objc2_core_audio_types::AudioBufferList>,
        NonNull<objc2_core_audio_types::AudioTimeStamp>,
        NonNull<objc2_core_audio_types::AudioBufferList>,
        NonNull<objc2_core_audio_types::AudioTimeStamp>,
    ),
>;

/// The default output device now: its id and its UID. A device that drops
/// out and comes back keeps its UID but gets a new id.
unsafe fn default_output() -> Result<(AudioObjectID, String)> {
    let output_id: AudioObjectID =
        get_prop(kAudioObjectSystemObject as AudioObjectID, kAudioHardwarePropertyDefaultOutputDevice)
            .map_err(|e| anyhow!("no default output device: {e}"))?;
    let output_uid_cf: *const CFString = get_prop(output_id, kAudioDevicePropertyDeviceUID)?;
    let output_uid = NonNull::new(output_uid_cf as *mut CFString)
        .ok_or_else(|| anyhow!("output device has no UID"))?
        .as_ref()
        .to_string();
    Ok((output_id, output_uid))
}

/// Whether Core Audio still has `device`. A device that went away answers
/// no, or not at all.
fn alive(device: AudioObjectID) -> bool {
    unsafe { get_prop::<u32>(device, kAudioDevicePropertyDeviceIsAlive).map(|v| v != 0).unwrap_or(false) }
}

/// How the supervisor opens the tap and asks what the output device is.
struct Mac;

impl crate::supervise::Platform for Mac {
    fn open(&mut self) -> Result<crate::supervise::Opened> {
        unsafe {
            let (id, uid) = default_output()?;
            let (stream, aggregate) = SystemSource::start_inner(&uid)?;
            // The output device the tap is clocked by, and the tap's own
            // device: when either dies — AirPods dropping out — so does the
            // capture, whatever it still delivers.
            let check = move || {
                for (what, device) in [("the output device", id), ("the tap's aggregate device", aggregate)] {
                    if !alive(device) {
                        return Some(format!("{what} ({device}) is gone"));
                    }
                }
                None
            };
            Ok(crate::supervise::Opened {
                stream,
                device: format!("{uid}#{id}"),
                failed: Default::default(),
                check: Some(Box::new(check)),
            })
        }
    }

    fn current_device(&mut self) -> Option<String> {
        unsafe { default_output().ok().map(|(id, uid)| format!("{uid}#{id}")) }
    }
}

/// Roughly 2.7s of headroom at 48kHz — enough to ride out a scheduling hiccup
/// without letting a stalled consumer grow memory without bound.
const RING_CAPACITY: usize = 128 * 1024;
const DRAIN_CHUNK_MS: u64 = 100;

pub struct SystemSource;

struct SystemGuard {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    device: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    tap: AudioObjectID,
    /// The IO block must outlive the running device.
    _block: IoBlock,
}

impl Drop for SystemGuard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        unsafe {
            AudioDeviceStop(self.device, self.proc_id);
            AudioDeviceDestroyIOProcID(self.device, self.proc_id);
            AudioHardwareDestroyAggregateDevice(self.device);
            AudioHardwareDestroyProcessTap(self.tap);
        }
        debug!("system audio tap torn down");
    }
}

fn addr(selector: AudioObjectPropertySelector) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    }
}

/// Read a fixed-size Core Audio property.
unsafe fn get_prop<T>(id: AudioObjectID, selector: AudioObjectPropertySelector) -> Result<T> {
    let address = addr(selector);
    let mut size = std::mem::size_of::<T>() as u32;
    let mut out = std::mem::MaybeUninit::<T>::uninit();
    let status = AudioObjectGetPropertyData(
        id,
        NonNull::from(&address),
        0,
        std::ptr::null(),
        NonNull::from(&mut size),
        NonNull::new_unchecked(out.as_mut_ptr() as *mut c_void),
    );
    if status != 0 {
        return Err(anyhow!("Core Audio property {selector:#x} failed with status {status}"));
    }
    Ok(out.assume_init())
}

fn key(k: &std::ffi::CStr) -> Retained<NSString> {
    NSString::from_str(k.to_str().expect("Core Audio keys are ASCII"))
}

impl SystemSource {
    /// Capture everything the Mac plays, for as long as the stream lives —
    /// opened again on whatever the output device is now whenever the one it
    /// was clocked by goes away (see [`crate::supervise`]).
    pub fn start() -> Result<AudioStream> {
        crate::supervise::supervise(Mac, crate::supervise::Timing::default())
    }

    /// The tap on the device `output_uid`, and the aggregate device holding it.
    unsafe fn start_inner(output_uid: &str) -> Result<(AudioStream, AudioObjectID)> {

        // 2. A mono tap of every process. Mono is both what we want for speech
        //    and the more reliable configuration on macOS.
        let empty: Retained<NSArray<NSNumber>> = NSArray::new();
        let desc = CATapDescription::initMonoGlobalTapButExcludeProcesses(
            CATapDescription::alloc(),
            &empty,
        );
        desc.setName(&NSString::from_str("voi-system-tap"));
        desc.setPrivate(true);

        let mut tap: AudioObjectID = 0;
        let status = AudioHardwareCreateProcessTap(Some(&desc), &mut tap);
        if status != 0 {
            return Err(anyhow!(
                "failed to create process tap (status {status}) — grant System Audio \
                 Recording in System Settings › Privacy & Security, and note this \
                 requires macOS 14.4 or later"
            ));
        }

        let tap_uid = desc.UUID().UUIDString().to_string();

        // 3. Format of the tap tells us the rate and channel count to declare.
        let asbd: AudioStreamBasicDescription = get_prop(tap, kAudioTapPropertyFormat)
            .map_err(|e| {
                AudioHardwareDestroyProcessTap(tap);
                anyhow!("failed to read tap format: {e}")
            })?;
        let sample_rate = asbd.mSampleRate as u32;
        let channels = asbd.mChannelsPerFrame as u16;
        debug!(sample_rate, channels, output_uid = %output_uid, "system audio tap created");

        // 4. Private aggregate device wrapping the tap — and only the tap.
        let sub_tap: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(
            &[&*key(kAudioSubTapUIDKey)],
            &[&*NSString::from_str(&tap_uid) as &AnyObject],
        );
        let tap_list: Retained<NSArray<AnyObject>> =
            NSArray::from_slice(&[&*sub_tap as &AnyObject]);

        let agg_uid = format!("voi-agg-{tap_uid}");
        let agg: Retained<NSDictionary<NSString, AnyObject>> = NSDictionary::from_slices(
            &[
                &*key(kAudioAggregateDeviceNameKey),
                &*key(kAudioAggregateDeviceUIDKey),
                &*key(kAudioAggregateDeviceMainSubDeviceKey),
                &*key(kAudioAggregateDeviceIsPrivateKey),
                &*key(kAudioAggregateDeviceIsStackedKey),
                &*key(kAudioAggregateDeviceTapAutoStartKey),
                &*key(kAudioAggregateDeviceTapListKey),
            ],
            &[
                &*NSString::from_str("Voi System Tap") as &AnyObject,
                &*NSString::from_str(&agg_uid) as &AnyObject,
                &*NSString::from_str(&output_uid) as &AnyObject,
                &*NSNumber::new_bool(true) as &AnyObject,
                &*NSNumber::new_bool(false) as &AnyObject,
                &*NSNumber::new_bool(true) as &AnyObject,
                &*tap_list as &AnyObject,
            ],
        );

        // NSDictionary is toll-free bridged with CFDictionary.
        let agg_cf = &*(Retained::as_ptr(&agg) as *const CFDictionary);
        let mut device: AudioObjectID = 0;
        let status = AudioHardwareCreateAggregateDevice(agg_cf, NonNull::from(&mut device));
        if status != 0 {
            AudioHardwareDestroyProcessTap(tap);
            return Err(anyhow!("failed to create aggregate device (status {status})"));
        }

        // 5. Realtime IO block: push into the lock-free ring and return. No
        //    allocation, no locking, no blocking.
        let rb = HeapRb::<f32>::new(RING_CAPACITY);
        let (producer, consumer) = rb.split();
        let producer = RtProducer(std::cell::UnsafeCell::new(producer));
        let overruns = Arc::new(AtomicU32::new(0));
        let overruns_cb = overruns.clone();

        let block = RcBlock::new(
            move |_now: NonNull<objc2_core_audio_types::AudioTimeStamp>,
                  input: NonNull<objc2_core_audio_types::AudioBufferList>,
                  _in_time: NonNull<objc2_core_audio_types::AudioTimeStamp>,
                  _output: NonNull<objc2_core_audio_types::AudioBufferList>,
                  _out_time: NonNull<objc2_core_audio_types::AudioTimeStamp>| {
                let abl = input.as_ref();
                if abl.mNumberBuffers == 0 {
                    return;
                }
                let buf = &abl.mBuffers[0];
                if buf.mData.is_null() {
                    return;
                }
                let n = buf.mDataByteSize as usize / std::mem::size_of::<f32>();
                let data = std::slice::from_raw_parts(buf.mData as *const f32, n);
                // Safety: Core Audio invokes this block on one realtime thread
                // at a time, so this is the only live reference to the producer.
                let prod = &mut *producer.0.get();
                if prod.push_slice(data) < n {
                    overruns_cb.fetch_add(1, Ordering::Relaxed);
                }
            },
        );

        let mut proc_id: AudioDeviceIOProcID = None;
        let status = AudioDeviceCreateIOProcIDWithBlock(
            NonNull::from(&mut proc_id),
            device,
            None,
            &*block as *const _ as *mut _,
        );
        if status != 0 {
            AudioHardwareDestroyAggregateDevice(device);
            AudioHardwareDestroyProcessTap(tap);
            return Err(anyhow!("failed to create IO proc (status {status})"));
        }

        let status = AudioDeviceStart(device, proc_id);
        if status != 0 {
            AudioDeviceDestroyIOProcID(device, proc_id);
            AudioHardwareDestroyAggregateDevice(device);
            AudioHardwareDestroyProcessTap(tap);
            return Err(anyhow!("failed to start aggregate device (status {status})"));
        }

        let (tx, rx) = mpsc::channel::<AudioChunk>(64);
        let stop = Arc::new(AtomicBool::new(false));
        let thread = spawn_drain(consumer, tx, sample_rate, channels, overruns, stop.clone());

        let guard = SystemGuard {
            stop,
            thread: Some(thread),
            device,
            proc_id,
            tap,
            _block: block,
        };
        Ok((AudioStream::new(rx, sample_rate, channels, Box::new(guard)), device))
    }
}

/// Drains the ring into `~DRAIN_CHUNK_MS` blocks on a normal (non-realtime) thread.
fn spawn_drain(
    mut consumer: HeapCons<f32>,
    tx: mpsc::Sender<AudioChunk>,
    sample_rate: u32,
    channels: u16,
    overruns: Arc<AtomicU32>,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("voi-system-drain".into())
        .spawn(move || {
            let want =
                (sample_rate as u64 * DRAIN_CHUNK_MS / 1000) as usize * channels.max(1) as usize;
            let mut scratch = vec![0.0f32; want.max(1024)];
            let take = want.min(scratch.len());
            let mut reported = 0u32;

            while !stop.load(Ordering::Acquire) {
                let n = consumer.pop_slice(&mut scratch[..take]);
                if n == 0 {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                    continue;
                }
                let chunk = AudioChunk {
                    samples: scratch[..n].to_vec(),
                    sample_rate,
                    channels,
                };
                if tx.blocking_send(chunk).is_err() {
                    return; // consumer gone
                }
                let o = overruns.load(Ordering::Relaxed);
                if o > reported {
                    warn!(overruns = o, "system audio ring overrun — samples dropped");
                    reported = o;
                }
            }
        })
        .expect("failed to spawn system audio drain thread")
}

// Safety: the guard owns Core Audio object IDs (plain integers) and a retained
// block. None of it is thread-affine.
unsafe impl Send for SystemGuard {}

/// Ring producer reachable from the realtime IO block.
///
/// Safety: Core Audio serialises IO block invocations for a given IOProc, so
/// there is never more than one concurrent writer.
struct RtProducer(std::cell::UnsafeCell<HeapProd<f32>>);
unsafe impl Send for RtProducer {}
unsafe impl Sync for RtProducer {}
