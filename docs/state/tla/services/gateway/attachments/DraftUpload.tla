---- MODULE DraftUpload ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, present, activeTab, target, staged
vars == <<phase, present, activeTab, target, staged>>
Init == /\ phase = "Waiting" /\ present = TRUE /\ activeTab = 1 /\ target = 1 /\ staged = FALSE
Switch == /\ activeTab' = 3 - activeTab /\ UNCHANGED <<phase,present,target,staged>>
Start == /\ phase = "Waiting" /\ present /\ phase' = "Hashing" /\ UNCHANGED <<present,activeTab,target,staged>>
Remove == /\ present /\ present' = FALSE /\ UNCHANGED <<phase,activeTab,target,staged>>
Hash == /\ phase = "Hashing" /\ phase' = (IF present THEN "Ready" ELSE "Aborted") /\ UNCHANGED <<present,activeTab,target,staged>>
Stage == /\ phase = "Ready" /\ present /\ phase' = "Staged" /\ staged' = TRUE /\ UNCHANGED <<present,activeTab,target>>
Fail == /\ phase \in {"Hashing","Ready"} /\ phase' = "Failed" /\ UNCHANGED <<present,activeTab,target,staged>>
Next == Switch \/ Start \/ Remove \/ Hash \/ Stage \/ Fail
Spec == Init /\ [][Next]_vars
CapturedTarget == target = 1
NoStageWithoutHash == staged => phase = "Staged"
====
