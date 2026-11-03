---- MODULE ProviderRecovery ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES published, refused, observed, offered
vars == <<published, refused, observed, offered>>
Init == /\ published \in {"A","B"} /\ refused = "A" /\ observed = {} /\ offered = FALSE
LocalSend == /\ observed' = observed \cup {published} /\ offered' = FALSE /\ UNCHANGED <<published,refused>>
NewInput == /\ published' \in {"A","B"} /\ offered' = FALSE /\ UNCHANGED <<refused,observed>>
Offer == /\ refused = published /\ published \notin observed /\ offered' = TRUE /\ UNCHANGED <<published,refused,observed>>
Launch == /\ offered /\ UNCHANGED vars
Next == LocalSend \/ NewInput \/ Offer \/ Launch
RecoveryCausality == offered => (refused = published /\ published \notin observed)
Spec == Init /\ [][Next]_vars
====
