---------------- MODULE Ownership ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES sealed, reserved, started, released, audit, complete, published, durable
vars == <<sealed, reserved, started, released, audit, complete, published, durable>>
Init == sealed = FALSE /\ reserved = FALSE /\ started = FALSE /\ released = FALSE /\ audit = FALSE /\ complete = FALSE /\ published = FALSE /\ durable = FALSE
Reserve == ~sealed /\ ~reserved /\ reserved' = TRUE /\ UNCHANGED <<sealed, started, released, audit, complete, published, durable>>
Start == reserved /\ ~sealed /\ ~started /\ started' = TRUE /\ UNCHANGED <<sealed, reserved, released, audit, complete, published, durable>>
Seal == ~sealed /\ sealed' = TRUE /\ UNCHANGED <<reserved, started, released, audit, complete, published, durable>>
Release == sealed /\ reserved /\ released' = TRUE /\ UNCHANGED <<sealed, reserved, started, audit, complete, published, durable>>
ProveAbsence == sealed /\ ~reserved /\ released' = TRUE /\ UNCHANGED <<sealed, reserved, started, audit, complete, published, durable>>
Evidence == sealed /\ audit' = TRUE /\ UNCHANGED <<sealed, reserved, started, released, complete, published, durable>>
Complete == sealed /\ released /\ audit /\ durable /\ complete' = TRUE /\ UNCHANGED <<sealed, reserved, started, released, audit, published, durable>>
Publish == complete /\ published' = TRUE /\ UNCHANGED <<sealed, reserved, started, released, audit, complete, durable>>
Persist == sealed /\ audit /\ durable' = TRUE /\ UNCHANGED <<sealed, reserved, started, released, audit, complete, published>>
Next == Persist \/ Reserve \/ Start \/ Seal \/ Release \/ ProveAbsence \/ Evidence \/ Complete \/ Publish
Spec == Init /\ [][Next]_vars
TypeOK == sealed \in BOOLEAN /\ reserved \in BOOLEAN /\ started \in BOOLEAN /\ released \in BOOLEAN /\ audit \in BOOLEAN /\ complete \in BOOLEAN /\ published \in BOOLEAN /\ durable \in BOOLEAN
NoUnreservedStart == started => reserved
CompletionNeedsRelease == complete => sealed /\ released /\ audit /\ durable
PublicationNeedsCompletion == published => complete
=============================================================================
