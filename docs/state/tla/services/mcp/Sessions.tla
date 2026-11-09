---------------- MODULE Sessions ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES grant, phase, listed, physicalReleased
vars == <<grant, phase, listed, physicalReleased>>
Init == grant = TRUE /\ phase = "Opening" /\ listed = FALSE /\ physicalReleased = FALSE
Register == phase = "Opening" /\ grant /\ phase' = "Open" /\ UNCHANGED <<grant, listed, physicalReleased>>
Revoke == grant /\ grant' = FALSE /\ phase' = (IF phase = "Opening" THEN "Opening" ELSE "Closed") /\ UNCHANGED <<listed, physicalReleased>>
List == phase = "Open" /\ grant /\ listed' = TRUE /\ UNCHANGED <<grant, phase, physicalReleased>>
RefreshFails == phase = "Open" /\ listed' = listed /\ UNCHANGED <<grant, phase, physicalReleased>>
Close == phase = "Open" /\ phase' = "Closed" /\ UNCHANGED <<grant, listed, physicalReleased>>
Release == phase = "Closed" /\ physicalReleased' = TRUE /\ UNCHANGED <<grant, phase, listed>>
RefuseRevoked == /\ phase = "Opening" /\ ~grant /\ phase' = "Closed" /\ UNCHANGED <<grant, listed, physicalReleased>>
Next == RefuseRevoked \/ Register \/ Revoke \/ List \/ RefreshFails \/ Close \/ Release
Spec == Init /\ [][Next]_vars
TypeOK == grant \in BOOLEAN /\ phase \in {"Opening", "Open", "Closed"} /\ listed \in BOOLEAN /\ physicalReleased \in BOOLEAN
NoOpenAfterRevoke == phase = "Open" => grant
PhysicalReleaseAfterLogicalClose == physicalReleased => phase = "Closed"
=============================================================================
