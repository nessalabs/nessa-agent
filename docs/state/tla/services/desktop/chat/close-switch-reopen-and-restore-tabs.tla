---- MODULE TabLifetimes ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES live, retired, counter, saved, selected
vars == <<live, retired, counter, saved, selected>>
Init == /\ live = {1} /\ retired = {} /\ counter = 1 /\ saved = FALSE /\ selected = 1
Save == /\ saved' = TRUE /\ UNCHANGED <<live,retired,counter,selected>>
Restore == /\ saved /\ counter < 3 /\ retired' = retired \cup live /\ counter' = counter + 1 /\ live' = {counter + 1} /\ selected' = counter + 1 /\ UNCHANGED saved
LateCallback == \E target \in retired: /\ target \notin live /\ UNCHANGED vars
Next == Save \/ Restore \/ LateCallback
FreshLocalLifetime == live \cap retired = {}
SelectionLive == selected \in live
AllocatorBound == counter \in 1..3
Spec == Init /\ [][Next]_vars
====
