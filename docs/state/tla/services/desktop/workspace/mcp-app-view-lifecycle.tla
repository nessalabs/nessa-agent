---- MODULE AppMount ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES phase, place, released, told
vars == <<phase, place, released, told>>
Init == /\ phase = "Reading" /\ place \in {"inline", "panel"} /\ released = FALSE /\ told = FALSE
Read == /\ phase = "Reading" /\ phase' = "Proxy" /\ UNCHANGED <<place,released,told>>
ProxyReady == /\ phase = "Proxy" /\ phase' = "Loading" /\ UNCHANGED <<place,released,told>>
Initialize == /\ phase = "Loading" /\ phase' = "Initializing" /\ UNCHANGED <<place,released,told>>
Initialized == /\ phase = "Initializing" /\ phase' = "Live" /\ told' = TRUE /\ UNCHANGED <<place,released>>
Teardown == /\ phase = "Live" /\ place # "inline" /\ phase' = "Ending" /\ UNCHANGED <<place,released,told>>
Close == /\ phase = "Ending" /\ phase' = "Gone" /\ released' = TRUE /\ UNCHANGED <<place,told>>
Failure == /\ phase \in {"Reading","Proxy","Loading","Initializing","Live","Ending"} /\ phase' = "Failed" /\ released' = TRUE /\ UNCHANGED <<place,told>>
Remove == /\ phase # "Gone" /\ phase' = "Gone" /\ released' = TRUE /\ UNCHANGED <<place,told>>
Next == Read \/ ProxyReady \/ Initialize \/ Initialized \/ Teardown \/ Close \/ Failure \/ Remove
TerminalReleased == phase \in {"Gone","Failed"} => released
NoEarlyTell == told => phase \in {"Live","Ending","Failed","Gone"}
InlineCannotTeardown == place = "inline" => phase # "Ending"
Spec == Init /\ [][Next]_vars
====
