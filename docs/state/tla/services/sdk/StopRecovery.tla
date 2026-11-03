---------------- MODULE StopRecovery ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES phase, cause, edge, declined, successors, restarts
vars == <<phase, cause, edge, declined, successors, restarts>>
Init == phase = "Unfinished" /\ cause \in {"Person", "Session", "Interrupted", "RetryableFailure"} /\ edge = FALSE /\ declined = FALSE /\ successors = 0 /\ restarts = 0
Restart == restarts < 2 /\ restarts' = restarts + 1 /\ UNCHANGED <<phase, cause, edge, declined, successors>>
SaveSourceEdge == phase = "Unfinished" /\ phase' = "Settled" /\ edge' = TRUE /\ UNCHANGED <<cause, declined, successors, restarts>>
FinishCapturedStop == phase = "Settled" /\ cause \in {"Person", "Session"} /\ phase' = "Done" /\ UNCHANGED <<cause, edge, declined, successors, restarts>>
DeclineOwed == phase = "Settled" /\ cause \in {"Interrupted", "RetryableFailure"} /\ successors = 0 /\ declined' = TRUE /\ UNCHANGED <<phase, cause, edge, successors, restarts>>
AdmitSuccessor == phase = "Settled" /\ cause \in {"Interrupted", "RetryableFailure"} /\ ~declined /\ edge /\ phase' = "Continued" /\ successors' = 1 /\ UNCHANGED <<cause, edge, declined, restarts>>
Next == Restart \/ SaveSourceEdge \/ FinishCapturedStop \/ DeclineOwed \/ AdmitSuccessor
Spec == Init /\ [][Next]_vars
TypeOK == phase \in {"Unfinished", "Settled", "Done", "Continued"} /\ cause \in {"Person", "Session", "Interrupted", "RetryableFailure"} /\ edge \in BOOLEAN /\ declined \in BOOLEAN /\ successors \in 0..1 /\ restarts \in 0..2
CapturedStopNeverContinued == cause \in {"Person", "Session"} => successors = 0
DeclineWinsBeforeAdmission == declined => successors = 0
SuccessorNeedsSourceEdge == successors = 1 => edge
FinalStopKeepsCause == cause \in {"Person", "Session"} => ~declined
=============================================================================
