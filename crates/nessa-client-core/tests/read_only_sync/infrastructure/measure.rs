//! The harness keeps one payload across paced and unpaced copies, and reopens it.
use super::{profiles_match, report};

#[test]
fn paced_and_unpaced_keep_the_same_bytes_and_reopen() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("measurement runtime");
    // Awaited on this runtime. A `block_on` inside the report would panic here.
    let report = runtime.block_on(report());
    assert_eq!(report["profilesMatch"], true);
    let unpaced = &report["profiles"][0];
    let paced = &report["profiles"][1];
    assert!(unpaced["payloadBytes"].as_u64().unwrap() > 0);
    assert_eq!(unpaced["payloadBytes"], paced["payloadBytes"]);
    assert_eq!(unpaced["downloaded"], unpaced["applied"]);
    assert_eq!(paced["downloaded"], paced["applied"]);
    assert_eq!(unpaced["downloaded"], paced["downloaded"]);
    assert_eq!(unpaced["reopenDownloaded"], unpaced["downloaded"]);
    assert_eq!(unpaced["reopenApplied"], unpaced["applied"]);
    assert_eq!(paced["reopenDownloaded"], paced["downloaded"]);
    assert_eq!(paced["reopenApplied"], paced["applied"]);
    assert_eq!(unpaced["virtualLagMs"], 0);
    assert!(paced["virtualLagMs"].as_u64().unwrap() >= 1_000);

    let table = report["sizeTable"].as_array().unwrap();
    assert_eq!(table.len(), 18);
    let mut waits: Vec<u64> = table
        .iter()
        .map(|row| row["waitMs"].as_u64().unwrap())
        .collect();
    waits.sort_unstable();
    let rank = |percent: u64| waits[((percent * waits.len() as u64).div_ceil(100) - 1) as usize];
    assert_eq!(report["orderStatistics"]["p50Ms"], rank(50));
    assert_eq!(report["orderStatistics"]["p95Ms"], rank(95));
    assert_eq!(
        report["orderStatistics"]["of"],
        "the 18 pace waits, not a sampled link"
    );

    let experiments = report["experiments"].as_array().unwrap();
    let row = |name: &str| {
        experiments
            .iter()
            .find(|row| row["name"] == name)
            .unwrap_or_else(|| panic!("missing {name}"))
    };
    assert_eq!(row("sleep")["run"]["decision"], "sleep");
    assert_eq!(row("sleep")["run"]["quantumRan"], false);
    assert_eq!(row("sleep")["run"]["keptBytes"], 0);
    assert_eq!(row("unreachable")["run"]["decision"], "unreachable");
    assert_eq!(row("unreachable")["run"]["keptBytes"], 0);
    assert_eq!(
        row("meteredUnknownQuote")["run"]["decision"],
        "unknownQuote"
    );
    assert_eq!(row("meteredUnknownQuote")["run"]["quantumRan"], false);
    assert_eq!(row("foreground")["run"]["winner"], "openTranscript");
    assert_eq!(row("background")["run"]["winner"], "openTranscript");
    assert_eq!(row("wake")["run"]["decision"], "admit");
    assert_eq!(row("wake")["run"]["winner"], "openTranscript");
    assert_eq!(row("catalogue")["run"]["winner"], "catalogue");
    assert_eq!(row("subscriptionDown")["run"]["winner"], "olderHistory");
    assert_eq!(row("relay")["route"], "relay");
    assert_eq!(row("relay")["run"]["winner"], "openTranscript");
    assert_eq!(row("allowanceRaised")["run"]["before"], "allowance");
    assert_eq!(row("allowanceRaised")["run"]["after"], "admit");
    assert_eq!(row("reconnectAfterLoss")["run"]["next"], "reconnect");
    assert_eq!(
        row("reconnectAfterLoss")["run"]["inFlightBeforeCancel"],
        true
    );
    assert_eq!(row("reconnectAfterLoss")["run"]["afterBackoff"], "admit");
    assert_eq!(row("reconnectAfterLoss")["run"]["keptBytes"], 0);
    let recorded: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../docs/design/weak-link-measurements.json"
    ))
    .expect("recorded measurements");
    assert_eq!(without_wall(&report), without_wall(&recorded));
    assert!(report["relayPreambleBytes"].as_u64().unwrap() > 0);
    assert!(report["notMeasured"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item == "phone radio"));
}

#[test]
fn a_stale_reopen_on_either_profile_does_not_match() {
    let fresh = serde_json::json!({
        "payloadBytes": 10,
        "downloaded": 2,
        "applied": 2,
        "reopenDownloaded": 2,
        "reopenApplied": 2,
    });
    assert!(profiles_match(&fresh, &fresh));
    for (side, field) in [
        ("unpaced", "reopenDownloaded"),
        ("unpaced", "reopenApplied"),
        ("paced", "reopenDownloaded"),
        ("paced", "reopenApplied"),
    ] {
        let mut unpaced = fresh.clone();
        let mut paced = fresh.clone();
        let profile = if side == "unpaced" {
            &mut unpaced
        } else {
            &mut paced
        };
        profile[field] = serde_json::json!(0);
        assert!(
            !profiles_match(&unpaced, &paced),
            "{side}.{field} reopened stale and still matched"
        );
    }
}

fn without_wall(value: &serde_json::Value) -> serde_json::Value {
    let mut copy = value.clone();
    let Some(profiles) = copy
        .get_mut("profiles")
        .and_then(|item| item.as_array_mut())
    else {
        return copy;
    };
    for profile in profiles {
        if let Some(object) = profile.as_object_mut() {
            object.remove("wallMs");
        }
    }
    copy
}
