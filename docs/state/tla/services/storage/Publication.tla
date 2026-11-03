---------------- MODULE Publication ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES phase, current, removedNewer, cleanupArmed, visible
vars == <<phase, current, removedNewer, cleanupArmed, visible>>
Init == phase = "Reserved" /\ current = 0 /\ removedNewer = FALSE /\ cleanupArmed = TRUE /\ visible = FALSE
Rename == phase = "Reserved" /\ phase' = "Published" /\ current' = 1 /\ visible' = TRUE /\ cleanupArmed' = FALSE /\ UNCHANGED <<removedNewer>>
Ack == phase = "Published" /\ current = 1 /\ phase' = "Acknowledged" /\ UNCHANGED <<current, removedNewer, cleanupArmed, visible>>
SyncFail == phase = "Published" /\ phase' = "Unacknowledged" /\ UNCHANGED <<current, removedNewer, cleanupArmed, visible>>
ReplaceNewer == current = 1 /\ current' = 2 /\ UNCHANGED <<phase, removedNewer, cleanupArmed, visible>>
Cleanup == cleanupArmed /\ current # 2 /\ phase' = "Discarded" /\ removedNewer' = (current = 2) /\ current' = 0 /\ cleanupArmed' = FALSE /\ UNCHANGED <<visible>>
StaleCleanup == current = 2 /\ removedNewer' = FALSE /\ UNCHANGED <<phase, current, cleanupArmed, visible>>
ReplaceOrigin == phase = "Reserved" /\ current' = 2 /\ UNCHANGED <<phase, removedNewer, cleanupArmed, visible>>
Next == ReplaceOrigin \/ Rename \/ Ack \/ SyncFail \/ ReplaceNewer \/ Cleanup \/ StaleCleanup
Spec == Init /\ [][Next]_vars
TypeOK == phase \in {"Reserved", "Published", "Acknowledged", "Unacknowledged", "Discarded"} /\ current \in 0..2 /\ removedNewer \in BOOLEAN /\ cleanupArmed \in BOOLEAN /\ visible \in BOOLEAN
RenameDisarmsReservation == visible => ~cleanupArmed
NoNewerRemoval == ~removedNewer
AcknowledgementDoesNotErase == phase = "Unacknowledged" => visible
=============================================================================
