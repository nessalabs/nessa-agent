//! Every combination of what a creation asked for, its channel's place, the
//! default device and what kind of creation it is, against the one place it
//! runs; and the pool's overflow to a cloud VM.
use super::*;
use nessa_sdk::domain::agent_execution::leases::{PeerEnvironment, SshDestination};

/// An answer as compared: a refusal by its kind.
fn kind(answer: Result<Place, ConversationError>) -> Result<Place, String> {
    answer.map_err(|error| format!("{error:?}"))
}

fn host() -> Place {
    Place::Host(SshDestination::new("devbox").unwrap())
}

fn peer() -> Place {
    Place::Peer(PeerEnvironment::new([7; PeerEnvironment::KEY_BYTES]))
}

/// (name asked, channel, default device, creation, answer).
type Row = (
    Option<&'static str>,
    Option<Place>,
    Option<Place>,
    Creation,
    Result<Place, ConversationError>,
);

#[test]
fn one_table_decides_where_a_new_conversation_runs() {
    use Creation::{Fork, Person, Thread};
    let invalid = || Err(ConversationError::InvalidInput);
    let unknown = || Err(ConversationError::EnvironmentNotConfigured);
    let table: Vec<Row> = vec![
        // Nothing asked, no channel: a person's goes to the default device,
        // an agent's thread runs here whatever the default.
        (None, None, None, Person, Ok(Place::Here)),
        (None, None, Some(peer()), Person, Ok(peer())),
        (None, None, Some(host()), Person, Ok(host())),
        (None, None, None, Thread, Ok(Place::Here)),
        (None, None, Some(peer()), Thread, Ok(Place::Here)),
        // Nothing asked, in a channel: the channel's place, never the default.
        (
            None,
            Some(Place::CloudVm),
            Some(peer()),
            Person,
            Ok(Place::CloudVm),
        ),
        (None, Some(Place::Pool), None, Person, Ok(Place::Pool)),
        (
            None,
            Some(Place::Here),
            Some(peer()),
            Person,
            Ok(Place::Here),
        ),
        (
            None,
            Some(Place::CloudVm),
            Some(peer()),
            Thread,
            Ok(Place::CloudVm),
        ),
        (None, Some(Place::Pool), None, Thread, Ok(Place::Pool)),
        // A place asked outside a channel wins over the default device.
        (Some("devbox"), None, Some(peer()), Person, Ok(host())),
        (Some("here"), None, Some(peer()), Person, Ok(Place::Here)),
        (Some("peer:any"), None, None, Thread, Ok(Place::Pool)),
        (
            Some("boat:thread"),
            None,
            Some(host()),
            Thread,
            Ok(Place::CloudVm),
        ),
        // In a channel a person names no place: the channel says where. That
        // refusal comes before the name is read.
        (Some("devbox"), Some(Place::Here), None, Person, invalid()),
        (
            Some("here"),
            Some(Place::CloudVm),
            Some(peer()),
            Person,
            invalid(),
        ),
        (
            Some("not a place"),
            Some(Place::Here),
            None,
            Person,
            invalid(),
        ),
        // A name that is no place.
        (Some("not a place"), None, None, Person, unknown()),
        (
            Some("not a place"),
            Some(Place::Here),
            None,
            Thread,
            unknown(),
        ),
        // An agent's thread stays in its channel and runs where it named.
        (Some("devbox"), Some(Place::Here), None, Thread, Ok(host())),
        (
            Some("here"),
            Some(Place::Pool),
            None,
            Thread,
            Ok(Place::Here),
        ),
        // A fork runs where its source runs, whatever its channel now says
        // and whatever the default device; it names nothing of its own.
        (None, None, Some(peer()), Fork(Place::Here), Ok(Place::Here)),
        (
            None,
            Some(Place::Here),
            None,
            Fork(Place::CloudVm),
            Ok(Place::CloudVm),
        ),
        (
            None,
            Some(Place::CloudVm),
            Some(peer()),
            Fork(Place::Here),
            Ok(Place::Here),
        ),
        (None, Some(Place::Pool), None, Fork(peer()), Ok(peer())),
        (Some("here"), None, None, Fork(Place::Here), invalid()),
    ];
    for (asked, channel, default_device, creation, answer) in table {
        let ask = PlaceAsk {
            asked,
            channel,
            default_device,
            creation,
        };
        assert_eq!(kind(decide_place(ask.clone())), kind(answer), "{ask:?}");
    }
}

#[test]
fn the_pool_overflows_to_a_cloud_vm_only_when_it_cannot_take_the_work() {
    for (pool, cloud, answer) in [
        (PoolNow::Free, true, Ok(Place::Pool)),
        (PoolNow::Free, false, Ok(Place::Pool)),
        (PoolNow::Busy, true, Ok(Place::CloudVm)),
        (PoolNow::Busy, false, Ok(Place::Pool)),
        (PoolNow::Offline, true, Ok(Place::CloudVm)),
        (
            PoolNow::Offline,
            false,
            Err(ConversationError::EnvironmentNotConfigured),
        ),
    ] {
        assert_eq!(
            kind(pool_choice(pool, cloud)),
            kind(answer),
            "{pool:?} {cloud}"
        );
    }
}
