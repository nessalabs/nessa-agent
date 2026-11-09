---------------- MODULE AgentThreadTools ----------------
EXTENDS Naturals, FiniteSets

\* Bounded safety abstraction. See the owning page for source mapping and exclusions.
VARIABLES phase, slot, verified, allowed, audit, stopped, observer, effect, stoppedAtAdmission
vars == <<phase, slot, verified, allowed, audit, stopped, observer, effect, stoppedAtAdmission>>
Init == /\ phase = "Idle"
        /\ slot = FALSE
        /\ verified = FALSE
        /\ allowed = FALSE
        /\ audit = FALSE
        /\ stopped = FALSE
        /\ observer = TRUE
        /\ effect = FALSE
        /\ stoppedAtAdmission = FALSE

Reserve == /\ (phase = "Idle" /\ ~stopped)
    /\ phase' = "Resolving"
    /\ slot' = TRUE
    /\ UNCHANGED <<verified, allowed, audit, stopped, observer, effect, stoppedAtAdmission>>

Allow == /\ (phase = "Resolving")
    /\ phase' = "Decided"
    /\ verified' = TRUE
    /\ allowed' = TRUE
    /\ UNCHANGED <<slot, audit, stopped, observer, effect, stoppedAtAdmission>>

Deny == /\ (phase = "Resolving")
    /\ phase' = "Decided"
    /\ verified' = TRUE
    /\ UNCHANGED <<slot, allowed, audit, stopped, observer, effect, stoppedAtAdmission>>

Audit == /\ (phase = "Decided")
    /\ phase' = "Audited"
    /\ audit' = TRUE
    /\ UNCHANGED <<slot, verified, allowed, stopped, observer, effect, stoppedAtAdmission>>

AuditFails == /\ (phase = "Decided")
    /\ phase' = "Refused"
    /\ slot' = FALSE
    /\ UNCHANGED <<verified, allowed, audit, stopped, observer, effect, stoppedAtAdmission>>

Stop == /\ (~stopped)
    /\ stopped' = TRUE
    /\ UNCHANGED <<phase, slot, verified, allowed, audit, observer, effect, stoppedAtAdmission>>

Disconnect == /\ (observer)
    /\ observer' = FALSE
    /\ UNCHANGED <<phase, slot, verified, allowed, audit, stopped, effect, stoppedAtAdmission>>

Authorize == /\ (phase = "Audited" /\ slot /\ verified /\ allowed /\ audit /\ ~stopped)
    /\ phase' = "Authorized" /\ slot' = FALSE /\ stoppedAtAdmission' = stopped
    /\ UNCHANGED <<verified, allowed, audit, stopped, observer, effect>>
Dispatch == /\ phase = "Authorized"
    /\ phase' = "Dispatched" /\ effect' = TRUE
    /\ UNCHANGED <<slot, verified, allowed, audit, stopped, observer, stoppedAtAdmission>>

Refuse == /\ (phase = "Audited" /\ (~allowed \/ stopped))
    /\ phase' = "Refused"
    /\ slot' = FALSE
    /\ UNCHANGED <<verified, allowed, audit, stopped, observer, effect, stoppedAtAdmission>>

Finish == /\ (phase = "Dispatched")
    /\ phase' = "Done"
    /\ slot' = FALSE
    /\ UNCHANGED <<verified, allowed, audit, stopped, observer, effect, stoppedAtAdmission>>

Next == Reserve \/ Allow \/ Deny \/ Audit \/ AuditFails \/ Stop \/ Disconnect \/ Authorize \/ Dispatch \/ Refuse \/ Finish
Spec == Init /\ [][Next]_vars
TypeOK == /\ (phase \in {"Idle", "Resolving", "Decided", "Audited", "Authorized", "Dispatched", "Refused", "Done"})
    /\ (slot \in BOOLEAN)
    /\ (verified \in BOOLEAN)
    /\ (allowed \in BOOLEAN)
    /\ (audit \in BOOLEAN)
    /\ (stopped \in BOOLEAN)
    /\ (observer \in BOOLEAN)
    /\ (effect \in BOOLEAN)
    /\ (stoppedAtAdmission \in BOOLEAN)
EffectHasAuthority == (effect => (verified /\ allowed /\ audit))
NoCapabilityAfterObservedStop == (~stoppedAtAdmission)
CallerLossKeepsOwnedAdmission == (phase \in {"Resolving", "Decided", "Audited"} => slot)
SourcePermitEndsAtAuthorization == (phase \in {"Authorized", "Dispatched", "Done"} => ~slot)
============================================================================
