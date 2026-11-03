---- MODULE CredentialSave ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, intent, effect, ack
vars == <<phase, intent, effect, ack>>
Init == /\ phase = "Target" /\ intent = FALSE /\ effect = "None" /\ ack = FALSE
Target == /\ phase = "Target" /\ phase' \in {"Intent", "Refused"} /\ UNCHANGED <<intent,effect,ack>>
Intent == /\ phase = "Intent" /\ phase' \in {"Write","Refused"} /\ intent' = (phase' = "Write") /\ UNCHANGED <<effect,ack>>
Write == /\ phase = "Write" /\ intent /\ phase' = "Outcome" /\ effect' \in {"Confirmed", "Refused", "Uncertain"} /\ UNCHANGED <<intent,ack>>
Outcome == /\ phase = "Outcome" /\ phase' = "Done" /\ ack' \in BOOLEAN /\ UNCHANGED <<intent,effect>>
Next == Target \/ Intent \/ Write \/ Outcome
Spec == Init /\ [][Next]_vars
IntentBeforeEffect == effect # "None" => intent
HonestAcknowledgement == ack => phase = "Done"
====
