---- MODULE StoredPreference ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES stored, local, bound, chosen, last, before
vars == <<stored, local, bound, chosen, last, before>>
Init == /\ stored = "old" /\ local = "old" /\ bound = TRUE /\ chosen = "old" /\ last = "init" /\ before = "old"
Choose == \E value \in {"old","new"}, writes \in BOOLEAN: /\ bound /\ chosen' = value /\ stored' = (IF writes THEN value ELSE stored) /\ local' = value /\ before' = local /\ last' = "choose" /\ UNCHANGED bound
ExternalWrite == \E value \in {"old","new"}: /\ stored' = value /\ last' = "external" /\ UNCHANGED <<local,bound,chosen,before>>
MatchingEvent == /\ bound /\ local' = stored /\ last' = "matching" /\ before' = local /\ UNCHANGED <<stored,bound,chosen>>
OtherKey == /\ bound /\ before' = local /\ last' = "other" /\ UNCHANGED <<stored,local,bound,chosen>>
Retire == /\ bound /\ bound' = FALSE /\ before' = local /\ last' = "retire" /\ UNCHANGED <<stored,local,chosen>>
Next == Choose \/ ExternalWrite \/ MatchingEvent \/ OtherKey \/ Retire
LocalWriteFailureStillApplies == last = "choose" => local = chosen
OtherKeyIgnored == last = "other" => local = before
Spec == Init /\ [][Next]_vars
====
