---- MODULE SubmissionRecovery ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES phase, identity, attempt, admitted, retryIdentity
vars == <<phase, identity, attempt, admitted, retryIdentity>>
Init == /\ phase = "Draft" /\ identity = "input-A" /\ attempt = 0 /\ admitted = FALSE /\ retryIdentity = "input-A"
Send == /\ phase = "Draft" /\ phase' = "Sending" /\ attempt' = 1 /\ UNCHANGED <<identity,admitted,retryIdentity>>
Accepted == /\ phase = "Sending" /\ phase' = "Accepted" /\ admitted' = TRUE /\ UNCHANGED <<identity,attempt,retryIdentity>>
Unknown == /\ phase = "Sending" /\ phase' = "Unknown" /\ UNCHANGED <<identity,attempt,admitted,retryIdentity>>
Refused == /\ phase = "Sending" /\ phase' = "Refused" /\ UNCHANGED <<identity,attempt,admitted,retryIdentity>>
Retry == /\ phase \in {"Unknown","Refused"} /\ attempt < 2 /\ phase' = "Sending" /\ attempt' = attempt + 1 /\ retryIdentity' = identity /\ UNCHANGED <<identity,admitted>>
Reconcile == /\ phase = "Unknown" /\ phase' = "Accepted" /\ admitted' = TRUE /\ UNCHANGED <<identity,attempt,retryIdentity>>
Next == Send \/ Accepted \/ Unknown \/ Refused \/ Retry \/ Reconcile
StableRetry == retryIdentity = identity
ReceiptMeaning == admitted => phase = "Accepted"
Spec == Init /\ [][Next]_vars
====
