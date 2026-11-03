---------------- MODULE RegistryRefusal ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES phase, registryRead, preserved, originalFault, audit
vars == <<phase, registryRead, preserved, originalFault, audit>>
Init == phase = "Observed" /\ registryRead = FALSE /\ preserved = FALSE /\ originalFault = FALSE /\ audit = FALSE
UnsafeLock == phase = "Observed" /\ phase' = "LockPreserved" /\ preserved' = TRUE /\ originalFault' = TRUE /\ UNCHANGED <<registryRead, audit>>
BadRegistry == phase = "Observed" /\ phase' = "RegistryPreserved" /\ registryRead' = TRUE /\ preserved' = TRUE /\ originalFault' = TRUE /\ UNCHANGED <<audit>>
Record == phase \in {"LockPreserved", "RegistryPreserved"} /\ phase' = "Recorded" /\ audit' = TRUE /\ UNCHANGED <<registryRead, preserved, originalFault>>
AuditFail == phase \in {"LockPreserved", "RegistryPreserved"} /\ phase' = "AuditUnavailable" /\ UNCHANGED <<registryRead, preserved, originalFault, audit>>
Next == UnsafeLock \/ BadRegistry \/ Record \/ AuditFail
Spec == Init /\ [][Next]_vars
TypeOK == phase \in {"Observed", "LockPreserved", "RegistryPreserved", "Recorded", "AuditUnavailable"} /\ registryRead \in BOOLEAN /\ preserved \in BOOLEAN /\ originalFault \in BOOLEAN /\ audit \in BOOLEAN
RefusalPreserves == phase # "Observed" => preserved /\ originalFault
UnsafeLockBeforeRead == phase = "LockPreserved" => ~registryRead
RecordedNeedsAudit == phase = "Recorded" => audit
=============================================================================
