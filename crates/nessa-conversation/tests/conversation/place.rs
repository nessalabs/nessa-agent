//! A place reads back from its name as the same place, every name reads one
//! way only, and a channel holds any place in its own words.
use super::*;
use crate::domain::{ChannelExecution, UnknownChannelPlace};

fn peer() -> PeerEnvironment {
    PeerEnvironment::new([0xab; PeerEnvironment::KEY_BYTES])
}

#[test]
fn every_place_reads_back_from_its_name() {
    for place in [
        Place::Here,
        Place::Host(SshDestination::new("me@devbox").unwrap()),
        Place::Peer(peer()),
        Place::Pool,
        Place::CloudVm,
    ] {
        assert_eq!(Place::named(&place.name()), Ok(place.clone()), "{place:?}");
        assert_eq!(place.remote_name().is_none(), place == Place::Here);
    }
}

#[test]
fn the_names_kept_in_files_and_on_the_wire_are_unchanged() {
    assert_eq!(Place::named("here"), Ok(Place::Here));
    assert_eq!(Place::named("peer:any"), Ok(Place::Pool));
    assert_eq!(Place::named("boat:thread"), Ok(Place::CloudVm));
    assert_eq!(Place::named(&peer().name()), Ok(Place::Peer(peer())));
    assert_eq!(
        Place::named("devbox"),
        Ok(Place::Host(SshDestination::new("devbox").unwrap()))
    );
    for unknown in ["", "peer:", "peer:zz", "-oProxyCommand=x", "a b"] {
        assert_eq!(Place::named(unknown), Err(UnknownPlace), "{unknown:?}");
    }
}

#[test]
fn a_channel_holds_any_place_and_spells_each_one_way() {
    let host = Place::Host(SshDestination::new("devbox").unwrap());
    for (place, word) in [
        (Place::Here, "local".to_owned()),
        (Place::CloudVm, "boat".to_owned()),
        (Place::Pool, "pool".to_owned()),
        (host.clone(), "devbox".to_owned()),
        (Place::Peer(peer()), peer().name()),
    ] {
        let execution = ChannelExecution::new(place.clone(), Place::Pool);
        assert_eq!(execution.coordinator_word(), word);
        assert_eq!(*execution.coordinator(), place);
        assert_eq!(
            ChannelExecution::parse(&word, "pool"),
            Ok(execution),
            "{word}"
        );
    }
    assert_eq!(
        ChannelExecution::parse("local", "local"),
        Ok(ChannelExecution::default())
    );
    // A kind has one spelling: its place name names nothing here. Any
    // other word an SSH destination could be is a computer, which the
    // gateway checks it serves when a person asks for it.
    for unknown in ["here", "peer:any", "boat:thread", "", "-oProxyCommand=x"] {
        assert_eq!(
            ChannelExecution::parse("local", unknown),
            Err(UnknownChannelPlace),
            "{unknown:?}"
        );
    }
}
