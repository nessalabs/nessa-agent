---------------- MODULE TunnelConnectivity ----------------
EXTENDS Naturals, FiniteSets

\* Bounded safety abstraction. See the owning page for source mapping and exclusions.
VARIABLES phase, nativeAdmitted, dispatched, draining, startedAfterDrain
vars == <<phase, nativeAdmitted, dispatched, draining, startedAfterDrain>>
Init == /\ phase = "Starting"
        /\ nativeAdmitted = FALSE
        /\ dispatched = FALSE
        /\ draining = FALSE
        /\ startedAfterDrain = FALSE

ProviderReady == /\ (phase = "Starting" /\ ~draining)
    /\ phase' = "Ready"
    /\ UNCHANGED <<nativeAdmitted, dispatched, draining, startedAfterDrain>>

ProviderFails == /\ (phase \in {"Starting", "Ready"} /\ ~draining)
    /\ phase' = "Failed"
    /\ UNCHANGED <<nativeAdmitted, dispatched, draining, startedAfterDrain>>

Retry == /\ (phase = "Failed" /\ ~draining)
    /\ phase' = "Starting"
    /\ startedAfterDrain' = draining
    /\ UNCHANGED <<nativeAdmitted, dispatched, draining>>

NativeAdmission == /\ (phase = "Ready" /\ ~draining /\ ~nativeAdmitted)
    /\ nativeAdmitted' = TRUE
    /\ UNCHANGED <<phase, dispatched, draining, startedAfterDrain>>

Dispatch == /\ (nativeAdmitted /\ ~dispatched)
    /\ dispatched' = TRUE
    /\ UNCHANGED <<phase, nativeAdmitted, draining, startedAfterDrain>>

Drain == /\ (~draining)
    /\ draining' = TRUE
    /\ phase' = "Stopped"
    /\ UNCHANGED <<nativeAdmitted, dispatched, startedAfterDrain>>

Next == ProviderReady \/ ProviderFails \/ Retry \/ NativeAdmission \/ Dispatch \/ Drain
Spec == Init /\ [][Next]_vars
TypeOK == /\ (phase \in {"Starting", "Ready", "Failed", "Stopped"})
    /\ (nativeAdmitted \in BOOLEAN)
    /\ (dispatched \in BOOLEAN)
    /\ (draining \in BOOLEAN)
    /\ (startedAfterDrain \in BOOLEAN)
ForwardingIsNotAuthority == (dispatched => nativeAdmitted)
NoRetryAfterDrain == (~startedAfterDrain)
DrainStopsPublication == (draining => phase = "Stopped")
============================================================================
