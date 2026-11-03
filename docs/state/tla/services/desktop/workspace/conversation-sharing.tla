---- MODULE SharingDialog ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES opening, serial, changing, target, needsRead, editable
vars == <<opening, serial, changing, target, needsRead, editable>>
Init == /\ opening = 1 /\ serial = 1 /\ changing = FALSE /\ target = 0 /\ needsRead = TRUE /\ editable = FALSE
Read == /\ opening # 0 /\ ~changing /\ needsRead' = FALSE /\ editable' = TRUE /\ UNCHANGED <<opening,serial,changing,target>>
Change == /\ opening # 0 /\ editable /\ ~changing /\ changing' = TRUE /\ target' = opening /\ editable' = FALSE /\ UNCHANGED <<opening,serial,needsRead>>
Close == /\ opening' = 0 /\ editable' = FALSE /\ UNCHANGED <<serial,changing,target,needsRead>>
Reopen == /\ serial < 2 /\ serial' = serial + 1 /\ opening' = serial + 1 /\ editable' = FALSE /\ needsRead' = TRUE /\ UNCHANGED <<changing,target>>
Settle == /\ changing /\ changing' = FALSE /\ needsRead' = TRUE /\ editable' = FALSE /\ UNCHANGED <<opening,serial,target>>
Next == Read \/ Change \/ Close \/ Reopen \/ Settle
PendingHoldsForm == changing => ~editable
RereadBeforeEdit == editable => ~needsRead
ClosedHasNoForm == opening = 0 => ~editable
Spec == Init /\ [][Next]_vars
====
