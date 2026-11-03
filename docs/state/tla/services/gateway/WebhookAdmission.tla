---- MODULE WebhookAdmission ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, credential, grant, checked, expired, posted
vars == <<phase, credential, grant, checked, expired, posted>>
Init == /\ phase = "Credential" /\ credential = FALSE /\ grant = FALSE /\ checked = FALSE /\ expired = FALSE /\ posted = FALSE
Credential == /\ phase = "Credential" /\ phase' = "Grant" /\ credential' = TRUE /\ UNCHANGED <<grant,checked,expired,posted>>
Grant == /\ phase = "Grant" /\ phase' = "Policy" /\ grant' = TRUE /\ UNCHANGED <<credential,checked,expired,posted>>
Policy == /\ phase = "Policy" /\ phase' = "Held" /\ checked' = TRUE /\ UNCHANGED <<credential,grant,expired,posted>>
Expire == /\ ~expired /\ expired' = TRUE /\ UNCHANGED <<phase,credential,grant,checked,posted>>
Cancel == /\ phase # "Done" /\ phase' = "Cancelled" /\ credential' = FALSE /\ grant' = FALSE /\ checked' = FALSE /\ UNCHANGED <<expired,posted>>
Post == /\ phase = "Held" /\ credential /\ grant /\ checked /\ ~expired /\ phase' = "Done" /\ posted' = TRUE /\ UNCHANGED <<credential,grant,checked,expired>>
Next == Credential \/ Grant \/ Policy \/ Expire \/ Cancel \/ Post
Spec == Init /\ [][Next]_vars
PartialOrder == grant => credential
CheckedOwnsFences == checked => credential /\ grant
NoCancelledPost == phase = "Cancelled" => ~posted
====
