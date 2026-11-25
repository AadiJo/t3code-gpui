//! macOS microphone consent (TCC). Capture asks for access the first time; a refusal is
//! reported as the fork reports a blocked `getUserMedia`, and the app's `Info.plist` carries
//! the usage description macOS shows in the prompt. Other platforms have no such gate.

/// Whether the system refuses microphone access. On macOS an undecided status asks first and
/// blocks until the person answers, so call it off the UI thread.
#[cfg(target_os = "macos")]
pub fn denied() -> bool {
    use block2::RcBlock;
    use objc2::msg_send;
    use objc2::runtime::{AnyClass, Bool};
    use objc2_foundation::NSString;

    #[link(name = "AVFoundation", kind = "framework")]
    unsafe extern "C" {}

    const NOT_DETERMINED: isize = 0;
    const AUTHORIZED: isize = 3;
    let Some(class) = AnyClass::get(c"AVCaptureDevice") else { return false };
    // `AVMediaTypeAudio`.
    let audio = NSString::from_str("soun");
    let status: isize = unsafe { msg_send![class, authorizationStatusForMediaType: &*audio] };
    match status {
        AUTHORIZED => false,
        NOT_DETERMINED => {
            let (sender, receiver) = std::sync::mpsc::channel();
            let block = RcBlock::new(move |granted: Bool| {
                let _ = sender.send(granted.as_bool());
            });
            let _: () = unsafe { msg_send![class, requestAccessForMediaType: &*audio, completionHandler: &*block] };
            !receiver.recv().unwrap_or(false)
        }
        _ => true,
    }
}

#[cfg(not(target_os = "macos"))]
pub fn denied() -> bool {
    false
}
