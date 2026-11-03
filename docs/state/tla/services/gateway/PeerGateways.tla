---------------- MODULE PeerGateways ----------------
EXTENDS Naturals
VARIABLES phase, setHeld, poller, peerTurn, otherPeer, intent, changed, outcome, ack
vars == <<phase, setHeld, poller, peerTurn, otherPeer, intent, changed, outcome, ack>>
Init == phase = "Idle" /\ setHeld = FALSE /\ poller = TRUE /\ peerTurn = FALSE /\ otherPeer = TRUE /\ intent = FALSE /\ changed = FALSE /\ outcome = FALSE /\ ack = FALSE

Intent == /\ (phase = "Idle")
    /\ phase' = "Requested"
    /\ intent' = TRUE
    /\ UNCHANGED <<setHeld, poller, peerTurn, otherPeer, changed, outcome, ack>>

IntentFails == /\ (phase = "Idle")
    /\ phase' = "Refused"
    /\ UNCHANGED <<setHeld, poller, peerTurn, otherPeer, intent, changed, outcome, ack>>

Acquire == /\ (phase = "Requested" /\ intent)
    /\ phase' = "SetHeld"
    /\ setHeld' = TRUE
    /\ UNCHANGED <<poller, peerTurn, otherPeer, intent, changed, outcome, ack>>

Busy == /\ (phase = "Requested" /\ intent)
    /\ phase' = "Refused"
    /\ outcome' = TRUE
    /\ UNCHANGED <<setHeld, poller, peerTurn, otherPeer, intent, changed, ack>>

Preempt == /\ (phase = "SetHeld" /\ setHeld /\ intent)
    /\ phase' = "Preempting"
    /\ poller' = FALSE
    /\ UNCHANGED <<setHeld, peerTurn, otherPeer, intent, changed, outcome, ack>>

Own == /\ (phase = "Preempting" /\ ~poller /\ setHeld)
    /\ phase' = "Owned"
    /\ peerTurn' = TRUE
    /\ UNCHANGED <<setHeld, poller, otherPeer, intent, changed, outcome, ack>>

Change == /\ (phase = "Owned" /\ peerTurn /\ setHeld /\ intent)
    /\ phase' = "Changed"
    /\ changed' = TRUE
    /\ UNCHANGED <<setHeld, poller, peerTurn, otherPeer, intent, outcome, ack>>

Complete == /\ (phase = "Changed")
    /\ phase' = "Done"
    /\ outcome' = TRUE
    /\ ack' = TRUE
    /\ setHeld' = FALSE
    /\ peerTurn' = FALSE
    /\ UNCHANGED <<poller, otherPeer, intent, changed>>

AuditFails == /\ (phase = "Changed")
    /\ phase' = "Unconfirmed"
    /\ setHeld' = FALSE
    /\ peerTurn' = FALSE
    /\ UNCHANGED <<poller, otherPeer, intent, changed, outcome, ack>>

BusyAuditFails == /\ (phase = "Requested" /\ intent)
    /\ phase' = "Unconfirmed"
    /\ UNCHANGED <<setHeld, poller, peerTurn, otherPeer, intent, changed, outcome, ack>>

Next == Intent \/ IntentFails \/ Acquire \/ Busy \/ Preempt \/ Own \/ Change \/ Complete \/ AuditFails \/ BusyAuditFails
Spec == Init /\ [][Next]_vars
TypeOK == phase \in {"Idle", "Requested", "SetHeld", "Preempting", "Owned", "Changed", "Done", "Unconfirmed", "Refused"} /\ setHeld \in BOOLEAN /\ poller \in BOOLEAN /\ peerTurn \in BOOLEAN /\ otherPeer \in BOOLEAN /\ intent \in BOOLEAN /\ changed \in BOOLEAN /\ outcome \in BOOLEAN /\ ack \in BOOLEAN
MutationHasIntent == changed => intent
SuccessHasBothRecords == ack => (intent /\ changed /\ outcome)
OtherPeerUninterrupted == otherPeer
CommandCannotRaceSelectedRead == peerTurn => (~poller /\ setHeld)
PreemptionNeedsIntent == ~poller => intent
============================================================================
