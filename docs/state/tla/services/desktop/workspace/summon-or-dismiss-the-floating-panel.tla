---- MODULE PanelToggle ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES shown, pressed, toggles, before, edge, reported
vars == <<shown, pressed, toggles, before, edge, reported>>
Init == /\ shown = FALSE /\ pressed = FALSE /\ toggles = 0 /\ before = 0 /\ edge = "none" /\ reported = "none"
Press == /\ ~pressed /\ toggles < 2 /\ pressed' = TRUE /\ shown' = ~shown /\ before' = toggles /\ toggles' = toggles + 1 /\ edge' = "press" /\ reported' = "attempted"
Repeat == /\ pressed /\ before' = toggles /\ edge' = "repeat" /\ UNCHANGED <<shown,pressed,toggles,reported>>
Release == /\ pressed /\ pressed' = FALSE /\ before' = toggles /\ edge' = "release" /\ UNCHANGED <<shown,toggles,reported>>
Next == Press \/ Repeat \/ Release
OnePressedEdge == edge = "repeat" => toggles = before
ReleaseNoToggle == edge = "release" => toggles = before
Spec == Init /\ [][Next]_vars
====
