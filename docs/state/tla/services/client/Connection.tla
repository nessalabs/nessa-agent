---------------- MODULE Connection ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES phase, attempt, candidate, terminated, closed
vars == <<phase, attempt, candidate, terminated, closed>>
Init == phase = "Connected" /\ attempt = 0 /\ candidate = FALSE /\ terminated = FALSE /\ closed = FALSE
Lose == phase = "Connected" /\ attempt = 0 /\ phase' = "Reconnecting" /\ UNCHANGED <<attempt, candidate, terminated, closed>>
Attempt == phase = "Reconnecting" /\ attempt < 2 /\ ~candidate /\ attempt' = attempt + 1 /\ candidate' = TRUE /\ terminated' = FALSE /\ UNCHANGED <<phase, closed>>
CandidateClosed == phase = "Reconnecting" /\ candidate /\ candidate' = TRUE /\ terminated' = TRUE /\ UNCHANGED <<phase, attempt, closed>>
Adopt == phase = "Reconnecting" /\ candidate /\ ~closed /\ ~terminated /\ phase' = "Connected" /\ candidate' = FALSE /\ UNCHANGED <<attempt, terminated, closed>>
Close == ~closed /\ phase' = "Closed" /\ closed' = TRUE /\ candidate' = FALSE /\ UNCHANGED <<attempt, terminated>>
Exhaust == phase = "Reconnecting" /\ attempt = 2 /\ ~candidate /\ phase' = "Closed" /\ closed' = TRUE /\ UNCHANGED <<attempt, candidate, terminated>>
RetryDead == phase = "Reconnecting" /\ candidate /\ terminated /\ candidate' = FALSE /\ UNCHANGED <<phase, attempt, terminated, closed>>
Next == RetryDead \/ Lose \/ Attempt \/ CandidateClosed \/ Adopt \/ Close \/ Exhaust
Spec == Init /\ [][Next]_vars
TypeOK == phase \in {"Connected", "Reconnecting", "Closed"} /\ attempt \in 0..2 /\ candidate \in BOOLEAN /\ terminated \in BOOLEAN /\ closed \in BOOLEAN
NoResurrection == closed => phase = "Closed"
NoDeadAdoption == phase = "Connected" => ~terminated
BoundedAttempts == attempt <= 2
=============================================================================
