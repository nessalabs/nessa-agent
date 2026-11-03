---------------- MODULE Discovery ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES phase, complete, address, instance, process, managed
vars == <<phase, complete, address, instance, process, managed>>
Init == phase = "Reading" /\ complete = FALSE /\ address = FALSE /\ instance = FALSE /\ process = FALSE /\ managed = FALSE
Read == phase = "Reading" /\ phase' = "Probing" /\ complete' = TRUE /\ UNCHANGED <<address, instance, process, managed>>
Absent == phase = "Reading" /\ phase' = "Absent" /\ UNCHANGED <<complete, address, instance, process, managed>>
Probe == phase = "Probing" /\ address' = TRUE /\ instance' = TRUE /\ process' = TRUE /\ managed' = TRUE /\ UNCHANGED <<phase, complete>>
Verify == phase = "Probing" /\ complete /\ address /\ instance /\ process /\ managed /\ phase' = "Verified" /\ UNCHANGED <<complete, address, instance, process, managed>>
Refuse == phase \in {"Reading", "Probing"} /\ phase' = "Refused" /\ UNCHANGED <<complete, address, instance, process, managed>>
ProbeMismatch == phase = "Probing" /\ address' = TRUE /\ instance' = TRUE /\ process' = FALSE /\ managed' = TRUE /\ UNCHANGED <<phase, complete>>
Next == ProbeMismatch \/ Read \/ Absent \/ Probe \/ Verify \/ Refuse
Spec == Init /\ [][Next]_vars
TypeOK == phase \in {"Reading", "Probing", "Verified", "Refused", "Absent"} /\ complete \in BOOLEAN /\ address \in BOOLEAN /\ instance \in BOOLEAN /\ process \in BOOLEAN /\ managed \in BOOLEAN
VerifiedIdentity == phase = "Verified" => complete /\ address /\ instance /\ process /\ managed
=============================================================================
