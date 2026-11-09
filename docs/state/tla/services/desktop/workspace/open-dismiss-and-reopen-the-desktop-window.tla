---- MODULE DesktopWindowOrdering ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES stage, dock, shown, focus, tray, hasDock, quit, hideSucceeded
vars == <<stage, dock, shown, focus, tray, hasDock, quit, hideSucceeded>>
Init == /\ stage = "Idle" /\ dock = FALSE /\ shown = FALSE /\ focus = FALSE /\ tray \in BOOLEAN /\ hasDock \in BOOLEAN /\ quit = FALSE /\ hideSucceeded = FALSE
Open == /\ stage = "Idle" /\ ~quit /\ dock' = TRUE /\ stage' = "Showing" /\ focus' = FALSE /\ UNCHANGED <<shown,tray,hasDock,quit,hideSucceeded>>
Show == /\ stage = "Showing" /\ dock /\ shown' = TRUE /\ stage' = "Focusing" /\ UNCHANGED <<dock,focus,tray,hasDock,quit,hideSucceeded>>
ShowFails == /\ stage = "Showing" /\ stage' = "Idle" /\ UNCHANGED <<dock,shown,focus,tray,hasDock,quit,hideSucceeded>>
Focus == /\ stage = "Focusing" /\ dock /\ shown /\ focus' = TRUE /\ stage' = "Idle" /\ UNCHANGED <<dock,shown,tray,hasDock,quit,hideSucceeded>>
Close == /\ stage = "Idle" /\ ~quit /\ focus' = FALSE /\ stage' = (IF tray \/ hasDock THEN "Hiding" ELSE "Quit") /\ quit' = ~(tray \/ hasDock) /\ UNCHANGED <<dock,shown,tray,hasDock,hideSucceeded>>
Hide == /\ stage = "Hiding" /\ shown' = FALSE /\ hideSucceeded' = TRUE /\ stage' = (IF tray THEN "RemoveDock" ELSE "Idle") /\ UNCHANGED <<dock,focus,tray,hasDock,quit>>
HideFails == /\ stage = "Hiding" /\ hideSucceeded' = FALSE /\ stage' = "Idle" /\ UNCHANGED <<dock,shown,focus,tray,hasDock,quit>>
RemoveDock == /\ stage = "RemoveDock" /\ hideSucceeded /\ ~shown /\ dock' = FALSE /\ stage' = "Idle" /\ UNCHANGED <<shown,focus,tray,hasDock,quit,hideSucceeded>>
Next == Open \/ Show \/ ShowFails \/ Focus \/ Close \/ Hide \/ HideFails \/ RemoveDock
DockBeforeFocus == focus => dock /\ shown
ReopenOrQuit == quit => ~tray /\ ~hasDock
DockRemovalAfterHide == stage = "RemoveDock" => hideSucceeded /\ ~shown
DockBeforeShow == stage \in {"Showing","Focusing"} => dock
Spec == Init /\ [][Next]_vars
====
