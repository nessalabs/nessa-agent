---- MODULE NativeTicket ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, spent, reads, expired
vars == <<phase, spent, reads, expired>>
Init == /\ phase = "Minted" /\ spent = FALSE /\ reads = 0 /\ expired = FALSE
Expire == /\ ~expired /\ expired' = TRUE /\ UNCHANGED <<phase,spent,reads>>
Redeem == /\ phase = "Minted" /\ ~spent /\ ~expired /\ spent' = TRUE /\ reads' = reads + 1 /\ phase' = "Reading" /\ UNCHANGED expired
Finish == /\ phase = "Reading" /\ phase' \in {"Read","ReadFailed"} /\ UNCHANGED <<spent,reads,expired>>
Evict == /\ phase # "Unknown" /\ phase' = "Unknown" /\ UNCHANGED <<spent,reads,expired>>
Next == Expire \/ Redeem \/ Finish \/ Evict
Spec == Init /\ [][Next]_vars
SingleUse == reads <= 1
ReadSpendsBeforeIO == reads > 0 => spent
====
