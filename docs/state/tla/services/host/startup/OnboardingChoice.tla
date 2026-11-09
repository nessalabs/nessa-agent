---- MODULE OnboardingChoice ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, ready, chosen, outcome
vars == <<phase, ready, chosen, outcome>>
Init == /\ phase = "Welcome" /\ ready = FALSE /\ chosen = FALSE /\ outcome = "None"
Report == /\ phase \in {"Welcome","Agent"} /\ ready' \in BOOLEAN /\ chosen' = (chosen /\ ready') /\ UNCHANGED <<phase,outcome>>
Picker == /\ phase = "Welcome" /\ phase' = "Agent" /\ UNCHANGED <<ready,chosen,outcome>>
Choose == /\ phase = "Agent" /\ ready /\ chosen' = TRUE /\ UNCHANGED <<phase,ready,outcome>>
Confirm == /\ phase = "Agent" /\ chosen /\ phase' = "Summon" /\ UNCHANGED <<ready,chosen,outcome>>
Complete == /\ phase = "Summon" /\ phase' = "Done" /\ outcome' = "Completed" /\ UNCHANGED <<ready,chosen>>
Dismiss == /\ phase # "Done" /\ phase' = "Done" /\ outcome' = "Dismissed" /\ chosen' = FALSE /\ UNCHANGED ready
Next == Report \/ Picker \/ Choose \/ Confirm \/ Complete \/ Dismiss
Spec == Init /\ [][Next]_vars
PickerHasReadyChoice == phase \in {"Welcome","Agent"} /\ chosen => ready
CompletionHasChoice == outcome = "Completed" => chosen
====
