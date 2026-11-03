---------------- MODULE Reviews ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES phase, selected, selectionAudit, writes, valid
vars == <<phase, selected, selectionAudit, writes, valid>>
Init == phase = "Pending" /\ selected = FALSE /\ selectionAudit = FALSE /\ writes = 0 /\ valid = FALSE
Validate == phase = "Pending" /\ valid' = TRUE /\ UNCHANGED <<phase, selected, selectionAudit, writes>>
Select == phase = "Pending" /\ valid /\ phase' = "Selected" /\ selected' = TRUE /\ UNCHANGED <<selectionAudit, writes, valid>>
Withdraw == phase = "Pending" /\ phase' = "Cancelled" /\ UNCHANGED <<selected, selectionAudit, writes, valid>>
AuditAck == phase = "Selected" /\ phase' = "Writing" /\ selectionAudit' = TRUE /\ UNCHANGED <<selected, writes, valid>>
AuditFail == phase = "Selected" /\ phase' = "AuditFailed" /\ UNCHANGED <<selected, selectionAudit, writes, valid>>
Write == phase = "Writing" /\ selectionAudit /\ phase' = "Written" /\ writes' = 1 /\ UNCHANGED <<selected, selectionAudit, valid>>
WriteLost == phase = "Writing" /\ selectionAudit /\ phase' = "Unconfirmed" /\ writes' = 1 /\ UNCHANGED <<selected, selectionAudit, valid>>
Finish == phase \in {"Written", "Unconfirmed"} /\ phase' = "Done" /\ UNCHANGED <<selected, selectionAudit, writes, valid>>
Next == Validate \/ Select \/ Withdraw \/ AuditAck \/ AuditFail \/ Write \/ WriteLost \/ Finish
Spec == Init /\ [][Next]_vars
TypeOK == phase \in {"Pending", "Selected", "Writing", "Written", "Unconfirmed", "AuditFailed", "Done", "Cancelled"} /\ selected \in BOOLEAN /\ selectionAudit \in BOOLEAN /\ writes \in 0..1 /\ valid \in BOOLEAN
ExactReview == selected => valid
WriteNeedsSelection == writes = 1 => selected /\ selectionAudit
CancelledNotConsumed == phase = "Cancelled" => ~selected
=============================================================================
