---- MODULE BoatAssignment ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, vm, createCount, authority, verified
vars == <<phase, vm, createCount, authority, verified>>
Init == /\ phase = "New" /\ vm = "None" /\ createCount = 0 /\ authority = FALSE /\ verified = FALSE
Intent == /\ phase = "New" /\ phase' = "Provisioning" /\ UNCHANGED <<vm,createCount,authority,verified>>
Create == /\ phase = "Provisioning" /\ createCount = 0 /\ createCount' = 1 /\ vm' = "VM1" /\ phase' \in {"Assigned","Refused","Uncertain"} /\ UNCHANGED <<authority,verified>>
Verify == /\ phase = "Assigned" /\ phase' \in {"Ready","Refused"} /\ verified' = (phase' = "Ready") /\ authority' = (phase' = "Ready") /\ UNCHANGED <<vm,createCount>>
Close == /\ phase = "Ready" /\ phase' = "Assigned" /\ authority' = FALSE /\ verified' = FALSE /\ UNCHANGED <<vm,createCount>>
Next == Intent \/ Create \/ Verify \/ Close
Spec == Init /\ [][Next]_vars
OneTarget == createCount <= 1 /\ (createCount = 1 => vm = "VM1")
VerifyBeforeAuthority == authority => verified /\ phase = "Ready"
RefusalIsTerminal == phase = "Refused" => ~authority
====
