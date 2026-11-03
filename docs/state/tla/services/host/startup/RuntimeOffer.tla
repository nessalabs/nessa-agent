---- MODULE RuntimeOffer ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, read, floor, offers, pending
vars == <<phase, read, floor, offers, pending>>
Init == /\ phase = "Mounted" /\ read = 0 /\ floor = 0 /\ offers = FALSE /\ pending = FALSE
Read == /\ phase = "Mounted" /\ read < 2 /\ read' = read + 1 /\ UNCHANGED <<phase,floor,offers,pending>>
Offers == /\ phase = "Mounted" /\ read > floor /\ offers' = TRUE /\ UNCHANGED <<phase,read,floor,pending>>
Install == /\ phase = "Mounted" /\ ~pending /\ pending' = TRUE /\ UNCHANGED <<phase,read,floor,offers>>
Uncertain == /\ pending /\ phase = "Mounted" /\ pending' = FALSE /\ offers' = FALSE /\ floor' = read /\ UNCHANGED <<phase,read>>
Dispose == /\ phase = "Mounted" /\ phase' = "Disposed" /\ offers' = FALSE /\ pending' = FALSE /\ UNCHANGED <<read,floor>>
Next == Read \/ Offers \/ Install \/ Uncertain \/ Dispose
Spec == Init /\ [][Next]_vars
FreshOffers == offers => read > floor
NoDisposedPublication == phase = "Disposed" => ~offers /\ ~pending
====
