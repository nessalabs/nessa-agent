---------------- MODULE Attachment ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES phase, generation, ready, created, audit
vars == <<phase, generation, ready, created, audit>>
Init == phase = "Authorized" /\ generation = 0 /\ ready = FALSE /\ created = FALSE /\ audit = "Pending"
Start == phase = "Authorized" /\ generation = 0 /\ phase' = "Starting" /\ UNCHANGED <<generation, ready, created, audit>>
OpenReturns == phase = "Starting" /\ phase' = "Published" /\ created' = TRUE /\ UNCHANGED <<generation, ready, audit>>
PublishAck == phase = "Published" /\ generation = 0 /\ phase' = "Attached" /\ ready' = TRUE /\ audit' = "Acknowledged" /\ UNCHANGED <<generation, created>>
PublishFail == phase = "Published" /\ phase' = "Failed" /\ audit' = "Failed" /\ UNCHANGED <<generation, ready, created>>
Stop == generation = 0 /\ generation' = 1 /\ phase' = "Stopped" /\ ready' = FALSE /\ UNCHANGED <<created, audit>>
LateSuccess == phase = "Stopped" /\ ~created /\ created' = TRUE /\ phase' = "Published" /\ UNCHANGED <<generation, ready, audit>>
Next == Start \/ OpenReturns \/ PublishAck \/ PublishFail \/ Stop \/ LateSuccess
Spec == Init /\ [][Next]_vars
TypeOK == phase \in {"Authorized", "Starting", "Published", "Attached", "Failed", "Stopped"} /\ generation \in 0..1 /\ ready \in BOOLEAN /\ created \in BOOLEAN /\ audit \in {"Pending", "Acknowledged", "Failed"}
ReadyNeedsCurrent == ready => generation = 0
ReadyNeedsEvidence == ready => audit = "Acknowledged"
StoppedNeverReady == phase = "Stopped" => ~ready
=============================================================================
