---- MODULE PreviewResource ----
EXTENDS Naturals, FiniteSets
VARIABLES draft, preview, retained, revoked
vars == <<draft, preview, retained, revoked>>
Init == /\ draft = TRUE /\ preview = FALSE /\ retained = TRUE /\ revoked = FALSE
Open == /\ retained /\ preview' = TRUE /\ UNCHANGED <<draft,retained,revoked>>
Remove == /\ draft /\ draft' = FALSE /\ UNCHANGED <<preview,retained,revoked>>
Close == /\ preview /\ preview' = FALSE /\ UNCHANGED <<draft,retained,revoked>>
Reconcile == /\ ~draft /\ ~preview /\ retained /\ retained' = FALSE /\ revoked' = TRUE /\ UNCHANGED <<draft,preview>>
Next == Open \/ Remove \/ Close \/ Reconcile
Spec == Init /\ [][Next]_vars
RetainWhileNeeded == draft \/ preview => retained
ReleasedRevokesURL == ~retained => revoked
====
