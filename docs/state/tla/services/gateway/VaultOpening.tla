---- MODULE VaultOpening ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, ownerLive, generation, captured, key, published
vars == <<phase, ownerLive, generation, captured, key, published>>
Init == /\ phase = "Locked" /\ ownerLive = TRUE /\ generation = 0 /\ captured = 0 /\ key = FALSE /\ published = FALSE
Begin == /\ phase = "Locked" /\ ownerLive /\ phase' = "Deriving" /\ captured' = generation /\ UNCHANGED <<ownerLive,generation,key,published>>
Derive == /\ phase = "Deriving" /\ phase' = "Derived" /\ key' \in BOOLEAN /\ UNCHANGED <<ownerLive,generation,captured,published>>
Close == /\ ownerLive /\ ownerLive' = FALSE /\ published' = FALSE /\ UNCHANGED <<phase,generation,captured,key>>
Lock == /\ generation = 0 /\ generation' = 1 /\ published' = FALSE /\ UNCHANGED <<phase,ownerLive,captured,key>>
Publish == /\ phase = "Derived" /\ key /\ ownerLive /\ generation = captured /\ published' = TRUE /\ phase' = "Unlocked" /\ UNCHANGED <<ownerLive,generation,captured,key>>
Next == Begin \/ Derive \/ Close \/ Lock \/ Publish
Spec == Init /\ [][Next]_vars
LiveOwner == published => ownerLive
GenerationFence == published => generation = captured
VerifiedKey == published => key
====
