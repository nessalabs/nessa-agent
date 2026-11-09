---------------- MODULE EventRegistration ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES current, oldHandle, active, reinstalled, staleRemoved
vars == <<current, oldHandle, active, reinstalled, staleRemoved>>
Init == current = 0 /\ oldHandle = FALSE /\ active = FALSE /\ reinstalled = 0 /\ staleRemoved = FALSE
Register == current = 0 /\ current' = 1 /\ oldHandle' = TRUE /\ active' = TRUE /\ UNCHANGED <<reinstalled, staleRemoved>>
Remove == current = 1 /\ active /\ active' = FALSE /\ UNCHANGED <<current, oldHandle, reinstalled, staleRemoved>>
Replace == current = 1 /\ ~active /\ current' = 2 /\ active' = TRUE /\ UNCHANGED <<oldHandle, reinstalled, staleRemoved>>
OldUnsubscribe == oldHandle /\ current = 1 /\ active' = FALSE /\ oldHandle' = FALSE /\ UNCHANGED <<current, reinstalled, staleRemoved>>
StaleUnsubscribe == oldHandle /\ current = 2 /\ oldHandle' = FALSE /\ UNCHANGED <<current, active, reinstalled, staleRemoved>>
Recover == active /\ reinstalled' = current /\ UNCHANGED <<current, oldHandle, active, staleRemoved>>
Next == Register \/ Remove \/ Replace \/ OldUnsubscribe \/ StaleUnsubscribe \/ Recover
Spec == Init /\ [][Next]_vars
TypeOK == current \in 0..2 /\ oldHandle \in BOOLEAN /\ active \in BOOLEAN /\ reinstalled \in 0..2 /\ staleRemoved \in BOOLEAN
StaleNeverRemovesReplacement == ~staleRemoved
ReinstallCurrent == reinstalled = 2 => current = 2
ReplacementSurvivesOldHandle == current = 2 => active
=============================================================================
