---- MODULE DevicesMount ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES live, pending, hub, unreachable, published
vars == <<live, pending, hub, unreachable, published>>
Init == /\ live = TRUE /\ pending = FALSE /\ hub = "none" /\ unreachable = FALSE /\ published = FALSE
Poll == /\ live /\ ~pending /\ pending' = TRUE /\ published' = FALSE /\ UNCHANGED <<live,hub,unreachable>>
Success == /\ live /\ pending /\ pending' = FALSE /\ hub' = "read" /\ unreachable' = FALSE /\ published' = TRUE /\ UNCHANGED live
Failure == /\ live /\ pending /\ pending' = FALSE /\ unreachable' = TRUE /\ published' = TRUE /\ UNCHANGED <<live,hub>>
Unmount == /\ live /\ live' = FALSE /\ published' = FALSE /\ UNCHANGED <<pending,hub,unreachable>>
Next == Poll \/ Success \/ Failure \/ Unmount
NoRetiredPublication == published => live
HubEvidence == hub \in {"none","read"}
Spec == Init /\ [][Next]_vars
====
