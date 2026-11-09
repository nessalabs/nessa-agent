---- MODULE QuestionFlights ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES slot, joined, dispatched, settled
vars == <<slot, joined, dispatched, settled>>
Init == /\ slot = "none" /\ joined = {} /\ dispatched = 0 /\ settled = FALSE
Start == \E stamp \in {"A","B"}: /\ slot = "none" /\ dispatched < 2 /\ slot' = stamp /\ joined' = {stamp} /\ dispatched' = dispatched + 1 /\ settled' = FALSE
Join == \E stamp \in {"A","B"}: /\ slot # "none" /\ stamp = slot /\ joined' = joined \cup {stamp} /\ UNCHANGED <<slot,dispatched,settled>>
Different == \E stamp \in {"A","B"}: /\ slot # "none" /\ stamp # slot /\ UNCHANGED vars
Settle == /\ slot # "none" /\ slot' = "none" /\ joined' = {} /\ settled' = TRUE /\ UNCHANGED dispatched
Next == Start \/ Join \/ Different \/ Settle
NoConflictingJoin == slot # "none" => joined \subseteq {slot}
EmptyReleased == slot = "none" => joined = {}
Spec == Init /\ [][Next]_vars
====
