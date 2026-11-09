//! macOS permissions. The microphone uses AVFoundation and screen recording
//! uses CoreGraphics. System audio recording (the process tap) has no public
//! API, so it calls the private TCC framework, loaded at run time; without it
//! the answer is `Unknown`. TCC also tells a denied screen recording apart from
//! one never asked for, which CoreGraphics can't.

use std::ffi::{CStr, c_char, c_int, c_void};
use std::sync::OnceLock;
use std::sync::mpsc;
use std::time::Duration;

use block2::{DynBlock, RcBlock};
use objc2::runtime::Bool;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use objc2_core_foundation::{CFDictionary, CFString};

use crate::{Access, Permission};

const TCC_FRAMEWORK: &CStr = c"/System/Library/PrivateFrameworks/TCC.framework/Versions/A/TCC";
const RTLD_NOW: c_int = 2;
const SCREEN_SERVICE: &str = "kTCCServiceScreenCapture";
const AUDIO_SERVICE: &str = "kTCCServiceAudioCapture";
/// How long a request waits for the user to answer the system prompt.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(600);

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
}

unsafe extern "C" {
    fn dlopen(path: *const c_char, mode: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

type Preflight = unsafe extern "C" fn(service: &CFString, options: *const CFDictionary) -> c_int;
type Request = unsafe extern "C" fn(
    service: &CFString,
    options: *const CFDictionary,
    answered: &DynBlock<dyn Fn(u8)>,
);

struct Tcc {
    preflight: Preflight,
    request: Request,
}

fn tcc() -> Option<&'static Tcc> {
    static TCC: OnceLock<Option<Tcc>> = OnceLock::new();
    TCC.get_or_init(|| {
        // SAFETY: the symbols have these C signatures in every macOS release
        // that has them; a missing framework or symbol gives null.
        unsafe {
            let handle = dlopen(TCC_FRAMEWORK.as_ptr(), RTLD_NOW);
            if handle.is_null() {
                return None;
            }
            let preflight = dlsym(handle, c"TCCAccessPreflight".as_ptr());
            let request = dlsym(handle, c"TCCAccessRequest".as_ptr());
            if preflight.is_null() || request.is_null() {
                return None;
            }
            Some(Tcc {
                preflight: std::mem::transmute::<*mut c_void, Preflight>(preflight),
                request: std::mem::transmute::<*mut c_void, Request>(request),
            })
        }
    })
    .as_ref()
}

pub fn access(permission: Permission) -> Access {
    match permission {
        Permission::Microphone => microphone(),
        Permission::ScreenRecording => tcc_access(SCREEN_SERVICE).unwrap_or_else(|| {
            if unsafe { CGPreflightScreenCaptureAccess() } {
                Access::Allowed
            } else {
                Access::NotAsked
            }
        }),
        Permission::SystemAudio => tcc_access(AUDIO_SERVICE).unwrap_or(Access::Unknown),
    }
}

pub fn request(permission: Permission) -> Access {
    let current = access(permission);
    if current != Access::NotAsked {
        return current;
    }
    match permission {
        Permission::Microphone => {
            if let Some(audio) = unsafe { AVMediaTypeAudio } {
                let (answered, answer) = mpsc::channel();
                let block = RcBlock::new(move |_: Bool| {
                    let _ = answered.send(());
                });
                unsafe {
                    AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &block)
                };
                let _ = answer.recv_timeout(ANSWER_TIMEOUT);
            }
        }
        // Shows the system alert and returns at once; the switch is in System Settings.
        Permission::ScreenRecording => {
            unsafe { CGRequestScreenCaptureAccess() };
        }
        Permission::SystemAudio => {
            if let Some(tcc) = tcc() {
                let (answered, answer) = mpsc::channel();
                let block = RcBlock::new(move |_: u8| {
                    let _ = answered.send(());
                });
                let service = CFString::from_str(AUDIO_SERVICE);
                unsafe { (tcc.request)(&service, std::ptr::null(), &block) };
                let _ = answer.recv_timeout(ANSWER_TIMEOUT);
            }
        }
    }
    access(permission)
}

fn microphone() -> Access {
    let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
        return Access::Unknown;
    };
    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(audio) };
    if status == AVAuthorizationStatus::Authorized {
        Access::Allowed
    } else if status == AVAuthorizationStatus::NotDetermined {
        Access::NotAsked
    } else if status == AVAuthorizationStatus::Denied || status == AVAuthorizationStatus::Restricted
    {
        Access::Denied
    } else {
        Access::Unknown
    }
}

/// TCC answers 0 for allowed and 1 for denied; anything else means not asked yet.
fn tcc_access(service: &str) -> Option<Access> {
    let tcc = tcc()?;
    let service = CFString::from_str(service);
    let answer = unsafe { (tcc.preflight)(&service, std::ptr::null()) };
    Some(match answer {
        0 => Access::Allowed,
        1 => Access::Denied,
        _ => Access::NotAsked,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The answers belong to whatever runs the tests (Terminal, an IDE), so only
    /// their presence is checked: `Unknown` for system audio means the TCC
    /// framework didn't load.
    #[test]
    fn every_permission_gets_an_answer() {
        for permission in [
            Permission::Microphone,
            Permission::ScreenRecording,
            Permission::SystemAudio,
        ] {
            let answer = access(permission);
            eprintln!("{permission:?}: {answer:?}");
            assert_ne!(answer, Access::Unknown, "{permission:?}");
        }
    }
}
