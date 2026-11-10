//! A place reads back from its name as the same place, every name reads one
//! way only, and a channel holds only the places it can run in.
use super::*;
use crate::conversation::domain::ChannelExecution;

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
        assert_eq!(place.asked_name().is_none(), place == Place::Here);
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
fn a_channel_holds_here_a_cloud_vm_or_the_pool_for_threads_only() {
    let host = Place::Host(SshDestination::new("devbox").unwrap());
    for (coordinator, threads, held) in [
        (Place::Here, Place::Here, true),
        (Place::CloudVm, Place::Pool, true),
        (Place::Pool, Place::Here, false),
        (host.clone(), Place::Here, false),
        (Place::Here, host, false),
        (Place::Here, Place::Peer(peer()), false),
    ] {
        let execution = ChannelExecution::new(coordinator.clone(), threads.clone());
        assert_eq!(execution.is_some(), held, "{coordinator:?} {threads:?}");
        if let Some(execution) = execution {
            assert_eq!(
                ChannelExecution::parse(execution.coordinator_word(), execution.threads_word()),
                Some(execution)
            );
        }
    }
    assert_eq!(
        ChannelExecution::parse("local", "local"),
        Some(ChannelExecution::default())
    );
    assert_eq!(ChannelExecution::parse("pool", "local"), None);
    assert_eq!(ChannelExecution::parse("local", "moon"), None);
}
