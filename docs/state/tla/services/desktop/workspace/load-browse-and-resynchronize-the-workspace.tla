---- MODULE WorkspaceRevision ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES held, removed, listed
vars == <<held, removed, listed>>
Init == /\ held = 0 /\ removed = 0 /\ listed = FALSE
Summary == \E revision \in 1..3: /\ revision > held /\ revision > removed /\ held' = revision /\ listed' = TRUE /\ UNCHANGED removed
Removal == \E revision \in 1..3: /\ revision >= held /\ removed' = (IF revision > removed THEN revision ELSE removed) /\ listed' = FALSE /\ UNCHANGED held
StaleRead == \E revision \in 1..3: /\ (revision <= held \/ revision <= removed) /\ UNCHANGED vars
Next == Summary \/ Removal \/ StaleRead
RemovalFence == listed => held > removed
RevisionBound == held \in 0..3 /\ removed \in 0..3
Spec == Init /\ [][Next]_vars
====
