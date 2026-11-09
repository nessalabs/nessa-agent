---- MODULE PastePayload ----
EXTENDS Naturals, FiniteSets
VARIABLES active, draft, viewer, owner, sent
vars == <<active, draft, viewer, owner, sent>>
Init == /\ active = 1 /\ draft = TRUE /\ viewer = "None" /\ owner = 0 /\ sent = "None"
Open == /\ active = 1 /\ draft /\ viewer' = "OriginalPayload" /\ owner' = active /\ UNCHANGED <<active,draft,sent>>
Switch == /\ active' = 3 - active /\ UNCHANGED <<draft,viewer,owner,sent>>
Close == /\ viewer # "None" /\ viewer' = "None" /\ owner' = 0 /\ UNCHANGED <<active,draft,sent>>
SendAccepted == /\ active = 1 /\ draft /\ draft' = FALSE /\ sent' = "OriginalPayload" /\ UNCHANGED <<active,viewer,owner>>
RestoreDraft == /\ ~draft /\ draft' = TRUE /\ UNCHANGED <<active,viewer,owner,sent>>
Next == Open \/ Switch \/ Close \/ SendAccepted \/ RestoreDraft
Spec == Init /\ [][Next]_vars
ViewerRetainsPayload == viewer # "None" => viewer = "OriginalPayload" /\ owner = 1
SendUsesPayload == sent \in {"None","OriginalPayload"}
====
