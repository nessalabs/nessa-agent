---- MODULE AttachmentDestination ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, active, captured, scope, present, attachedTo
vars == <<phase, active, captured, scope, present, attachedTo>>
Init == /\ phase = "Waiting" /\ active = 1 /\ captured = 1 /\ scope = 1 /\ present = TRUE /\ attachedTo = 0
Switch == /\ active' = 3 - active /\ UNCHANGED <<phase,captured,scope,present,attachedTo>>
ReplaceStore == /\ scope = 1 /\ scope' = 2 /\ UNCHANGED <<phase,active,captured,present,attachedTo>>
Delete == /\ present /\ present' = FALSE /\ UNCHANGED <<phase,active,captured,scope,attachedTo>>
Complete == /\ phase = "Waiting" /\ phase' = "Finished" /\ attachedTo' = (IF scope = 1 /\ present THEN captured ELSE 0) /\ UNCHANGED <<active,captured,scope,present>>
Next == Switch \/ ReplaceStore \/ Delete \/ Complete
Spec == Init /\ [][Next]_vars
OriginalConversation == attachedTo \in {0,captured}
CapturedIdentity == captured = 1
====
