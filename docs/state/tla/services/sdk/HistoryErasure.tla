---------------- MODULE HistoryErasure ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES phase, incarnation, request, resetAck, cleanup, access
vars == <<phase, incarnation, request, resetAck, cleanup, access>>
Init == phase = "Loaded" /\ incarnation = 0 /\ request = FALSE /\ resetAck = FALSE /\ cleanup = FALSE /\ access = FALSE
Erase == phase = "Loaded" /\ phase' = "ResetPending" /\ request' = TRUE /\ UNCHANGED <<incarnation, resetAck, cleanup, access>>
PhysicalReset == phase = "ResetPending" /\ request /\ phase' = "ResetObserved" /\ incarnation' = 1 /\ UNCHANGED <<request, resetAck, cleanup, access>>
LostReply == phase = "ResetObserved" /\ phase' = "ResetPending" /\ UNCHANGED <<incarnation, request, resetAck, cleanup, access>>
Reconcile == phase \in {"ResetPending", "ResetObserved"} /\ incarnation = 1 /\ phase' = "CleanupPending" /\ resetAck' = TRUE /\ UNCHANGED <<incarnation, request, cleanup, access>>
Cleanup == phase = "CleanupPending" /\ resetAck /\ phase' = "Ready" /\ cleanup' = TRUE /\ UNCHANGED <<incarnation, request, resetAck, access>>
LoadOrSave == phase = "Ready" /\ resetAck /\ cleanup /\ access' = TRUE /\ UNCHANGED <<phase, incarnation, request, resetAck, cleanup>>
Next == Erase \/ PhysicalReset \/ LostReply \/ Reconcile \/ Cleanup \/ LoadOrSave
Spec == Init /\ [][Next]_vars
TypeOK == phase \in {"Loaded", "ResetPending", "ResetObserved", "CleanupPending", "Ready"} /\ incarnation \in 0..1 /\ request \in BOOLEAN /\ resetAck \in BOOLEAN /\ cleanup \in BOOLEAN /\ access \in BOOLEAN
AccessAfterErasure == access => incarnation = 1 /\ resetAck /\ cleanup
AcknowledgedNewIncarnation == resetAck => incarnation = 1
OneResetIdentity == phase # "Loaded" => request
=============================================================================
