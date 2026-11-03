---- MODULE BackgroundTask ----
EXTENDS Naturals, FiniteSets
VARIABLES outcome, stop, delivery, cleanup, revision
vars == <<outcome, stop, delivery, cleanup, revision>>
Init == /\ outcome = "None" /\ stop = "None" /\ delivery = "Waiting" /\ cleanup = FALSE /\ revision = 0
Stop == /\ stop = "None" /\ outcome = "None" /\ stop' \in {"Requested", "Deleted", "Shutdown"} /\ revision' = revision + 1 /\ UNCHANGED <<outcome,delivery,cleanup>>
Settle == /\ outcome = "None" /\ outcome' \in {"Exited", "Interrupted"} /\ cleanup' \in BOOLEAN /\ (outcome' = "Interrupted" => ~cleanup') /\ revision' = revision + 1 /\ UNCHANGED <<stop,delivery>>
Deliver == /\ outcome # "None" /\ delivery = "Waiting" /\ delivery' \in {"Delivered", "Discarded"} /\ revision' = revision + 1 /\ UNCHANGED <<outcome,stop,cleanup>>
Next == Stop \/ Settle \/ Deliver
Spec == Init /\ [][Next]_vars
OutcomeBeforeDelivery == delivery # "Waiting" => outcome # "None"
InterruptedIsUncertain == outcome = "Interrupted" => ~cleanup
BoundedRevision == revision <= 3
====
