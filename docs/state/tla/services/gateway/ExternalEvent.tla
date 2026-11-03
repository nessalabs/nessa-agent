---- MODULE ExternalEvent ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, authority, request, dispatches, ack
vars == <<phase, authority, request, dispatches, ack>>
Init == /\ phase = "Authorize" /\ authority = TRUE /\ request = "event1" /\ dispatches = 0 /\ ack = FALSE
Revoke == /\ authority /\ authority' = FALSE /\ UNCHANGED <<phase,request,dispatches,ack>>
Authorize == /\ phase = "Authorize" /\ authority /\ phase' = "Submit" /\ UNCHANGED <<authority,request,dispatches,ack>>
Submit == /\ phase = "Submit" /\ dispatches = 0 /\ dispatches' = 1 /\ phase' = "Dispatched" /\ UNCHANGED <<authority,request,ack>>
Acknowledge == /\ phase = "Dispatched" /\ ack' = TRUE /\ phase' = "Authorize" /\ UNCHANGED <<authority,request,dispatches>>
Replay == /\ phase = "Submit" /\ dispatches = 1 /\ phase' = "Dispatched" /\ UNCHANGED <<authority,request,dispatches,ack>>
Next == Revoke \/ Authorize \/ Submit \/ Acknowledge \/ Replay
Spec == Init /\ [][Next]_vars
OneEventIdentity == request = "event1" /\ dispatches <= 1
AckIsDispatch == ack => dispatches = 1
====
