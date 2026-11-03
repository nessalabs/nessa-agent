---- MODULE RuleRevocation ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES binding, owner, phase, listed, ack, reported
vars == <<binding, owner, phase, listed, ack, reported>>
Init == /\ binding = 1 /\ owner = 0 /\ phase = "Ready" /\ listed = TRUE /\ ack = "none" /\ reported = "none"
Revoke == /\ phase = "Ready" /\ owner' = binding /\ phase' = "Revoking" /\ reported' = "none" /\ UNCHANGED <<binding,listed,ack>>
Answer == /\ phase = "Revoking" /\ owner = binding /\ phase' = "Rereading" /\ ack' \in {"ok","unknown"} /\ UNCHANGED <<binding,owner,listed,reported>>
Read == \E actual \in BOOLEAN: /\ phase = "Rereading" /\ owner = binding /\ listed' = actual /\ phase' = "Ready" /\ owner' = 0 /\ reported' = (IF actual THEN "retained" ELSE "absent") /\ UNCHANGED <<binding,ack>>
Rebind == /\ binding = 1 /\ binding' = 2 /\ phase' = "Ready" /\ reported' = "none" /\ UNCHANGED <<owner,listed,ack>>
Next == Revoke \/ Answer \/ Read \/ Rebind
RereadMeaning == reported = "absent" => ~listed
RetainedMeaning == reported = "retained" => listed
NoEarlySuccess == phase = "Rereading" => reported # "absent"
Spec == Init /\ [][Next]_vars
====
