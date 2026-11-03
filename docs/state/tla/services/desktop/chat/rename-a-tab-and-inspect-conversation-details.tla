---- MODULE LocalTabTitle ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES title, edited, chosen, last, before, target
vars == <<title, edited, chosen, last, before, target>>
Init == /\ title = [tab \in {"A","B"} |-> "gateway"] /\ edited = {} /\ chosen = [tab \in {"A","B"} |-> "gateway"] /\ last = "init" /\ before = title /\ target = "A"
Rename == \E tab \in {"A","B"}: /\ target' = tab /\ before' = title /\ title' = [title EXCEPT ![tab] = "local"] /\ chosen' = [chosen EXCEPT ![tab] = "local"] /\ edited' = edited \cup {tab} /\ last' = "rename"
Project == \E tab \in {"A","B"}: /\ target' = tab /\ before' = title /\ title' = [title EXCEPT ![tab] = (IF tab \in edited THEN @ ELSE "new-gateway")] /\ last' = "project" /\ UNCHANGED <<edited,chosen>>
RefuseEmpty == \E tab \in {"A","B"}: /\ target' = tab /\ before' = title /\ last' = "empty" /\ UNCHANGED <<title,edited,chosen>>
Next == Rename \/ Project \/ RefuseEmpty
EditedTitleRetained == \A tab \in edited: title[tab] = chosen[tab]
OtherTabUnchanged == last = "rename" => \A tab \in {"A","B"} \ {target}: title[tab] = before[tab]
EmptyNoRename == last = "empty" => title = before
Spec == Init /\ [][Next]_vars
====
