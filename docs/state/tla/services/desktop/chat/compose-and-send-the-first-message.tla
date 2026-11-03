---- MODULE ComposerObservation ----
EXTENDS Naturals, FiniteSets
VARIABLES active, observation, owner, serial, expanded, before, callback, last
vars == <<active, observation, owner, serial, expanded, before, callback, last>>
Init == /\ active = "A" /\ observation = 0 /\ owner = "A" /\ serial = 0 /\ expanded = TRUE /\ before = 0 /\ callback = 0 /\ last = "init"
Submit == /\ serial < 2 /\ serial' = serial + 1 /\ observation' = serial + 1 /\ owner' = active /\ last' = "submit" /\ UNCHANGED <<active,expanded,before,callback>>
Switch == /\ active' \in {"A","B"} \ {active} /\ expanded' = FALSE /\ last' = "switch" /\ UNCHANGED <<observation,owner,serial,before,callback>>
Expand == /\ expanded' = TRUE /\ last' = "expand" /\ UNCHANGED <<active,observation,owner,serial,before,callback>>
Empty == /\ observation # 0 /\ owner = active /\ observation' = 0 /\ expanded' = FALSE /\ last' = "empty" /\ UNCHANGED <<active,owner,serial,before,callback>>
Refusal == \E reply \in 1..2: /\ before' = observation /\ callback' = reply /\ observation' = (IF reply = observation THEN 0 ELSE observation) /\ last' = "refusal" /\ UNCHANGED <<active,owner,serial,expanded>>
Next == Submit \/ Switch \/ Expand \/ Empty \/ Refusal
ObservationBound == observation = 0 \/ observation = serial
StaleCallbackPreservesReplacement == (last = "refusal" /\ callback # before) => observation = before
Spec == Init /\ [][Next]_vars
====
