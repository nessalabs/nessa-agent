---- MODULE AttachmentHold ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, claim, audited, alive, confirmed
vars == <<phase, claim, audited, alive, confirmed>>
Init == /\ phase = "Pending" /\ claim = 1 /\ audited = FALSE /\ alive = TRUE /\ confirmed = FALSE
Audit == /\ phase = "Pending" /\ phase' \in {"Audited","Reverted"} /\ audited' = (phase' = "Audited") /\ UNCHANGED <<claim,alive,confirmed>>
Delete == /\ alive /\ alive' = FALSE /\ confirmed' = FALSE /\ phase' = "Released" /\ claim' = 0 /\ UNCHANGED audited
Confirm == /\ phase = "Audited" /\ alive /\ claim = 1 /\ confirmed' = TRUE /\ phase' = "Kept" /\ UNCHANGED <<claim,audited,alive>>
ReplaceClaim == /\ phase = "Kept" /\ claim' = 2 /\ phase' = "Replacement" /\ UNCHANGED <<audited,alive,confirmed>>
UndoOwnClaim == /\ claim = 1 /\ phase \in {"Pending","Audited","Kept"} /\ phase' = "Reverted" /\ confirmed' = FALSE /\ claim' = 0 /\ UNCHANGED <<audited,alive>>
Next == Audit \/ Delete \/ Confirm \/ ReplaceClaim \/ UndoOwnClaim
Spec == Init /\ [][Next]_vars
AuditBeforeConfirm == confirmed => audited
ConversationOwnsHold == confirmed => alive /\ claim # 0
ReplacementSurvivesOldUndo == phase = "Replacement" => claim = 2 /\ confirmed
====
