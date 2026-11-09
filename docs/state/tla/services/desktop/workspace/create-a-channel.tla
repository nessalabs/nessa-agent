---- MODULE ChannelOpening ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES opening, serial, pending, listed, navigated
vars == <<opening, serial, pending, listed, navigated>>
Init == /\ opening = 0 /\ serial = 0 /\ pending = {} /\ listed = {} /\ navigated = 0
Open == /\ serial < 2 /\ serial' = serial + 1 /\ opening' = serial + 1 /\ navigated' = 0 /\ UNCHANGED <<pending,listed>>
Create == /\ opening # 0 /\ opening \notin pending /\ pending' = pending \cup {opening} /\ UNCHANGED <<opening,serial,listed,navigated>>
Close == /\ opening' = 0 /\ navigated' = 0 /\ UNCHANGED <<serial,pending,listed>>
Made == \E owner \in pending: /\ pending' = pending \ {owner} /\ listed' = listed \cup {owner} /\ navigated' = (IF owner = opening THEN owner ELSE navigated) /\ UNCHANGED <<opening,serial>>
Refused == \E owner \in pending: /\ pending' = pending \ {owner} /\ UNCHANGED <<opening,serial,listed,navigated>>
Next == Open \/ Create \/ Close \/ Made \/ Refused
NavigationOwner == navigated = 0 \/ navigated = opening
NavigationListed == navigated = 0 \/ navigated \in listed
Spec == Init /\ [][Next]_vars
====
