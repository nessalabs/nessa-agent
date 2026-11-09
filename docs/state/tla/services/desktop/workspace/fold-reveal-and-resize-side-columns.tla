---- MODULE ColumnChoice ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES wanted, folded, room, last, beforeWanted
vars == <<wanted, folded, room, last, beforeWanted>>
Init == /\ wanted \in BOOLEAN /\ folded = FALSE /\ room = TRUE /\ last = "init" /\ beforeWanted = wanted
Choose == \E choice \in BOOLEAN: /\ wanted' = choice /\ folded' = FALSE /\ last' = "choose" /\ beforeWanted' = wanted /\ UNCHANGED room
Fit == \E fits \in BOOLEAN: /\ beforeWanted' = wanted /\ folded' = (wanted /\ ~fits) /\ room' = fits /\ last' = "fit" /\ UNCHANGED wanted
FitRoom == /\ beforeWanted' = wanted /\ folded' = FALSE /\ room' = TRUE /\ last' = "fit" /\ UNCHANGED wanted
Next == Choose \/ Fit \/ FitRoom
HiddenStaysHidden == ~wanted => ~folded
FitPreservesChoice == last = "fit" => wanted = beforeWanted
ManualChoiceImmediate == last = "choose" => ~folded
RoomRestores == (last = "fit" /\ room) => ~folded
Spec == Init /\ [][Next]_vars
====
