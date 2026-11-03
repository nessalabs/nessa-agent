---- MODULE RelayPage ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, verified, allowed, audited, cursor, rows
vars == <<phase, verified, allowed, audited, cursor, rows>>
Init == /\ phase = "Verify" /\ verified = FALSE /\ allowed = FALSE /\ audited = FALSE /\ cursor = 0 /\ rows = {}
Verify == /\ phase = "Verify" /\ phase' \in {"Policy","Refused"} /\ verified' = (phase' = "Policy") /\ UNCHANGED <<allowed,audited,cursor,rows>>
Policy == /\ phase = "Policy" /\ phase' = "Audit" /\ allowed' \in BOOLEAN /\ UNCHANGED <<verified,audited,cursor,rows>>
Audit == /\ phase = "Audit" /\ phase' \in {"Read","Refused"} /\ audited' = (phase' = "Read") /\ UNCHANGED <<verified,allowed,cursor,rows>>
Page == /\ phase = "Read" /\ audited /\ allowed /\ phase' \in {"Page","Gap"} /\ UNCHANGED <<verified,allowed,audited,cursor,rows>>
Commit == /\ phase = "Page" /\ phase' = "Committed" /\ cursor' = 1 /\ rows' = {1} /\ UNCHANGED <<verified,allowed,audited>>
Next == Verify \/ Policy \/ Audit \/ Page \/ Commit
Spec == Init /\ [][Next]_vars
PrivateRead == phase \in {"Page","Gap","Committed"} => verified /\ allowed /\ audited
AtomicCursor == cursor = Cardinality(rows)
====
