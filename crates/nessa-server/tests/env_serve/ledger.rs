//! The host's lease audit on disk: what accounting answers from it.
use super::*;

fn granted(lease: &str) -> LedgerEntry {
    LedgerEntry::Granted {
        lease: lease.into(),
        agent: "claude".into(),
    }
}

fn ended(lease: &str, forced: bool) -> LedgerEntry {
    LedgerEntry::Ended {
        lease: lease.into(),
        cleanup: Cleanup::Confirmed { forced },
        lost: false,
    }
}

/// An end answers for the grant before it, never for one after it: a lease
/// granted again after its end is uncertain until it ends again, and a line
/// this build cannot read is skipped.
#[test]
fn accounting_answers_the_last_grant_s_end() {
    let directory = tempfile::tempdir().unwrap();
    let ledger = FileLedger::open(directory.path().join("leases.jsonl")).unwrap();
    assert_eq!(ledger.accounted("a").unwrap(), Cleanup::NotHeld);
    ledger.record(&granted("a")).unwrap();
    assert_eq!(ledger.accounted("a").unwrap(), Cleanup::Uncertain);
    ledger.record(&ended("a", true)).unwrap();
    assert_eq!(
        ledger.accounted("a").unwrap(),
        Cleanup::Confirmed { forced: true }
    );
    {
        let mut file = ledger.append.lock().unwrap();
        file.write_all(b"{not a line this build reads}\n").unwrap();
    }
    ledger.record(&granted("a")).unwrap();
    assert_eq!(ledger.accounted("a").unwrap(), Cleanup::Uncertain);
    ledger.record(&ended("a", false)).unwrap();
    assert_eq!(
        ledger.accounted("a").unwrap(),
        Cleanup::Confirmed { forced: false }
    );
    assert_eq!(ledger.accounted("b").unwrap(), Cleanup::NotHeld);
}
