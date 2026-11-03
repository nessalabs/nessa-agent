---- MODULE DeviceApprovalDisplay ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES displayed, fingerprint, captured, pending, switching, dropped, settled
vars == <<displayed, fingerprint, captured, pending, switching, dropped, settled>>
Init == /\ displayed \in {"key-A","key-B"} /\ fingerprint \in BOOLEAN /\ captured = "none" /\ pending = FALSE /\ switching = FALSE /\ dropped = FALSE /\ settled = FALSE
Approve == /\ fingerprint /\ ~pending /\ captured' = displayed /\ pending' = TRUE /\ UNCHANGED <<displayed,fingerprint,switching,dropped,settled>>
Complete == /\ pending /\ pending' = FALSE /\ UNCHANGED <<displayed,fingerprint,captured,switching,dropped,settled>>
Switch == /\ ~switching /\ ~settled /\ switching' = TRUE /\ dropped' = FALSE /\ UNCHANGED <<displayed,fingerprint,captured,pending,settled>>
Drop == /\ switching /\ dropped' = TRUE /\ UNCHANGED <<displayed,fingerprint,captured,pending,switching,settled>>
ReadNewGateway == /\ switching /\ dropped /\ switching' = FALSE /\ settled' = TRUE /\ UNCHANGED <<displayed,fingerprint,captured,pending,dropped>>
Next == Approve \/ Complete \/ Switch \/ Drop \/ ReadNewGateway
ExactKeyApproval == captured = "none" \/ (captured = displayed /\ fingerprint)
RestartObserved == settled => dropped
Spec == Init /\ [][Next]_vars
====
