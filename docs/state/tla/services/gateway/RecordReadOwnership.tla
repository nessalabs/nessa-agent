---- MODULE RecordReadOwnership ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, lease, caller, joined
vars == <<phase, lease, caller, joined>>
Init == /\ phase = "Reserved" /\ lease = "Caller" /\ caller = TRUE /\ joined = FALSE
Admit == /\ phase = "Reserved" /\ phase' = "Reading" /\ lease' = "Source" /\ UNCHANGED <<caller,joined>>
Timeout == /\ phase = "Reading" /\ phase' = "TimedOutRetained" /\ caller' = FALSE /\ UNCHANGED <<lease,joined>>
Join == /\ phase \in {"Reading","TimedOutRetained"} /\ joined' = TRUE /\ phase' = (IF caller THEN "Encoding" ELSE "Released") /\ lease' = (IF caller THEN "Writer" ELSE "None") /\ UNCHANGED caller
Encode == /\ phase = "Encoding" /\ phase' \in {"Queued","Released"} /\ lease' = (IF phase' = "Queued" THEN "Writer" ELSE "None") /\ UNCHANGED <<caller,joined>>
Select == /\ phase = "Queued" /\ phase' = "Sending" /\ lease' = "None" /\ UNCHANGED <<caller,joined>>
Next == Admit \/ Timeout \/ Join \/ Encode \/ Select
Spec == Init /\ [][Next]_vars
RetainedUntilJoin == phase \in {"Reading","TimedOutRetained"} => lease = "Source" /\ ~joined
ReleaseBeforeSend == phase = "Sending" => lease = "None" /\ joined
====
