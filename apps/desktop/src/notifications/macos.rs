//! macOS: `UNUserNotificationCenter` notifications grouped under one thread
//! identifier, and the Dock badge label. Notification Center needs the app to
//! run from a bundle with an identifier (an ad hoc signature is enough); run
//! unbundled (`cargo run`), only the Dock badge works. The first launch asks
//! "“T3 Code” Would Like to Send You Notifications".

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use block2::RcBlock;
use futures::channel::mpsc::UnboundedSender;
use objc2::rc::Retained;
use objc2::runtime::{Bool, ProtocolObject};
use objc2::{AllocAnyThread, DefinedClass, MainThreadMarker, define_class, msg_send};
use objc2_app_kit::NSApplication;
use objc2_foundation::{NSArray, NSBundle, NSError, NSObject, NSObjectProtocol, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationPresentationOptions,
    UNNotificationRequest, UNNotificationResponse, UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};

use super::{Activation, NativeBackend, NativeNotification};

/// What a click on each shown notification opens, by request identifier.
type Targets = Arc<Mutex<HashMap<String, Activation>>>;

struct DelegateState {
    activations: UnboundedSender<Activation>,
    targets: Targets,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and the delegate does not implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "T3CodeNotificationDelegate"]
    #[ivars = DelegateState]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        /// A click: open the notification's thread.
        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion: &block2::DynBlock<dyn Fn()>,
        ) {
            let id = response.notification().request().identifier().to_string();
            let target = self.ivars().targets.lock().ok().and_then(|targets| targets.get(&id).cloned());
            let activation = target.unwrap_or_else(|| Activation::Inbox(id.into()));
            let _ = self.ivars().activations.unbounded_send(activation);
            completion.call(());
        }

        /// The app only posts while unfocused, so a post that lands as it gains focus still shows.
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            completion.call((UNNotificationPresentationOptions::Banner | UNNotificationPresentationOptions::List,));
        }
    }
);

impl Delegate {
    fn new(state: DelegateState) -> Retained<Self> {
        let this = Self::alloc().set_ivars(state);
        // SAFETY: NSObject's init takes no arguments and returns the initialized object.
        unsafe { msg_send![super(this), init] }
    }
}

/// Notification Center and the Dock tile.
pub struct MacBackend {
    /// `None` outside an app bundle, where `UNUserNotificationCenter` raises.
    center: Option<Retained<UNUserNotificationCenter>>,
    /// The center holds its delegate weakly.
    _delegate: Option<Retained<Delegate>>,
    targets: Targets,
}

impl MacBackend {
    /// Connects to Notification Center and asks for permission (once per install).
    pub fn new(activations: UnboundedSender<Activation>) -> Option<Self> {
        let targets: Targets = Arc::default();
        if NSBundle::mainBundle().bundleIdentifier().is_none() {
            eprintln!("notifications: not running from an app bundle; only the Dock badge works");
            return Some(Self { center: None, _delegate: None, targets });
        }
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let delegate = Delegate::new(DelegateState { activations, targets: targets.clone() });
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        let options = UNAuthorizationOptions::Alert | UNAuthorizationOptions::Badge;
        center.requestAuthorizationWithOptions_completionHandler(
            options,
            &RcBlock::new(|granted: Bool, error: *mut NSError| {
                if !granted.as_bool() {
                    // SAFETY: the handler's error is null or a valid NSError for the call.
                    let reason = unsafe { error.as_ref() }.map(|error| error.localizedDescription().to_string());
                    eprintln!("notifications: not allowed ({})", reason.unwrap_or_default());
                }
            }),
        );
        Some(Self { center: Some(center), _delegate: Some(delegate), targets })
    }
}

impl NativeBackend for MacBackend {
    fn show(&self, notification: NativeNotification) {
        let Some(center) = &self.center else { return };
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&notification.title));
        content.setBody(&NSString::from_str(&notification.lines.join("\n")));
        // One group in Notification Center (`groupId: "t3-work"`); no sound (`silent`).
        content.setThreadIdentifier(&NSString::from_str("t3-work"));
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&notification.id),
            &content,
            None,
        );
        if let Ok(mut targets) = self.targets.lock() {
            targets.insert(notification.id, notification.activation);
        }
        center.addNotificationRequest_withCompletionHandler(
            &request,
            Some(&RcBlock::new(|error: *mut NSError| {
                // SAFETY: the handler's error is null or a valid NSError for the call.
                if let Some(error) = unsafe { error.as_ref() } {
                    eprintln!("notifications: could not show: {}", error.localizedDescription());
                }
            })),
        );
    }

    fn remove(&self, id: &str) {
        let Some(center) = &self.center else { return };
        if let Ok(mut targets) = self.targets.lock() {
            targets.remove(id);
        }
        let ids = NSArray::from_retained_slice(&[NSString::from_str(id)]);
        center.removeDeliveredNotificationsWithIdentifiers(&ids);
        center.removePendingNotificationRequestsWithIdentifiers(&ids);
    }

    fn set_badge(&self, count: usize) {
        let Some(main_thread) = MainThreadMarker::new() else { return };
        let label = (count > 0).then(|| NSString::from_str(&count.to_string()));
        NSApplication::sharedApplication(main_thread).dockTile().setBadgeLabel(label.as_deref());
    }
}
