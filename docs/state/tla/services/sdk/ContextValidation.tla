---------------- MODULE ContextValidation ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES phase, count, prefix, candidate, sameId, absent, namespace, steps, valid, previous
vars == <<phase, count, prefix, candidate, sameId, absent, namespace, steps, valid, previous>>
Init == phase = "Inspecting" /\ count \in 0..2 /\ prefix \in 0..2 /\ candidate \in 0..2 /\ sameId \in BOOLEAN /\ absent \in BOOLEAN /\ namespace = 0 /\ steps = 0 /\ valid = FALSE /\ previous \in 0..2 /\ previous <= prefix /\ prefix <= count
ChangeNamespace == phase = "Inspecting" /\ steps = 0 /\ namespace' = 1 /\ steps' = 1 /\ UNCHANGED <<phase, count, prefix, candidate, sameId, absent, valid, previous>>
ReturnNamespace == phase = "Inspecting" /\ steps = 1 /\ namespace' = 0 /\ steps' = 2 /\ UNCHANGED <<phase, count, prefix, candidate, sameId, absent, valid, previous>>
Validate == phase = "Inspecting" /\ prefix <= count /\ (IF absent THEN count = previous ELSE candidate <= prefix /\ (~sameId \/ candidate = previous \/ namespace = 1)) /\ phase' = "Validated" /\ valid' = TRUE /\ UNCHANGED <<count, prefix, candidate, sameId, absent, namespace, steps, previous>>
Refuse == phase = "Inspecting" /\ phase' = "Refused" /\ UNCHANGED <<count, prefix, candidate, sameId, absent, namespace, steps, valid, previous>>
Next == ChangeNamespace \/ ReturnNamespace \/ Validate \/ Refuse
Spec == Init /\ [][Next]_vars
TypeOK == phase \in {"Inspecting", "Validated", "Refused"} /\ count \in 0..2 /\ prefix \in 0..2 /\ candidate \in 0..2 /\ sameId \in BOOLEAN /\ absent \in BOOLEAN /\ namespace \in 0..1 /\ steps \in 0..2 /\ valid \in BOOLEAN /\ previous \in 0..2
ValidRetainedCounts == valid => prefix <= count
BoundaryWithinTerminal == valid /\ ~absent => candidate <= prefix
NoClearUsedContext == valid /\ absent => count = previous
SameIdResetNeedsPriorNetChange == valid /\ ~absent /\ sameId /\ candidate # previous => namespace = 1 /\ steps = 1
=============================================================================
