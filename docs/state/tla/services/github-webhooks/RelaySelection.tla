---- MODULE RelaySelection ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, generation, captured, selected, events, audit
vars == <<phase, generation, captured, selected, events, audit>>
Init == /\ phase = "Preparing" /\ generation = 1 /\ captured = 1 /\ selected = TRUE /\ events = TRUE /\ audit = FALSE
ReplaceGeneration == /\ generation = 1 /\ generation' = 2 /\ UNCHANGED <<phase,captured,selected,events,audit>>
CommitRemoval == /\ phase = "Preparing" /\ generation = captured /\ phase' = "Committed" /\ selected' = FALSE /\ events' = FALSE /\ audit' = TRUE /\ UNCHANGED <<generation,captured>>
Fail == /\ phase = "Preparing" /\ phase' = "Refused" /\ UNCHANGED <<generation,captured,selected,events,audit>>
Next == ReplaceGeneration \/ CommitRemoval \/ Fail
Spec == Init /\ [][Next]_vars
DeletionEvidence == ~selected => ~events /\ audit
FailedLeavesPrior == phase = "Refused" => selected /\ events /\ ~audit
====
