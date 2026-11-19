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

#[test]
fn a_channel_names_the_cloud_its_vms_are_made_on() {
    use crate::workspaces::domain::Cloud;
    // Each cloud's own name is a channel place on its VMs, kept apart for
    // the main agent and for threads.
    let execution = ChannelExecution::parse("agent37", "boat").unwrap();
    assert_eq!(*execution.coordinator(), Place::CloudVm);
    assert_eq!(*execution.threads(), Place::CloudVm);
    assert_eq!(execution.coordinator_cloud(), Some(Cloud::Agent37));
    assert_eq!(execution.threads_cloud(), Some(Cloud::Boat));
    assert_eq!(execution.coordinator_word(), "agent37");
    assert_eq!(execution.threads_word(), "boat");
    assert_ne!(execution, ChannelExecution::parse("boat", "boat").unwrap());
    // A place that is not a cloud VM names no cloud: a pool's overflow goes
    // to the cloud chosen for new thread VMs.
    let elsewhere = ChannelExecution::parse("local", "pool").unwrap();
    assert_eq!(elsewhere.coordinator_cloud(), None);
    assert_eq!(elsewhere.threads_cloud(), None);
    for cloud in Cloud::ALL {
        assert_eq!(Cloud::parse(cloud.as_str()), Some(cloud));
    }
    assert_eq!(Cloud::parse("boat:thread"), None);
}
