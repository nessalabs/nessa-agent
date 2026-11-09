---------------- MODULE DatabaseOpen ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES phase, original, version, staged, chain, integrity, committed
vars == <<phase, original, version, staged, chain, integrity, committed>>
Init == phase = "Inspecting" /\ original = 2 /\ version = 2 /\ staged = 2 /\ chain = FALSE /\ integrity = FALSE /\ committed = FALSE
Chain == phase = "Inspecting" /\ chain' = TRUE /\ UNCHANGED <<phase, original, version, staged, integrity, committed>>
Lock == phase = "Inspecting" /\ chain /\ phase' = "Locked" /\ UNCHANGED <<original, version, staged, chain, integrity, committed>>
Check == phase = "Locked" /\ integrity' = TRUE /\ UNCHANGED <<phase, original, version, staged, chain, committed>>
Upgrade == phase = "Locked" /\ integrity /\ chain /\ phase' = "Staged" /\ staged' = 3 /\ UNCHANGED <<original, version, chain, integrity, committed>>
Commit == phase = "Staged" /\ integrity /\ phase' = "Open" /\ version' = staged /\ committed' = TRUE /\ UNCHANGED <<original, staged, chain, integrity>>
Rollback == phase \in {"Inspecting", "Locked", "Staged"} /\ phase' = "Refused" /\ version' = original /\ staged' = original /\ UNCHANGED <<original, chain, integrity, committed>>
Next == Chain \/ Lock \/ Check \/ Upgrade \/ Commit \/ Rollback
Spec == Init /\ [][Next]_vars
TypeOK == phase \in {"Inspecting", "Locked", "Staged", "Open", "Refused"} /\ original \in 0..3 /\ version \in 0..3 /\ staged \in 0..3 /\ chain \in BOOLEAN /\ integrity \in BOOLEAN /\ committed \in BOOLEAN
NoPartialVersion == ~committed => version = original
OpenRequiresVersion == phase = "Open" => version = 3 /\ integrity /\ chain
RefusalPreserves == phase = "Refused" => version = original
StagedRequiresValidation == phase \in {"Staged", "Open"} => integrity /\ chain
=============================================================================
