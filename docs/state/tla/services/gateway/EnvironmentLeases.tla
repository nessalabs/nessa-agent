---------------- MODULE EnvironmentLeases ----------------
EXTENDS Naturals, FiniteSets

\* Bounded safety abstraction. See the owning page for source mapping and exclusions.
VARIABLES phase, readableGrant, allowed, held, observer, released, askedTimeout, grantTimeout, effectiveTimeout
vars == <<phase, readableGrant, allowed, held, observer, released, askedTimeout, grantTimeout, effectiveTimeout>>
Init == /\ phase = "Idle"
        /\ readableGrant = FALSE
        /\ allowed = FALSE
        /\ held = FALSE
        /\ observer = TRUE
        /\ released = FALSE
        /\ askedTimeout \in 1..2
        /\ grantTimeout \in 1..2
        /\ effectiveTimeout = 0

ReadGrant == /\ (phase = "Idle")
    /\ phase' = "ReadGrant"
    /\ readableGrant' = TRUE
    /\ UNCHANGED <<allowed, held, observer, released, askedTimeout, grantTimeout, effectiveTimeout>>

DenyUnreadable == /\ (phase = "Idle")
    /\ phase' = "Refused"
    /\ UNCHANGED <<readableGrant, allowed, held, observer, released, askedTimeout, grantTimeout, effectiveTimeout>>

Admit == /\ (phase = "ReadGrant" /\ readableGrant)
    /\ phase' = "Admitted"
    /\ allowed' = TRUE
    /\ held' = TRUE
    /\ effectiveTimeout' = (IF askedTimeout <= grantTimeout THEN askedTimeout ELSE grantTimeout)
    /\ UNCHANGED <<readableGrant, observer, released, askedTimeout, grantTimeout>>

Deny == /\ (phase = "ReadGrant")
    /\ phase' = "Refused"
    /\ UNCHANGED <<readableGrant, allowed, held, observer, released, askedTimeout, grantTimeout, effectiveTimeout>>

Launch == /\ (phase = "Admitted" /\ held /\ allowed)
    /\ phase' = "Running"
    /\ UNCHANGED <<readableGrant, allowed, held, observer, released, askedTimeout, grantTimeout, effectiveTimeout>>

LoseObserver == /\ (observer)
    /\ observer' = FALSE
    /\ UNCHANGED <<phase, readableGrant, allowed, held, released, askedTimeout, grantTimeout, effectiveTimeout>>

Stop == /\ (phase \in {"Admitted", "Running"})
    /\ phase' = "Stopping"
    /\ UNCHANGED <<readableGrant, allowed, held, observer, released, askedTimeout, grantTimeout, effectiveTimeout>>

Uncertain == /\ (phase = "Stopping")
    /\ phase' = "Uncertain"
    /\ UNCHANGED <<readableGrant, allowed, held, observer, released, askedTimeout, grantTimeout, effectiveTimeout>>

Retry == /\ (phase = "Uncertain")
    /\ phase' = "Stopping"
    /\ UNCHANGED <<readableGrant, allowed, held, observer, released, askedTimeout, grantTimeout, effectiveTimeout>>

Release == /\ (phase = "Stopping" /\ held)
    /\ phase' = "Released"
    /\ released' = TRUE
    /\ held' = FALSE
    /\ UNCHANGED <<readableGrant, allowed, observer, askedTimeout, grantTimeout, effectiveTimeout>>

Next == ReadGrant \/ DenyUnreadable \/ Admit \/ Deny \/ Launch \/ LoseObserver \/ Stop \/ Uncertain \/ Retry \/ Release
Spec == Init /\ [][Next]_vars
TypeOK == /\ (phase \in {"Idle", "ReadGrant", "Admitted", "Running", "Stopping", "Uncertain", "Released", "Refused"})
    /\ (readableGrant \in BOOLEAN)
    /\ (allowed \in BOOLEAN)
    /\ (held \in BOOLEAN)
    /\ (observer \in BOOLEAN)
    /\ (released \in BOOLEAN)
    /\ (askedTimeout \in 1..2)
    /\ (grantTimeout \in 1..2)
    /\ (effectiveTimeout \in 0..2)
RunningNeedsGrant == (phase = "Running" => (readableGrant /\ allowed /\ held))
TermsNeverWiden == (effectiveTimeout <= askedTimeout /\ effectiveTimeout <= grantTimeout)
UncertainKeepsOwner == (phase = "Uncertain" => (held /\ ~released))
ReleasedIsConfirmed == (~held /\ allowed => released)
============================================================================
