---- MODULE HostComposition ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, required, gateway, surface
vars == <<phase, required, gateway, surface>>
Init == /\ phase = "Composing" /\ required = FALSE /\ gateway = FALSE /\ surface = "None"
Compose == /\ phase = "Composing" /\ phase' \in {"Ready","Refused"} /\ required' = (phase' = "Ready") /\ UNCHANGED <<gateway,surface>>
Gateway == /\ phase = "Ready" /\ required /\ gateway' = TRUE /\ UNCHANGED <<phase,required,surface>>
Show == /\ phase \in {"Ready","Refused"} /\ surface' \in (IF phase = "Refused" THEN {"Refusal"} ELSE {"Setup","Desktop"}) /\ UNCHANGED <<phase,required,gateway>>
Next == Compose \/ Gateway \/ Show
Spec == Init /\ [][Next]_vars
RequiredBeforeGateway == gateway => required
RefusalSurface == phase = "Refused" => ~gateway /\ surface \in {"None","Refusal"}
====
