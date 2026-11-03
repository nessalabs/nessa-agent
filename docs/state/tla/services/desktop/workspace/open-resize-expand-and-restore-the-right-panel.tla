---- MODULE WidgetPanel ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES tabs, front, chat, byChat
vars == <<tabs, front, chat, byChat>>
Init == /\ tabs = {} /\ front = "none" /\ chat = "A" /\ byChat = [c \in {"A","B"} |-> {}]
Open == \E widget \in {"app1","app2"}: /\ tabs' = {widget} /\ front' = widget /\ byChat' = [byChat EXCEPT ![chat] = {widget}] /\ UNCHANGED chat
Close == /\ tabs # {} /\ tabs' = {} /\ front' = "none" /\ byChat' = [byChat EXCEPT ![chat] = {}] /\ UNCHANGED chat
Switch == \E next \in {"A","B"}: /\ next # chat /\ chat' = next /\ tabs' = byChat[next] /\ front' = (IF byChat[next] = {} THEN "none" ELSE CHOOSE w \in byChat[next]: TRUE) /\ UNCHANGED byChat
Next == Open \/ Close \/ Switch
PerChatOwnership == tabs = byChat[chat]
FrontOwned == (front = "none" /\ tabs = {}) \/ front \in tabs
OneAppPerChat == Cardinality(tabs) <= 1
Spec == Init /\ [][Next]_vars
====
