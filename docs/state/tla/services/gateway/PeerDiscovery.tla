---------------- MODULE PeerDiscovery ----------------
EXTENDS Naturals
VARIABLES field, phase, credential, announced
vars == <<field, phase, credential, announced>>

Init == field = "none" /\ phase = "Active" /\ credential = "current" /\ announced = FALSE

Key == /\ field = "none" /\ phase = "Active"
       /\ field' = "keyed"
       /\ UNCHANGED <<phase, credential, announced>>

Offer == /\ field = "keyed" /\ phase = "Active"
         /\ field' = "offered"
         /\ UNCHANGED <<phase, credential, announced>>

Install == /\ field = "offered" /\ phase = "Active"
           /\ field' = "installed"
           /\ UNCHANGED <<phase, credential, announced>>

Expire == /\ credential' = "expired"
          /\ UNCHANGED <<field, phase, announced>>

Announce == /\ announced' = (field = "installed" /\ phase = "Active" /\ credential = "current")
            /\ UNCHANGED <<field, phase, credential>>

Quiet == /\ announced' = FALSE
         /\ UNCHANGED <<field, phase, credential>>

Next == Key \/ Offer \/ Install \/ Expire \/ Announce \/ Quiet
Spec == Init /\ [][Next]_vars

TypeOK == field \in {"none", "keyed", "offered", "installed"}
          /\ phase = "Active"
          /\ credential \in {"current", "expired"}
          /\ announced \in BOOLEAN

AnnounceOnlyInstalled == announced => (field = "installed" /\ phase = "Active" /\ credential = "current")
ExpiryKeepsPhase == phase = "Active"
=============================================================================
