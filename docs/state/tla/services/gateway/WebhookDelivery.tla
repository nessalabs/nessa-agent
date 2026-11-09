---- MODULE WebhookDelivery ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, attempts, epoch, due, timestamp
vars == <<phase, attempts, epoch, due, timestamp>>
Init == /\ phase = "Queued" /\ attempts = 0 /\ epoch = 1 /\ due = FALSE /\ timestamp = FALSE
Admit == /\ phase \in {"Queued","Retry"} /\ epoch = 1 /\ (phase = "Retry" => due) /\ attempts < 5 /\ phase' = "Sending" /\ attempts' = attempts + 1 /\ timestamp' = TRUE /\ due' = FALSE /\ UNCHANGED epoch
Observe == /\ phase = "Sending" /\ phase' \in {"Sent","NotSent","TimedOut","Unknown","Retry"} /\ (phase' = "Retry" => attempts < 5) /\ UNCHANGED <<attempts,epoch,due,timestamp>>
Due == /\ phase = "Retry" /\ due' = TRUE /\ UNCHANGED <<phase,attempts,epoch,timestamp>>
Revoke == /\ epoch = 1 /\ epoch' = 2 /\ UNCHANGED <<phase,attempts,due,timestamp>>
Refuse == /\ phase \in {"Queued","Retry"} /\ phase' = "NotSent" /\ UNCHANGED <<attempts,epoch,due,timestamp>>
Next == Admit \/ Observe \/ Due \/ Revoke \/ Refuse
Spec == Init /\ [][Next]_vars
AttemptBound == attempts <= 5
TimestampShape == timestamp = (attempts > 0)
RetryBudget == phase = "Retry" => attempts < 5
====
