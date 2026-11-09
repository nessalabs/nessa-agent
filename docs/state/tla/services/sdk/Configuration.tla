---------------- MODULE Configuration ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES phase, selected, active, ack, reconciled, nextTurn
vars == <<phase, selected, active, ack, reconciled, nextTurn>>
Init == phase = "Pending" /\ selected = 0 /\ active = 0 /\ ack = FALSE /\ reconciled = FALSE /\ nextTurn = FALSE
AckNextTurn == phase = "Pending" /\ phase' = "Observed" /\ ack' = TRUE /\ nextTurn' = TRUE /\ UNCHANGED <<selected, active, reconciled>>
AckCurrent == phase = "Pending" /\ phase' = "Observed" /\ ack' = TRUE /\ active' = 1 /\ UNCHANGED <<selected, reconciled, nextTurn>>
Uncertain == phase = "Pending" /\ phase' = "Uncertain" /\ UNCHANGED <<selected, active, ack, reconciled, nextTurn>>
Commit == phase = "Observed" /\ ack /\ phase' = "Applied" /\ selected' = 1 /\ UNCHANGED <<active, ack, reconciled, nextTurn>>
Reconcile == phase = "Applied" /\ selected = 1 /\ reconciled' = TRUE /\ UNCHANGED <<phase, selected, active, ack, nextTurn>>
Send == phase = "Applied" /\ reconciled /\ phase' = "Submitted" /\ UNCHANGED <<selected, active, ack, reconciled, nextTurn>>
Hold == phase = "Uncertain" /\ phase' = "Held" /\ UNCHANGED <<selected, active, ack, reconciled, nextTurn>>
Next == AckNextTurn \/ AckCurrent \/ Uncertain \/ Commit \/ Reconcile \/ Send \/ Hold
Spec == Init /\ [][Next]_vars
TypeOK == phase \in {"Pending", "Observed", "Applied", "Uncertain", "Held", "Submitted"} /\ selected \in 0..1 /\ active \in 0..1 /\ ack \in BOOLEAN /\ reconciled \in BOOLEAN /\ nextTurn \in BOOLEAN
SelectionNeedsAck == selected = 1 => ack
NextTurnPreservesActive == nextTurn => active = 0
SendNeedsSourceView == phase = "Submitted" => reconciled /\ selected = 1
=============================================================================
