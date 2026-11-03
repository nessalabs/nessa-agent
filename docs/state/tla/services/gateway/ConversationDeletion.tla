---- MODULE ConversationDeletion ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, tombstone, link, erasure, audit, erased
vars == <<phase, tombstone, link, erasure, audit, erased>>
Init == /\ phase = "Live" /\ tombstone = FALSE /\ link = "Unread" /\ erasure = "None" /\ audit = FALSE /\ erased = FALSE
Delete == /\ phase = "Live" /\ phase' = "Stopping" /\ tombstone' = TRUE /\ UNCHANGED <<link,erasure,audit,erased>>
Stop == /\ phase = "Stopping" /\ phase' = "Reading" /\ UNCHANGED <<tombstone,link,erasure,audit,erased>>
Read == /\ phase = "Reading" /\ link' \in {"Absent","Unknown","Recorded"} /\ erasure' = (CASE link' = "Absent" -> "NoSession" [] link' = "Unknown" -> "Unknown" [] OTHER -> "None") /\ phase' = "Provider" /\ UNCHANGED <<tombstone,audit,erased>>
Provider == /\ phase = "Provider" /\ link = "Recorded" /\ erasure = "None" /\ erasure' \in {"Deleted","Archived","Acknowledged","Retained"} /\ UNCHANGED <<phase,tombstone,link,audit,erased>>
Audit == /\ phase = "Provider" /\ erasure # "None" /\ audit' = TRUE /\ phase' = "Erasing" /\ UNCHANGED <<tombstone,link,erasure,erased>>
Erase == /\ phase = "Erasing" /\ audit /\ erased' = TRUE /\ phase' = "Erased" /\ UNCHANGED <<tombstone,link,erasure,audit>>
Next == Delete \/ Stop \/ Read \/ Provider \/ Audit \/ Erase
Spec == Init /\ [][Next]_vars
ErasurePrerequisites == erased => tombstone /\ audit /\ erasure # "None"
UnknownIsNotAbsent == link = "Unknown" => erasure = "Unknown"
AbsentMeaning == link = "Absent" => erasure = "NoSession"
====
