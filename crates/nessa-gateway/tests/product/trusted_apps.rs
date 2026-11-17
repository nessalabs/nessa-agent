//! `apps.trusted` takes nothing and `apps.untrust` names a server; both are
//! admitted only for a caller that manages the gateway's credentials.
use super::*;

#[test]
fn a_trusted_read_takes_no_parameters() {
    assert!(asks_nothing(&serde_json::Value::Null));
    assert!(asks_nothing(&json!({})));
    for refused in [json!({"server": "x"}), json!([]), json!("x")] {
        assert!(!asks_nothing(&refused), "{refused}");
    }
}

#[test]
fn an_untrust_names_a_server() {
    let asked = untrust_request(json!({"requestId": "r", "server": "figma"})).unwrap();
    assert_eq!(asked.server, "figma");
    assert_eq!(asked.request_id, "r");
    for refused in [
        json!({"requestId": "r", "server": ""}),
        json!({"requestId": "r"}),
        json!({"server": "figma"}),
        json!(null),
    ] {
        assert!(untrust_request(refused.clone()).is_none(), "{refused}");
    }
}

#[test]
fn only_a_credential_manager_reads_or_revokes_trust() {
    use nessa_protocol::product::generated::action_for_method;
    for method in ["apps.trusted", "apps.untrust"] {
        assert_eq!(
            action_for_method(method),
            Some("credential.manage"),
            "{method}"
        );
    }
}
