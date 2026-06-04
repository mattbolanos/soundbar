use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use objc2::rc::Retained;
use objc2_core_audio::{
    AudioDeviceCreateIOProcID, AudioDeviceDestroyIOProcID, AudioDeviceIOProcID, AudioDeviceStart,
    AudioDeviceStop, AudioHardwareCreateAggregateDevice, AudioHardwareCreateProcessTap,
    AudioHardwareDestroyAggregateDevice, AudioHardwareDestroyProcessTap, AudioObjectID,
    CATapDescription, CATapMuteBehavior, kAudioAggregateDeviceIsPrivateKey,
    kAudioAggregateDeviceNameKey, kAudioAggregateDeviceTapAutoStartKey,
    kAudioAggregateDeviceTapListKey, kAudioAggregateDeviceUIDKey, kAudioObjectUnknown,
    kAudioSubTapUIDKey,
};
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioTimeStamp};
use objc2_core_foundation::{
    CFArray, CFDictionary, CFString, CFType, CFRetained, kCFBooleanTrue,
};
use objc2_foundation::NSString;

use crate::audio::buffer::AudioProducer;

pub struct Capture {
    tap_id: AudioObjectID,
    aggregate_id: AudioObjectID,
    io_proc_id: AudioDeviceIOProcID,
    callback_state: Box<CallbackState>,
}

struct CallbackState {
    producer: AudioProducer,
    running: Arc<AtomicBool>,
}

impl Capture {
    pub fn start(producer: AudioProducer, running: Arc<AtomicBool>) -> Result<Self> {
        let tap_description = create_tap_description();
        let mut tap_id = kAudioObjectUnknown;
        check_status(
            unsafe { AudioHardwareCreateProcessTap(Some(&tap_description), &mut tap_id) },
            "AudioHardwareCreateProcessTap",
        )?;

        let aggregate_id = match create_aggregate_device(&tap_description) {
            Ok(id) => id,
            Err(error) => {
                unsafe {
                    AudioHardwareDestroyProcessTap(tap_id);
                }
                return Err(error);
            }
        };

        let mut callback_state = Box::new(CallbackState { producer, running });
        let mut io_proc_id = None;
        let out_io_proc_id = NonNull::new(&mut io_proc_id as *mut AudioDeviceIOProcID)
            .context("failed to build IOProc output pointer")?;

        if let Err(error) = check_status(
            unsafe {
                AudioDeviceCreateIOProcID(
                    aggregate_id,
                    Some(audio_io_proc),
                    callback_state.as_mut() as *mut CallbackState as *mut c_void,
                    out_io_proc_id,
                )
            },
            "AudioDeviceCreateIOProcID",
        ) {
            unsafe {
                AudioHardwareDestroyAggregateDevice(aggregate_id);
                AudioHardwareDestroyProcessTap(tap_id);
            }
            return Err(error);
        }

        if let Err(error) = check_status(
            unsafe { AudioDeviceStart(aggregate_id, io_proc_id) },
            "AudioDeviceStart",
        ) {
            unsafe {
                AudioDeviceDestroyIOProcID(aggregate_id, io_proc_id);
                AudioHardwareDestroyAggregateDevice(aggregate_id);
                AudioHardwareDestroyProcessTap(tap_id);
            }
            return Err(error);
        }

        Ok(Self {
            tap_id,
            aggregate_id,
            io_proc_id,
            callback_state,
        })
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _keep_state_alive = &self.callback_state;
        unsafe {
            AudioDeviceStop(self.aggregate_id, self.io_proc_id);
            AudioDeviceDestroyIOProcID(self.aggregate_id, self.io_proc_id);
            AudioHardwareDestroyAggregateDevice(self.aggregate_id);
            AudioHardwareDestroyProcessTap(self.tap_id);
        }
    }
}

fn create_tap_description() -> Retained<CATapDescription> {
    unsafe {
        let description = CATapDescription::new();
        let name = NSString::from_str("soundbar system audio");
        description.setName(&name);
        description.setPrivate(true);
        description.setMixdown(true);
        description.setMono(false);
        description.setExclusive(true);
        description.setMuteBehavior(CATapMuteBehavior::Unmuted);
        description
    }
}

fn create_aggregate_device(tap_description: &CATapDescription) -> Result<AudioObjectID> {
    let uid = format!(
        "dev.soundbar.aggregate.{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before UNIX_EPOCH")?
            .as_nanos()
    );

    let tap_uuid = unsafe { tap_description.UUID() };
    let tap_uid = CFString::from_str(&tap_uuid.UUIDString().to_string());
    let subtap = cf_dictionary(&[cf_key(kAudioSubTapUIDKey)], &[tap_uid.as_ref()]);
    let taps = CFArray::<CFDictionary<CFString, CFType>>::from_objects(&[&subtap]);

    let name = CFString::from_static_str("soundbar aggregate");
    let uid = CFString::from_str(&uid);
    let true_value = unsafe { kCFBooleanTrue.expect("kCFBooleanTrue is unavailable") };

    let aggregate = cf_dictionary(
        &[
            cf_key(kAudioAggregateDeviceNameKey),
            cf_key(kAudioAggregateDeviceUIDKey),
            cf_key(kAudioAggregateDeviceIsPrivateKey),
            cf_key(kAudioAggregateDeviceTapAutoStartKey),
            cf_key(kAudioAggregateDeviceTapListKey),
        ],
        &[
            name.as_ref(),
            uid.as_ref(),
            true_value.as_ref(),
            true_value.as_ref(),
            taps.as_ref(),
        ],
    );

    let mut aggregate_id = kAudioObjectUnknown;
    let out = NonNull::new(&mut aggregate_id as *mut AudioObjectID)
        .context("failed to build aggregate device output pointer")?;
    check_status(
        unsafe { AudioHardwareCreateAggregateDevice(aggregate.as_ref(), out) },
        "AudioHardwareCreateAggregateDevice",
    )?;

    Ok(aggregate_id)
}

fn cf_key(key: &'static std::ffi::CStr) -> CFRetained<CFString> {
    CFString::from_str(key.to_str().expect("Core Audio key is not UTF-8"))
}

fn cf_dictionary(
    keys: &[CFRetained<CFString>],
    values: &[&CFType],
) -> CFRetained<CFDictionary<CFString, CFType>> {
    let key_refs = keys.iter().map(|key| key.as_ref()).collect::<Vec<_>>();
    CFDictionary::from_slices(&key_refs, values)
}

unsafe extern "C-unwind" fn audio_io_proc(
    _device: AudioObjectID,
    _now: NonNull<AudioTimeStamp>,
    input_data: NonNull<AudioBufferList>,
    _input_time: NonNull<AudioTimeStamp>,
    _output_data: NonNull<AudioBufferList>,
    _output_time: NonNull<AudioTimeStamp>,
    client_data: *mut c_void,
) -> i32 {
    if client_data.is_null() {
        return 0;
    }

    let state = unsafe { &*(client_data as *const CallbackState) };
    if !state.running.load(Ordering::Relaxed) {
        return 0;
    }

    let list = unsafe { input_data.as_ref() };
    let buffers = audio_buffers(list);
    for buffer in buffers {
        if buffer.mData.is_null() || buffer.mDataByteSize == 0 {
            continue;
        }

        let sample_count = buffer.mDataByteSize as usize / std::mem::size_of::<f32>();
        let samples = unsafe { std::slice::from_raw_parts(buffer.mData as *const f32, sample_count) };
        state.producer.push_slice_lossy(samples);
    }

    0
}

fn audio_buffers(list: &AudioBufferList) -> &[AudioBuffer] {
    unsafe { std::slice::from_raw_parts(list.mBuffers.as_ptr(), list.mNumberBuffers as usize) }
}

fn check_status(status: i32, operation: &str) -> Result<()> {
    if status == 0 {
        Ok(())
    } else {
        bail!("{operation} failed with OSStatus {status} ({})", fourcc(status));
    }
}

fn fourcc(status: i32) -> String {
    let bytes = status.to_be_bytes();
    if bytes.iter().all(|byte| byte.is_ascii_graphic() || *byte == b' ') {
        String::from_utf8_lossy(&bytes).into_owned()
    } else {
        format!("0x{status:08x}")
    }
}
