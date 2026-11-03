---- MODULE PaneAdmission ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES panes, focus, refused, before
vars == <<panes, focus, refused, before>>
Range(s) == {s[i] : i \in DOMAIN s}
Init == /\ panes = <<"A">> /\ focus = "A" /\ refused = FALSE /\ before = <<"A">>
OpenExisting == \E item \in {"A","B"}: /\ item \in Range(panes) /\ focus' = item /\ refused' = FALSE /\ before' = panes /\ UNCHANGED panes
Split == \E item \in {"A","B"}: /\ item \notin Range(panes) /\ Len(panes) < 2 /\ panes' = Append(panes,item) /\ focus' = item /\ refused' = FALSE /\ before' = panes
NoRoom == /\ before' = panes /\ refused' = TRUE /\ UNCHANGED <<panes,focus>>
Replace == \E item \in {"A","B"}: /\ item \notin Range(panes) /\ panes' = [i \in DOMAIN panes |-> IF panes[i] = focus THEN item ELSE panes[i]] /\ focus' = item /\ before' = panes /\ refused' = FALSE
Close == /\ Len(panes) = 2 /\ panes' = <<panes[1]>> /\ focus' = panes[1] /\ before' = panes /\ refused' = FALSE
Next == OpenExisting \/ Split \/ NoRoom \/ Replace \/ Close
UniqueItems == Len(panes) = Cardinality(Range(panes))
FocusExists == focus \in Range(panes)
RefusalUnchanged == refused => panes = before
Spec == Init /\ [][Next]_vars
====
