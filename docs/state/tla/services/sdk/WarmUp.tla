---------------- MODULE WarmUp ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES phase, started, released, audit, record
vars == <<phase, started, released, audit, record>>
Init == phase = "Observing" /\ started = FALSE /\ released = FALSE /\ audit = FALSE /\ record = FALSE
Start == phase = "Observing" /\ ~started /\ phase' = "Opening" /\ started' = TRUE /\ UNCHANGED <<released, audit, record>>
OpenSettles == phase = "Opening" /\ phase' = "Closing" /\ UNCHANGED <<started, released, audit, record>>
Release == phase = "Closing" /\ phase' = "Released" /\ released' = TRUE /\ UNCHANGED <<started, audit, record>>
Retain == phase = "Closing" /\ phase' = "Retained" /\ UNCHANGED <<started, released, audit, record>>
Audit == phase = "Released" /\ audit' = TRUE /\ UNCHANGED <<phase, started, released, record>>
Record == phase = "Released" /\ audit /\ released /\ phase' = "Warmed" /\ record' = TRUE /\ UNCHANGED <<started, released, audit>>
RecordFail == phase = "Released" /\ phase' = "StillCold" /\ UNCHANGED <<started, released, audit, record>>
Next == Start \/ OpenSettles \/ Release \/ Retain \/ Audit \/ Record \/ RecordFail
Spec == Init /\ [][Next]_vars
TypeOK == phase \in {"Observing", "Opening", "Closing", "Released", "Retained", "Warmed", "StillCold"} /\ started \in BOOLEAN /\ released \in BOOLEAN /\ audit \in BOOLEAN /\ record \in BOOLEAN
CompletionNeedsRelease == record => released /\ audit
RetainedNotPrepared == phase = "Retained" => ~record /\ ~released
=============================================================================
