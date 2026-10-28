//! Throwaway probe, not part of the deliverable — deleted before commit.
#![cfg(all(feature = "macos", target_os = "macos"))]

use std::io::Write;

use block2::RcBlock;
use objc2_foundation::{NSBundle, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotificationRequest,
    UNUserNotificationCenter,
};

fn log(msg: &str) {
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/probe_unc.log")
        .unwrap();
    writeln!(f, "{msg}").unwrap();
}

fn main() {
    let bundle = NSBundle::mainBundle();
    let ident = bundle.bundleIdentifier();
    log(&format!("bundleIdentifier = {:?}", ident));
    let center = UNUserNotificationCenter::currentNotificationCenter();
    log("got center ok");

    let opts = UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound;
    let completion = RcBlock::new(move |granted: objc2::runtime::Bool, err: *mut objc2_foundation::NSError| {
        log(&format!("auth granted={} err_is_null={}", granted.as_bool(), err.is_null()));

        let content = UNMutableNotificationContent::new();
        unsafe {
            content.setTitle(&NSString::from_str("Probe title"));
            content.setBody(&NSString::from_str("Probe body"));
        }
        let identifier = NSString::from_str("probe-tag-1");
        let request = unsafe {
            UNNotificationRequest::requestWithIdentifier_content_trigger(&identifier, &content, None)
        };
        let center2 = UNUserNotificationCenter::currentNotificationCenter();
        let add_completion = RcBlock::new(move |err: *mut objc2_foundation::NSError| {
            log(&format!("add request completion err_is_null={}", err.is_null()));
        });
        unsafe { center2.addNotificationRequest_withCompletionHandler(&request, Some(&add_completion)) };
    });
    unsafe { center.requestAuthorizationWithOptions_completionHandler(opts, &completion) };

    log("requested, sleeping");
    std::thread::sleep(std::time::Duration::from_secs(5));
    log("done");
}
