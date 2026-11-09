---- MODULE LinkedFileEvidence ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, saved, dispatched, read
vars == <<phase, saved, dispatched, read>>
Init == /\ phase = "Audit" /\ saved = FALSE /\ dispatched = FALSE /\ read = FALSE
Audit == /\ phase = "Audit" /\ phase' \in {"Admit","Refused"} /\ saved' = (phase' = "Admit") /\ UNCHANGED <<dispatched,read>>
Dispatch == /\ phase = "Admit" /\ saved /\ dispatched' = TRUE /\ phase' = "Dispatched" /\ UNCHANGED <<saved,read>>
Next == Audit \/ Dispatch
Spec == Init /\ [][Next]_vars
EvidenceBeforeDispatch == dispatched => saved
NamingDoesNotRead == ~read
====
