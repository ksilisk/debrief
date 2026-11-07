//! One app's sound through a Core Audio process tap. Unlike ScreenCaptureKit's
//! app filter, it hears the app's helper processes too (Chrome plays from those).

use std::ffi::CStr;
use std::mem::{MaybeUninit, size_of};
use std::ptr::{NonNull, null};
use std::slice;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_core_audio::{
    AudioConvertHostTimeToNanos, AudioDeviceCreateIOProcIDWithBlock, AudioDeviceDestroyIOProcID,
    AudioDeviceIOProcID, AudioDeviceStart, AudioDeviceStop, AudioHardwareCreateAggregateDevice,
    AudioHardwareCreateProcessTap, AudioHardwareDestroyAggregateDevice,
    AudioHardwareDestroyProcessTap, AudioObjectAddPropertyListenerBlock,
    AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectID,
    AudioObjectPropertyAddress, AudioObjectPropertySelector,
    AudioObjectRemovePropertyListenerBlock, AudioObjectSetPropertyData, CATapDescription,
    kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceIsStackedKey,
    kAudioAggregateDeviceNameKey, kAudioAggregateDeviceTapAutoStartKey,
    kAudioAggregateDeviceTapListKey, kAudioAggregateDeviceUIDKey,
    kAudioDevicePropertyDeviceIsAlive, kAudioDevicePropertyNominalSampleRate,
    kAudioHardwarePropertyProcessObjectList, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject, kAudioProcessPropertyBundleID,
    kAudioSubTapDriftCompensationKey, kAudioSubTapUIDKey, kAudioTapPropertyDescription,
};
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioTimeStamp, AudioTimeStampFlags};
use objc2_core_foundation::{CFDictionary, CFRetained, CFString};
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSUUID};

use crate::{AudioChunk, CaptureError, Handler, Source};

const SYSTEM: AudioObjectID = kAudioObjectSystemObject as AudioObjectID;
const NAME: &str = "Debrief";
const WORKER_POLL: Duration = Duration::from_millis(100);

type IoBlock = RcBlock<
    dyn Fn(
        NonNull<AudioTimeStamp>,
        NonNull<AudioBufferList>,
        NonNull<AudioTimeStamp>,
        NonNull<AudioBufferList>,
        NonNull<AudioTimeStamp>,
    ),
>;
type ListenerBlock = RcBlock<dyn Fn(u32, NonNull<AudioObjectPropertyAddress>)>;

struct Chunk {
    host_time: u64,
    sample_rate: u32,
    samples: Vec<f32>,
}

pub struct Tap {
    tap: AudioObjectID,
    device: AudioObjectID,
    io_proc: AudioDeviceIOProcID,
    io_block: Option<IoBlock>,
    queue: DispatchRetained<DispatchQueue>,
    listeners: Vec<(AudioObjectID, AudioObjectPropertySelector, ListenerBlock)>,
    running: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

// SAFETY: the blocks are kept only to unregister them in `drop`; Core Audio
// already calls them from its own threads.
unsafe impl Send for Tap {}
unsafe impl Sync for Tap {}

impl Tap {
    /// Starts at once, even if the app is not running yet: its processes are
    /// added whenever they appear.
    pub fn start(bundle_id: &str, handler: Arc<dyn Handler>) -> Result<Self, CaptureError> {
        let processes = app_processes(bundle_id)?;
        let description = unsafe {
            CATapDescription::initMonoMixdownOfProcesses(
                CATapDescription::alloc(),
                &number_array(&processes),
            )
        };
        unsafe {
            description.setName(&NSString::from_str(NAME));
            description.setPrivate(true);
        }

        let running = Arc::new(AtomicBool::new(true));
        let mut this = Self {
            tap: 0,
            device: 0,
            io_proc: None,
            io_block: None,
            queue: DispatchQueue::new("debrief.tap", None),
            listeners: Vec::new(),
            running: Arc::clone(&running),
            worker: None,
        };

        check(
            unsafe { AudioHardwareCreateProcessTap(Some(&description), &mut this.tap) },
            "Creating the audio tap",
        )?;
        let tap_uid = unsafe { description.UUID().UUIDString() };
        this.device = create_device(&tap_uid)?;
        let rate = Arc::new(AtomicU32::new(nominal_rate(this.device)?));

        let (sender, receiver) = mpsc::channel();
        let io_rate = Arc::clone(&rate);
        let io_block: IoBlock = RcBlock::new(
            move |_now: NonNull<AudioTimeStamp>,
                  input: NonNull<AudioBufferList>,
                  input_time: NonNull<AudioTimeStamp>,
                  _output: NonNull<AudioBufferList>,
                  _output_time: NonNull<AudioTimeStamp>| {
                let (list, time) = unsafe { (input.as_ref(), input_time.as_ref()) };
                if time.mFlags.0 & AudioTimeStampFlags::HostTimeValid.0 == 0 {
                    return;
                }
                let buffers = unsafe {
                    slice::from_raw_parts(list.mBuffers.as_ptr(), list.mNumberBuffers as usize)
                };
                let samples = mono(buffers);
                if !samples.is_empty() {
                    let _ = sender.send(Chunk {
                        host_time: time.mHostTime,
                        sample_rate: io_rate.load(Ordering::Relaxed),
                        samples,
                    });
                }
            },
        );
        check(
            unsafe {
                AudioDeviceCreateIOProcIDWithBlock(
                    NonNull::from(&mut this.io_proc),
                    this.device,
                    None,
                    RcBlock::as_ptr(&io_block),
                )
            },
            "Connecting to the audio tap",
        )?;
        this.io_block = Some(io_block);

        let worker_handler = Arc::clone(&handler);
        this.worker = Some(
            thread::Builder::new()
                .name("debrief-tap".into())
                .spawn(move || forward(&receiver, &running, worker_handler.as_ref()))
                .map_err(|error| CaptureError::Platform(error.to_string()))?,
        );

        this.follow_app(bundle_id, processes, description)?;
        let device = this.device;
        this.listen(device, kAudioDevicePropertyNominalSampleRate, move || {
            if let Ok(value) = nominal_rate(device) {
                rate.store(value, Ordering::Relaxed);
            }
        })?;
        this.listen(device, kAudioDevicePropertyDeviceIsAlive, move || {
            if get::<u32>(device, kAudioDevicePropertyDeviceIsAlive).is_ok_and(|alive| alive == 0) {
                handler.failed("System audio stopped: the audio tap went away".into());
            }
        })?;

        check(
            unsafe { AudioDeviceStart(this.device, this.io_proc) },
            "Starting the audio tap",
        )?;
        Ok(this)
    }

    fn follow_app(
        &mut self,
        bundle_id: &str,
        processes: Vec<AudioObjectID>,
        description: Retained<CATapDescription>,
    ) -> Result<(), CaptureError> {
        let (tap, bundle_id, tapped) = (self.tap, bundle_id.to_owned(), Mutex::new(processes));
        self.listen(SYSTEM, kAudioHardwarePropertyProcessObjectList, move || {
            let Ok(processes) = app_processes(&bundle_id) else {
                return;
            };
            let mut tapped = tapped.lock().unwrap_or_else(PoisonError::into_inner);
            if *tapped == processes {
                return;
            }
            unsafe { description.setProcesses(&number_array(&processes)) };
            if set_description(tap, &description).is_ok() {
                *tapped = processes;
            }
        })
    }

    fn listen(
        &mut self,
        object: AudioObjectID,
        selector: AudioObjectPropertySelector,
        handler: impl Fn() + 'static,
    ) -> Result<(), CaptureError> {
        let block: ListenerBlock = RcBlock::new(
            move |_count: u32, _addresses: NonNull<AudioObjectPropertyAddress>| handler(),
        );
        let address = address(selector);
        check(
            unsafe {
                AudioObjectAddPropertyListenerBlock(
                    object,
                    NonNull::from(&address),
                    Some(&*self.queue),
                    RcBlock::as_ptr(&block),
                )
            },
            "Watching the audio tap",
        )?;
        self.listeners.push((object, selector, block));
        Ok(())
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        for (object, selector, block) in self.listeners.drain(..) {
            let address = address(selector);
            unsafe {
                AudioObjectRemovePropertyListenerBlock(
                    object,
                    NonNull::from(&address),
                    Some(&*self.queue),
                    RcBlock::as_ptr(&block),
                );
            }
        }
        unsafe {
            if self.io_proc.is_some() {
                AudioDeviceStop(self.device, self.io_proc);
                AudioDeviceDestroyIOProcID(self.device, self.io_proc);
            }
            if self.device != 0 {
                AudioHardwareDestroyAggregateDevice(self.device);
            }
            if self.tap != 0 {
                AudioHardwareDestroyProcessTap(self.tap);
            }
        }
        self.io_block = None;
        self.running.store(false, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Matches the app itself and its helpers, whose bundle IDs start with `<app>.`.
fn belongs_to(bundle_id: &str, app: &str) -> bool {
    bundle_id == app
        || bundle_id
            .strip_prefix(app)
            .is_some_and(|rest| rest.starts_with('.'))
}

fn forward(receiver: &Receiver<Chunk>, running: &AtomicBool, handler: &dyn Handler) {
    loop {
        match receiver.recv_timeout(WORKER_POLL) {
            Ok(chunk) => handler.audio(AudioChunk {
                source: Source::System,
                time: unsafe { AudioConvertHostTimeToNanos(chunk.host_time) } as f64 / 1e9,
                sample_rate: chunk.sample_rate,
                samples: &chunk.samples,
            }),
            Err(RecvTimeoutError::Timeout) if running.load(Ordering::Acquire) => {}
            Err(_) => return,
        }
    }
}

/// The device the tap is read through: private to this process.
fn create_device(tap_uid: &NSString) -> Result<AudioObjectID, CaptureError> {
    let tap = NSDictionary::<NSString, AnyObject>::from_slices(
        &[
            &*key(kAudioSubTapUIDKey),
            &*key(kAudioSubTapDriftCompensationKey),
        ],
        &[any(tap_uid), any(&*NSNumber::new_bool(true))],
    );
    let description = NSDictionary::<NSString, AnyObject>::from_slices(
        &[
            &*key(kAudioAggregateDeviceNameKey),
            &*key(kAudioAggregateDeviceUIDKey),
            &*key(kAudioAggregateDeviceIsPrivateKey),
            &*key(kAudioAggregateDeviceIsStackedKey),
            &*key(kAudioAggregateDeviceTapAutoStartKey),
            &*key(kAudioAggregateDeviceTapListKey),
        ],
        &[
            any(&*NSString::from_str(NAME)),
            any(&*NSUUID::new().UUIDString()),
            any(&*NSNumber::new_bool(true)),
            any(&*NSNumber::new_bool(false)),
            any(&*NSNumber::new_bool(false)),
            any(&*NSArray::from_retained_slice(&[tap])),
        ],
    );
    let description = unsafe { &*(Retained::as_ptr(&description) as *const CFDictionary) };
    let mut device = 0;
    check(
        unsafe { AudioHardwareCreateAggregateDevice(description, NonNull::from(&mut device)) },
        "Creating the audio tap device",
    )?;
    Ok(device)
}

fn app_processes(bundle_id: &str) -> Result<Vec<AudioObjectID>, CaptureError> {
    Ok(get_list(SYSTEM, kAudioHardwarePropertyProcessObjectList)?
        .into_iter()
        .filter(|&process| {
            get_string(process, kAudioProcessPropertyBundleID)
                .is_ok_and(|id| belongs_to(&id, bundle_id))
        })
        .collect())
}

fn set_description(tap: AudioObjectID, description: &CATapDescription) -> Result<(), CaptureError> {
    let address = address(kAudioTapPropertyDescription);
    let pointer: *const CATapDescription = description;
    check(
        unsafe {
            AudioObjectSetPropertyData(
                tap,
                NonNull::from(&address),
                0,
                null(),
                size_of::<*const CATapDescription>() as u32,
                NonNull::from(&pointer).cast(),
            )
        },
        "Updating the audio tap",
    )
}

fn nominal_rate(device: AudioObjectID) -> Result<u32, CaptureError> {
    get::<f64>(device, kAudioDevicePropertyNominalSampleRate).map(|rate| rate.round() as u32)
}

fn mono(buffers: &[AudioBuffer]) -> Vec<f32> {
    let mut mono: Vec<f32> = Vec::new();
    let mut channels = 0;
    for buffer in buffers {
        if buffer.mData.is_null() || buffer.mNumberChannels == 0 {
            continue;
        }
        let count = buffer.mNumberChannels as usize;
        let values = unsafe {
            slice::from_raw_parts(
                buffer.mData as *const f32,
                buffer.mDataByteSize as usize / size_of::<f32>(),
            )
        };
        if mono.is_empty() {
            mono = vec![0.0; values.len() / count];
        }
        for (frame, values) in mono.iter_mut().zip(values.chunks_exact(count)) {
            *frame += values.iter().sum::<f32>();
        }
        channels += count;
    }
    if channels > 1 {
        let scale = 1.0 / channels as f32;
        mono.iter_mut().for_each(|value| *value *= scale);
    }
    mono
}

fn number_array(ids: &[AudioObjectID]) -> Retained<NSArray<NSNumber>> {
    let numbers: Vec<Retained<NSNumber>> = ids.iter().map(|&id| NSNumber::new_u32(id)).collect();
    NSArray::from_retained_slice(&numbers)
}

fn key(name: &CStr) -> Retained<NSString> {
    NSString::from_str(&name.to_string_lossy())
}

fn any<T: AsRef<AnyObject>>(object: &T) -> &AnyObject {
    object.as_ref()
}

fn address(selector: AudioObjectPropertySelector) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    }
}

fn get<T: Copy>(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
) -> Result<T, CaptureError> {
    let address = address(selector);
    let mut size = size_of::<T>() as u32;
    let mut value = MaybeUninit::<T>::uninit();
    check(
        unsafe {
            AudioObjectGetPropertyData(
                object,
                NonNull::from(&address),
                0,
                null(),
                NonNull::from(&mut size),
                NonNull::new_unchecked(value.as_mut_ptr().cast()),
            )
        },
        "Reading an audio property",
    )?;
    Ok(unsafe { value.assume_init() })
}

fn get_string(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
) -> Result<String, CaptureError> {
    let raw: *const CFString = get(object, selector)?;
    Ok(NonNull::new(raw.cast_mut())
        .map(|raw| unsafe { CFRetained::from_raw(raw) }.to_string())
        .unwrap_or_default())
}

fn get_list(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
) -> Result<Vec<AudioObjectID>, CaptureError> {
    let address = address(selector);
    let mut size = 0u32;
    check(
        unsafe {
            AudioObjectGetPropertyDataSize(
                object,
                NonNull::from(&address),
                0,
                null(),
                NonNull::from(&mut size),
            )
        },
        "Reading an audio property",
    )?;
    let mut ids: Vec<AudioObjectID> = vec![0; size as usize / size_of::<AudioObjectID>()];
    if ids.is_empty() {
        return Ok(ids);
    }
    check(
        unsafe {
            AudioObjectGetPropertyData(
                object,
                NonNull::from(&address),
                0,
                null(),
                NonNull::from(&mut size),
                NonNull::new_unchecked(ids.as_mut_ptr().cast()),
            )
        },
        "Reading an audio property",
    )?;
    ids.truncate(size as usize / size_of::<AudioObjectID>());
    Ok(ids)
}

fn check(status: i32, action: &str) -> Result<(), CaptureError> {
    if status == 0 {
        return Ok(());
    }
    let bytes = status.to_be_bytes();
    let code = if bytes.iter().all(u8::is_ascii_graphic) {
        format!("'{}'", String::from_utf8_lossy(&bytes))
    } else {
        status.to_string()
    };
    Err(CaptureError::Platform(format!(
        "{action} failed (Core Audio error {code})"
    )))
}

#[cfg(test)]
mod tests {
    use std::ffi::c_void;

    use super::*;

    #[test]
    fn helper_processes_belong_to_their_app() {
        assert!(belongs_to("com.google.Chrome", "com.google.Chrome"));
        assert!(belongs_to("com.google.Chrome.helper", "com.google.Chrome"));
        assert!(!belongs_to("com.google.ChromeCanary", "com.google.Chrome"));
        assert!(!belongs_to("us.zoom.xos", "com.google.Chrome"));
    }

    #[test]
    fn mixes_channels_down_to_mono() {
        let mut stereo = [0.5f32, -0.5, 1.0, 0.0];
        let buffer = AudioBuffer {
            mNumberChannels: 2,
            mDataByteSize: size_of_val(&stereo) as u32,
            mData: stereo.as_mut_ptr().cast::<c_void>(),
        };
        assert_eq!(mono(&[buffer]), vec![0.0, 0.5]);
    }

    #[test]
    fn explains_core_audio_errors() {
        let error = check(i32::from_be_bytes(*b"who?"), "Creating the audio tap");
        assert_eq!(
            error.map_err(|e| e.to_string()),
            Err("Creating the audio tap failed (Core Audio error 'who?')".into())
        );
        assert!(check(0, "x").is_ok());
    }
}
