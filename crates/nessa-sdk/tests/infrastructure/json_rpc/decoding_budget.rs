//! Exact JSON item limits cover retained values, ignored metadata, and object keys.
use super::*;
use crate::infrastructure::json_rpc::Reader;

fn array_frame(field: &str, items: usize, item: &str) -> Vec<u8> {
    let mut frame = if field == "result" {
        String::from(r#"{"jsonrpc":"2.0","id":1,"result":["#)
    } else {
        String::from(r#"{"jsonrpc":"2.0","id":1,"result":null,"metadata":["#)
    };
    for index in 0..items {
        if index > 0 {
            frame.push(',');
        }
        frame.push_str(item);
    }
    frame.push_str("]}");
    frame.into_bytes()
}

#[test]
fn values_in_retained_and_ignored_arrays_share_one_exact_budget() {
    for (field, envelope_items) in [("result", 7), ("metadata", 9)] {
        for item in ["0", "[]"] {
            let available = MAX_JSON_ITEMS - envelope_items;
            let accepted = array_frame(field, available, item);
            assert!(parse(&accepted).is_ok(), "{field}: {item}");
            let rejected = array_frame(field, available + 1, item);
            assert!(matches!(parse(&rejected), Err(AgentError::Protocol(_))));
        }
    }
}

#[test]
fn object_keys_are_charged_before_their_values() {
    // Envelope + result array + its object consume eight items; each member
    // consumes two more, even when its value is an empty collection.
    let entries = (MAX_JSON_ITEMS - 8) / 2;
    let object = (0..entries)
        .map(|index| format!("\"{index}\":[]"))
        .collect::<Vec<_>>()
        .join(",");
    let accepted = format!(r#"{{"jsonrpc":"2.0","id":1,"result":[{{{object}}}]}}"#);
    assert!(parse(accepted.as_bytes()).is_ok());
    let rejected = format!(r#"{{"jsonrpc":"2.0","id":1,"result":[{{{object},"extra":null}}]}}"#);
    assert!(matches!(
        parse(rejected.as_bytes()),
        Err(AgentError::Protocol(_))
    ));
}

#[tokio::test]
async fn small_wire_collections_cannot_expand_into_millions_of_values() {
    let mut frame = array_frame("metadata", 2_000_000, "[]");
    frame.push(b'\n');
    assert!(frame.len() < 16 * 1024 * 1024);
    assert!(matches!(
        Reader::new(frame.as_slice(), 16 * 1024 * 1024).next().await,
        Err(AgentError::Protocol(_))
    ));
}

#[test]
fn valid_unknown_metadata_is_ignored_but_duplicate_keys_are_still_rejected() {
    let frame = br#"{"jsonrpc":"2.0","id":1,"result":null,"metadata":{"trace":[{"name":"first"},{"name":"second"}],"flags":[true,false],"optional":null}}"#;
    let message = parse(frame).unwrap();
    assert_eq!(message.id, Some(RpcId::Number(1)));
    assert_eq!(message.result, Some(Value::Null));
    for metadata in [r#"{"same":1,"same":1}"#, r#"[{"same":1,"s\u0061me":2}]"#] {
        let frame = format!(r#"{{"jsonrpc":"2.0","id":1,"result":null,"metadata":{metadata}}}"#);
        assert!(matches!(
            parse(frame.as_bytes()),
            Err(AgentError::Protocol(_))
        ));
    }
    assert!(parse(br#"{"jsonrpc":"2.0","id":1,"result":null} null"#).is_err());
}
