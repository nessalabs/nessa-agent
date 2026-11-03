---- MODULE CliTokenLifetime ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, callerExpiry, requested, chosen, issued
vars == <<phase, callerExpiry, requested, chosen, issued>>
Init == /\ phase = "Validate" /\ callerExpiry \in 1..3 /\ requested \in 1..3 /\ chosen = 0 /\ issued = FALSE
Validate == /\ phase = "Validate" /\ chosen' = (IF requested < callerExpiry THEN requested ELSE callerExpiry) /\ phase' = "Issue" /\ UNCHANGED <<callerExpiry,requested,issued>>
Issue == /\ phase = "Issue" /\ chosen > 1 /\ issued' = TRUE /\ phase' = "Done" /\ UNCHANGED <<callerExpiry,requested,chosen>>
Next == Validate \/ Issue
Spec == Init /\ [][Next]_vars
LifetimeCapped == chosen <= callerExpiry /\ chosen <= requested
NoExpiredIssue == issued => chosen > 1
====
