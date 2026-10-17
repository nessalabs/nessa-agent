//! macOS: the User Notifications framework.
//!
//! A notice is a `UNNotificationRequest` named by the review's identifier, so
//! the same review replaces itself and `removeDeliveredNotificationsWithIdentifiers`
//! takes exactly it back. Its thread identifier is the conversation, which
//! groups a conversation's notices in Notification Center and is how a click
//! finds where to go.
//!
//! The framework refuses a process that is not an app bundle with an
//! identifier by raising an Objective-C exception, which aborts. `tauri dev`
//! runs the bare binary, so outside a bundle this center is never touched and
//! the host answers `unsupported`.

use std::path::Path;
use std::ptr::NonNull;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_foundation::{NSArray, NSBundle, NSError, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification,
    UNNotificationDefaultActionIdentifier, UNNotificationPresentationOptions,
    UNNotificationRequest, UNNotificationResponse, UNNotificationSettings, UNNotificationSound,
    UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};

use crate::review_notices::application::{NoticeCenter, NoticeOpened};
use crate::review_notices::domain::{NoticeAccess, NoticeId, ReviewNotice};

/// How long a settings read or a post may take. The framework answers these
/// from a local daemon in milliseconds.
const ANSWER: Duration = Duration::from_secs(10);
/// How long the person may take over the system's allow prompt.
const PROMPT: Duration = Duration::from_secs(600);

struct DelegateIvars {
    opened: Arc<dyn NoticeOpened>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and this class does
    // not implement `Drop`.
    #[unsafe(super(NSObject))]
    #[name = "NessaReviewNoticeDelegate"]
    #[ivars = DelegateIvars]
    struct NoticeDelegate;

    unsafe impl NSObjectProtocol for NoticeDelegate {}

    unsafe impl UNUserNotificationCenterDelegate for NoticeDelegate {
        /// A review waiting while Nessa is frontmost still shows: the person
        /// may be in another conversation.
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            completion.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion: &block2::DynBlock<dyn Fn()>,
        ) {
            // SAFETY: a static NSString the framework defines.
            let default = unsafe { UNNotificationDefaultActionIdentifier };
            if response.actionIdentifier().isEqualToString(default) {
                let content = response.notification().request().content();
                let conversation = content.threadIdentifier().to_string();
                if !conversation.is_empty() {
                    self.ivars().opened.opened(&conversation);
                }
            }
            completion.call(());
        }
    }
);

// SAFETY: the only ivar is `Send + Sync`, it is never changed after `init`,
// and NSObject's retain and release are thread safe.
unsafe impl Send for NoticeDelegate {}
// SAFETY: as above.
unsafe impl Sync for NoticeDelegate {}

impl NoticeDelegate {
    fn new(opened: Arc<dyn NoticeOpened>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(DelegateIvars { opened });
        // SAFETY: NSObject's `init` on a freshly allocated instance.
        unsafe { msg_send![super(this), init] }
    }
}

/// The User Notifications center, for an app run from its bundle.
pub(crate) struct UserNotificationCenter {
    // The center holds its delegate weakly; this keeps it alive.
    _delegate: Retained<NoticeDelegate>,
}

impl UserNotificationCenter {
    /// The center, when this process is an app bundle's executable.
    pub(crate) fn for_bundle(opened: Arc<dyn NoticeOpened>) -> Option<Self> {
        let executable = std::env::current_exe().ok()?;
        // The framework's own condition is a bundle with an identifier.
        if !inside_app_bundle(&executable) || NSBundle::mainBundle().bundleIdentifier().is_none() {
            return None;
        }
        let delegate = NoticeDelegate::new(opened);
        center().setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        Some(Self {
            _delegate: delegate,
        })
    }
}

/// `…/Name.app/Contents/MacOS/name`.
pub(crate) fn inside_app_bundle(executable: &Path) -> bool {
    let mut up = executable.ancestors().skip(1);
    let (Some(macos), Some(contents), Some(bundle)) = (up.next(), up.next(), up.next()) else {
        return false;
    };
    macos.file_name().is_some_and(|name| name == "MacOS")
        && contents.file_name().is_some_and(|name| name == "Contents")
        && bundle
            .extension()
            .is_some_and(|extension| extension == "app")
}

fn center() -> Retained<UNUserNotificationCenter> {
    UNUserNotificationCenter::currentNotificationCenter()
}

fn access_from(status: UNAuthorizationStatus) -> NoticeAccess {
    match status {
        UNAuthorizationStatus::NotDetermined => NoticeAccess::NotAsked,
        UNAuthorizationStatus::Denied => NoticeAccess::Denied,
        // Authorized, Provisional and Ephemeral all deliver.
        _ => NoticeAccess::Allowed,
    }
}

fn error_text(error: *mut NSError) -> Option<String> {
    // SAFETY: the framework passes nil or a valid NSError for the call.
    unsafe { error.as_ref() }.map(|error| error.localizedDescription().to_string())
}

impl NoticeCenter for UserNotificationCenter {
    fn access(&self) -> NoticeAccess {
        let (send, receive) = mpsc::channel();
        let block = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
            // SAFETY: the framework passes a valid settings object.
            let status = unsafe { settings.as_ref() }.authorizationStatus();
            let _ = send.send(access_from(status));
        });
        center().getNotificationSettingsWithCompletionHandler(&block);
        receive.recv_timeout(ANSWER).unwrap_or_else(|_| {
            crate::host_log::warn("the notification settings did not answer");
            NoticeAccess::Unsupported
        })
    }

    fn ask(&self) -> NoticeAccess {
        let (send, receive) = mpsc::channel();
        let block = RcBlock::new(move |_granted: Bool, error: *mut NSError| {
            let _ = send.send(error_text(error));
        });
        center().requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &block,
        );
        match receive.recv_timeout(PROMPT) {
            Ok(Some(error)) => {
                crate::host_log::warn(format!("asking to allow notifications failed: {error}"))
            }
            Ok(None) => {}
            Err(_) => crate::host_log::warn("the allow-notifications prompt did not answer"),
        }
        // What the person chose, read back rather than taken from the prompt.
        self.access()
    }

    fn post(&self, notice: &ReviewNotice) -> Result<(), String> {
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&notice.title));
        content.setBody(&NSString::from_str(&notice.body));
        content.setThreadIdentifier(&NSString::from_str(notice.id.conversation()));
        content.setSound(Some(&UNNotificationSound::defaultSound()));
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(notice.id.identifier()),
            &content,
            None,
        );
        let (send, receive) = mpsc::channel();
        let block = RcBlock::new(move |error: *mut NSError| {
            let _ = send.send(error_text(error));
        });
        center().addNotificationRequest_withCompletionHandler(&request, Some(&block));
        match receive.recv_timeout(ANSWER) {
            Ok(None) => Ok(()),
            Ok(Some(error)) => Err(error),
            Err(_) => Err("the notification center did not answer".to_owned()),
        }
    }

    fn withdraw(&self, id: &NoticeId) -> Result<(), String> {
        let identifiers = NSArray::from_retained_slice(&[NSString::from_str(id.identifier())]);
        let center = center();
        center.removePendingNotificationRequestsWithIdentifiers(&identifiers);
        center.removeDeliveredNotificationsWithIdentifiers(&identifiers);
        Ok(())
    }

    fn withdraw_all(&self) {
        let center = center();
        center.removeAllPendingNotificationRequests();
        center.removeAllDeliveredNotifications();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_app_bundles_executable_is_inside_one() {
        assert!(inside_app_bundle(Path::new(
            "/Applications/Nessa.app/Contents/MacOS/nessa"
        )));
        assert!(!inside_app_bundle(Path::new(
            "/Users/me/nessa/target/debug/nessa-app"
        )));
        assert!(!inside_app_bundle(Path::new("/Contents/MacOS/nessa")));
        assert!(!inside_app_bundle(Path::new(
            "/Applications/Nessa/Contents/MacOS/nessa"
        )));
    }

    #[test]
    fn every_delivering_status_is_allowed() {
        assert_eq!(
            access_from(UNAuthorizationStatus::NotDetermined),
            NoticeAccess::NotAsked
        );
        assert_eq!(
            access_from(UNAuthorizationStatus::Denied),
            NoticeAccess::Denied
        );
        for status in [
            UNAuthorizationStatus::Authorized,
            UNAuthorizationStatus::Provisional,
            UNAuthorizationStatus::Ephemeral,
        ] {
            assert_eq!(access_from(status), NoticeAccess::Allowed);
        }
    }
}
