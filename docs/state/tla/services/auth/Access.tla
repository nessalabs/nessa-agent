---------------- MODULE Access ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES phase, proof, binding, member, live, fresh, checked
vars == <<phase, proof, binding, member, live, fresh, checked>>
Init == phase = "Verifying" /\ proof = FALSE /\ binding = FALSE /\ member = FALSE /\ live = TRUE /\ fresh = FALSE /\ checked = FALSE
Verify == phase = "Verifying" /\ phase' = "Reading" /\ proof' = TRUE /\ UNCHANGED <<binding, member, live, fresh, checked>>
Snapshot == phase = "Reading" /\ binding' = TRUE /\ member' = TRUE /\ fresh' = TRUE /\ UNCHANGED <<phase, proof, live, checked>>
Revoke == phase = "Reading" /\ live' = FALSE /\ UNCHANGED <<phase, proof, binding, member, fresh, checked>>
Admit == phase = "Reading" /\ proof /\ binding /\ member /\ live /\ fresh /\ phase' = "Admitted" /\ checked' = TRUE /\ UNCHANGED <<proof, binding, member, live, fresh>>
Refuse == phase \in {"Verifying", "Reading"} /\ phase' = "Refused" /\ UNCHANGED <<proof, binding, member, live, fresh, checked>>
Next == Verify \/ Snapshot \/ Revoke \/ Admit \/ Refuse
Spec == Init /\ [][Next]_vars
TypeOK == phase \in {"Verifying", "Reading", "Admitted", "Refused"} /\ proof \in BOOLEAN /\ binding \in BOOLEAN /\ member \in BOOLEAN /\ live \in BOOLEAN /\ fresh \in BOOLEAN /\ checked \in BOOLEAN
AdmissionNeedsCoherentAccess == checked => proof /\ binding /\ member /\ live /\ fresh
=============================================================================
