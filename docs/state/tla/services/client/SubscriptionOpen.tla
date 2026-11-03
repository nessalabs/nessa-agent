---------------- MODULE SubscriptionOpen ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES phase, aborted, answered, closeDone, nextSent, buffer, done
vars == <<phase, aborted, answered, closeDone, nextSent, buffer, done>>
Init == phase = "Opening" /\ aborted = FALSE /\ answered = FALSE /\ closeDone = FALSE /\ nextSent = FALSE /\ buffer = 0 /\ done = FALSE
Abort == phase = "Opening" /\ phase' = "Aborted" /\ aborted' = TRUE /\ buffer' = 0 /\ done' = TRUE /\ UNCHANGED <<answered, closeDone, nextSent>>
AnswerLive == phase = "Opening" /\ ~aborted /\ phase' = "Open" /\ answered' = TRUE /\ UNCHANGED <<aborted, closeDone, nextSent, buffer, done>>
AnswerAborted == phase = "Aborted" /\ phase' = "Closing" /\ answered' = TRUE /\ UNCHANGED <<aborted, closeDone, nextSent, buffer, done>>
Close == phase = "Closing" /\ phase' = "Closed" /\ closeDone' = TRUE /\ UNCHANGED <<aborted, answered, nextSent, buffer, done>>
NextOpen == phase = "Closed" /\ closeDone /\ nextSent' = TRUE /\ UNCHANGED <<phase, aborted, answered, closeDone, buffer, done>>
Batch == phase = "Open" /\ buffer < 2 /\ ~done /\ buffer' = buffer + 1 /\ UNCHANGED <<phase, aborted, answered, closeDone, nextSent, done>>
Consume == buffer > 0 /\ ~aborted /\ buffer' = buffer - 1 /\ UNCHANGED <<phase, aborted, answered, closeDone, nextSent, done>>
RemoteEnd == phase = "Open" /\ phase' = "Closing" /\ done' = TRUE /\ UNCHANGED <<aborted, answered, closeDone, nextSent, buffer>>
Overflow == phase = "Open" /\ buffer = 2 /\ phase' = "Closing" /\ buffer' = 0 /\ done' = TRUE /\ UNCHANGED <<aborted, answered, closeDone, nextSent>>
Next == Abort \/ AnswerLive \/ AnswerAborted \/ Close \/ NextOpen \/ Batch \/ Consume \/ RemoteEnd \/ Overflow
Spec == Init /\ [][Next]_vars
TypeOK == phase \in {"Opening", "Open", "Aborted", "Closing", "Closed"} /\ aborted \in BOOLEAN /\ answered \in BOOLEAN /\ closeDone \in BOOLEAN /\ nextSent \in BOOLEAN /\ buffer \in 0..2 /\ done \in BOOLEAN
AbortedOpenClosedBeforeNext == nextSent /\ aborted => answered /\ closeDone
NoBufferedAfterAbort == aborted => buffer = 0
CloseAfterHandle == closeDone => answered
=============================================================================
