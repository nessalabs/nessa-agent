---- MODULE WebhookRegistration ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, intent, issued, installed, stopped, secret
vars == <<phase, intent, issued, installed, stopped, secret>>
Init == /\ phase = "Reserve" /\ intent = FALSE /\ issued = FALSE /\ installed = FALSE /\ stopped = FALSE /\ secret = FALSE
Reserve == /\ phase = "Reserve" /\ intent' = TRUE /\ phase' = "Issue" /\ UNCHANGED <<issued,installed,stopped,secret>>
Issue == /\ phase = "Issue" /\ intent /\ issued' = TRUE /\ phase' = "Validate" /\ UNCHANGED <<intent,installed,stopped,secret>>
Validate == /\ phase = "Validate" /\ phase' \in {"Install","Refused"} /\ UNCHANGED <<intent,issued,installed,stopped,secret>>
Stop == /\ phase \in {"Validate","Install"} /\ stopped' = TRUE /\ phase' = "Cancelled" /\ intent' = FALSE /\ UNCHANGED <<issued,installed,secret>>
Install == /\ phase = "Install" /\ ~stopped /\ installed' = TRUE /\ phase' = "Registered" /\ UNCHANGED <<intent,issued,stopped,secret>>
Next == Reserve \/ Issue \/ Validate \/ Stop \/ Install
Spec == Init /\ [][Next]_vars
ReservationBeforeIssue == issued /\ ~stopped => intent
NoStoppedInstall == stopped => ~installed
NoBearerResult == ~secret
====
