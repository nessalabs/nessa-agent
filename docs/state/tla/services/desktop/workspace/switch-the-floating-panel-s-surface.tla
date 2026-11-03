---- MODULE SurfaceSubscription ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES live, subscription, surface, stored, before, event, invoked
vars == <<live, subscription, surface, stored, before, event, invoked>>
Init == /\ live = TRUE /\ subscription = "Pending" /\ surface = "translucent" /\ stored = "translucent" /\ before = "translucent" /\ event = "init" /\ invoked = FALSE
ListenReady == /\ subscription = "Pending" /\ subscription' = (IF live THEN "Attached" ELSE "Removed") /\ event' = "ready" /\ UNCHANGED <<live,surface,stored,before,invoked>>
Toggle == \E writes \in BOOLEAN: /\ live /\ subscription # "Removed" /\ before' = surface /\ surface' = (IF surface = "clear" THEN "translucent" ELSE "clear") /\ stored' = (IF writes THEN (IF surface = "clear" THEN "translucent" ELSE "clear") ELSE stored) /\ event' = "toggle" /\ invoked' = TRUE /\ UNCHANGED <<live,subscription>>
Retire == /\ live /\ live' = FALSE /\ subscription' = (IF subscription = "Attached" THEN "Removed" ELSE subscription) /\ event' = "retire" /\ invoked' = FALSE /\ UNCHANGED <<surface,stored,before>>
LateEvent == /\ ~live /\ before' = surface /\ event' = "late" /\ invoked' = FALSE /\ UNCHANGED <<live,subscription,surface,stored>>
Next == ListenReady \/ Toggle \/ Retire \/ LateEvent
LateListenerNoToggle == event = "late" => surface = before
NoRetiredInvocation == ~live => ~invoked
ResolvedRetiredRemoved == (~live /\ event = "ready") => subscription = "Removed"
Spec == Init /\ [][Next]_vars
====
