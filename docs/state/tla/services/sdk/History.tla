---------------- MODULE History ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES phase, physical, committed, binding, attempts
vars == <<phase, physical, committed, binding, attempts>>
Init == phase = "Pending" /\ physical = 0 /\ committed = 0 /\ binding = TRUE /\ attempts = 0
Save == phase = "Pending" /\ phase' = "Saving" /\ attempts' = 1 /\ UNCHANGED <<physical, committed, binding>>
Append == phase = "Saving" /\ physical = 0 /\ physical' = 1 /\ UNCHANGED <<phase, committed, binding, attempts>>
LostAck == phase = "Saving" /\ phase' = "Uncertain" /\ UNCHANGED <<physical, committed, binding, attempts>>
Retry == phase = "Uncertain" /\ attempts < 2 /\ binding /\ phase' = "Saving" /\ attempts' = attempts + 1 /\ UNCHANGED <<physical, committed, binding>>
Commit == phase = "Saving" /\ physical = 1 /\ binding /\ phase' = "Acknowledged" /\ committed' = 1 /\ UNCHANGED <<physical, binding, attempts>>
Conflict == phase = "Uncertain" /\ phase' = "Refused" /\ binding' = FALSE /\ UNCHANGED <<physical, committed, attempts>>
Next == Save \/ Append \/ LostAck \/ Retry \/ Commit \/ Conflict
Spec == Init /\ [][Next]_vars
TypeOK == phase \in {"Pending", "Saving", "Uncertain", "Acknowledged", "Refused"} /\ physical \in 0..1 /\ committed \in 0..1 /\ binding \in BOOLEAN /\ attempts \in 0..2
NoInventedCommit == committed <= physical
AcknowledgedHasExactBinding == phase = "Acknowledged" => binding
NoDuplicatePhysicalAppend == physical <= 1
=============================================================================
