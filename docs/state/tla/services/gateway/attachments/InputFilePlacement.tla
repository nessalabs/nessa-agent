---- MODULE InputFilePlacement ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, asked, delivered, waiter, outcome, lease
vars == <<phase, asked, delivered, waiter, outcome, lease>>
Init == /\ phase = "Submitted" /\ asked = FALSE /\ delivered = FALSE /\ waiter = TRUE /\ outcome = FALSE /\ lease = 1
Ask == /\ phase = "Submitted" /\ phase' = "Placing" /\ asked' = TRUE /\ UNCHANGED <<delivered,waiter,outcome,lease>>
Detach == /\ waiter /\ waiter' = FALSE /\ UNCHANGED <<phase,asked,delivered,outcome,lease>>
Place == /\ phase = "Placing" /\ asked /\ phase' = "Observed" /\ delivered' \in BOOLEAN /\ UNCHANGED <<asked,waiter,outcome,lease>>
Record == /\ phase = "Observed" /\ outcome' = TRUE /\ phase' = "Done" /\ UNCHANGED <<asked,delivered,waiter,lease>>
Next == Ask \/ Detach \/ Place \/ Record
Spec == Init /\ [][Next]_vars
AskBeforeEffect == delivered => asked
LeaseIdentity == lease = 1
OutcomeBeforeDone == phase = "Done" => outcome
====
