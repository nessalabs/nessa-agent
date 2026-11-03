---- MODULE WebhookOutbox ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, sendingSaved, observation, audit, posts
vars == <<phase, sendingSaved, observation, audit, posts>>
Init == /\ phase = "Queued" /\ sendingSaved = FALSE /\ observation = "None" /\ audit = "None" /\ posts = 0
SaveSending == /\ phase = "Queued" /\ phase' = "Sending" /\ sendingSaved' = TRUE /\ audit' = "Owed" /\ UNCHANGED <<observation,posts>>
Publish == /\ audit = "Owed" /\ audit' = "Published" /\ UNCHANGED <<phase,sendingSaved,observation,posts>>
Post == /\ phase = "Sending" /\ audit = "Published" /\ posts = 0 /\ posts' = 1 /\ UNCHANGED <<phase,sendingSaved,observation,audit>>
Observe == /\ phase = "Sending" /\ posts = 1 /\ observation = "None" /\ observation' \in {"Success","Failure"} /\ UNCHANGED <<phase,sendingSaved,audit,posts>>
SaveOutcome == /\ phase = "Sending" /\ observation # "None" /\ phase' = "Terminal" /\ audit' = "Owed" /\ UNCHANGED <<sendingSaved,observation,posts>>
Recover == /\ phase = "Sending" /\ observation = "None" /\ phase' = "Unknown" /\ audit' = "Owed" /\ UNCHANGED <<sendingSaved,observation,posts>>
Next == SaveSending \/ Publish \/ Post \/ Observe \/ SaveOutcome \/ Recover
Spec == Init /\ [][Next]_vars
PersistBeforePost == posts > 0 => sendingSaved
KnownBeforeTerminal == phase = "Terminal" => observation # "None"
OneAttempt == posts <= 1
====
