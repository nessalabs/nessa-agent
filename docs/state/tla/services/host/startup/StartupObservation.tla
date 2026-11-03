---- MODULE StartupObservation ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, lifetime, captured, revision, delivered, value
vars == <<phase, lifetime, captured, revision, delivered, value>>
Init == /\ phase = "Subscribing" /\ lifetime = 0 /\ captured = 0 /\ revision = 0 /\ delivered = FALSE /\ value = "None"
Subscribe == /\ phase = "Subscribing" /\ lifetime = captured /\ phase' = "Listening" /\ UNCHANGED <<lifetime,captured,revision,delivered,value>>
Observe == /\ phase = "Listening" /\ lifetime = captured /\ \E r \in 0..2: /\ r >= revision /\ revision' = r /\ delivered' = TRUE /\ value' = "Host" /\ UNCHANGED <<phase,lifetime,captured>>
SnapshotFailure == /\ phase = "Listening" /\ ~delivered /\ value' = "Unavailable" /\ UNCHANGED <<phase,lifetime,captured,revision,delivered>>
Stop == /\ phase # "Stopped" /\ phase' = "Stopped" /\ lifetime' = 1 /\ UNCHANGED <<captured,revision,delivered,value>>
Next == Subscribe \/ Observe \/ SnapshotFailure \/ Stop
Spec == Init /\ [][Next]_vars
RevisionBound == revision <= 2
HostHasEvidence == value = "Host" => delivered
====
