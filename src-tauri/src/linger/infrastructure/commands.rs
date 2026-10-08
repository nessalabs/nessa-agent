//! Setup's linger commands. The JSON is what the screen is allowed to say.

use serde::Serialize;
use tauri::State;

use crate::composition::HostDependencies;
use crate::linger::{
    application::{AuditDelivery, LingerView},
    domain::LingerShown,
};

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ShownWire {
    #[cfg(any(test, not(target_os = "linux")))]
    NotApplicable,
    Offer,
    Enabled,
    Declined,
    Refused,
    Waiting,
    Unsupported,
    Unconfirmed,
}

#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AuditWire {
    NotRequired,
    Recorded,
    Failed,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LingerResponse {
    shown: ShownWire,
    audit: AuditWire,
}

fn response(view: LingerView) -> LingerResponse {
    LingerResponse {
        shown: match view.shown() {
            #[cfg(any(test, not(target_os = "linux")))]
            LingerShown::NotApplicable => ShownWire::NotApplicable,
            LingerShown::Offer => ShownWire::Offer,
            LingerShown::Enabled => ShownWire::Enabled,
            LingerShown::Declined => ShownWire::Declined,
            LingerShown::Refused => ShownWire::Refused,
            LingerShown::Waiting => ShownWire::Waiting,
            LingerShown::Unsupported => ShownWire::Unsupported,
            LingerShown::Unconfirmed => ShownWire::Unconfirmed,
        },
        audit: match view.audit() {
            AuditDelivery::NotRequired => AuditWire::NotRequired,
            AuditDelivery::Recorded => AuditWire::Recorded,
            AuditDelivery::Failed => AuditWire::Failed,
        },
    }
}

#[tauri::command]
pub(crate) fn linger_status(deps: State<'_, HostDependencies>) -> LingerResponse {
    response(deps.linger.status())
}

#[tauri::command]
pub(crate) fn linger_accept(deps: State<'_, HostDependencies>) -> LingerResponse {
    response(deps.linger.accept())
}

#[tauri::command]
pub(crate) fn linger_decline(deps: State<'_, HostDependencies>) -> LingerResponse {
    response(deps.linger.decline())
}

#[cfg(test)]
mod tests {
    use super::response;
    use crate::linger::{
        application::{AuditDelivery, LingerView},
        domain::LingerShown,
    };

    #[test]
    fn not_applicable_is_the_wire_the_shell_parses() {
        let json = serde_json::to_string(&response(LingerView::not_this_host())).unwrap();
        assert_eq!(json, r#"{"shown":"not-applicable","audit":"not-required"}"#);
    }

    #[test]
    fn every_shown_tag_is_named_by_the_shell_union() {
        let model = include_str!("../../../../src/onboarding/model/linger.ts");
        let window = include_str!("../../../../src/host/window.ts");
        for command in ["linger_status", "linger_accept", "linger_decline"] {
            assert!(
                window.contains(&format!("\"{command}\"")),
                "{command} is missing from the shell"
            );
        }
        let rows = [
            (LingerShown::NotApplicable, "not-applicable"),
            (LingerShown::Offer, "offer"),
            (LingerShown::Enabled, "enabled"),
            (LingerShown::Declined, "declined"),
            (LingerShown::Refused, "refused"),
            (LingerShown::Waiting, "waiting"),
            (LingerShown::Unsupported, "unsupported"),
            (LingerShown::Unconfirmed, "unconfirmed"),
        ];
        for (variant, tag) in rows {
            let json = serde_json::to_value(&response(LingerView::from_shown_for_test(
                variant,
                AuditDelivery::NotRequired,
            )))
            .unwrap();
            assert_eq!(json["shown"], tag);
            assert!(
                model.contains(&format!("\"{tag}\"")),
                "{tag} is missing from the shell union"
            );
        }
        for audit in ["not-required", "recorded", "failed"] {
            assert!(model.contains(&format!("\"{audit}\"")));
        }
    }
}
