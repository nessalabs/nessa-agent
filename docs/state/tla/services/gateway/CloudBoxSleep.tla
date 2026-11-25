-------------------------- MODULE CloudBoxSleep --------------------------
EXTENDS TLC
VARIABLES phase, work, evidence, stopped, receipt
vars == <<phase, work, evidence, stopped, receipt>>

Init == /\ phase = "Probe" /\ work \in BOOLEAN
        /\ evidence = "None" /\ stopped = FALSE /\ receipt = "Running"

\* One box, one sleep caller. Gateway admission can run independently of
\* the per-box application lock. Preserve work at stop to witness overlap.
Activity == /\ ~stopped /\ work' = ~work
            /\ UNCHANGED <<phase, evidence, stopped, receipt>>

Observe == /\ phase = "Probe"
           /\ evidence' \in {"Unknown", IF work THEN "Busy" ELSE "Idle"}
           /\ phase' = IF evidence' = "Idle" THEN "Stopping" ELSE "Refused"
           /\ UNCHANGED <<work, stopped, receipt>>

ProviderReply == /\ phase = "Stopping"
                 /\ stopped' \in BOOLEAN
                 /\ phase' = IF stopped' THEN "Recording" ELSE "Failed"
                 /\ UNCHANGED <<work, evidence, receipt>>

Persist == /\ phase = "Recording"
           /\ receipt' \in {"Running", "Sleeping"}
           /\ phase' = IF receipt' = "Sleeping" THEN "Auditing" ELSE "Unrecorded"
           /\ UNCHANGED <<work, evidence, stopped>>

Audit == /\ phase = "Auditing" /\ phase' \in {"Done", "AuditFailed"}
         /\ UNCHANGED <<work, evidence, stopped, receipt>>

Next == Activity \/ Observe \/ ProviderReply \/ Persist \/ Audit
Spec == Init /\ [][Next]_vars
IdleEvidenceBeforeStop == stopped => evidence = "Idle"
EffectBeforeReceipt == receipt = "Sleeping" => stopped
SuccessRequiresReceipt == phase = "Done" => receipt = "Sleeping"

\* Stronger product goal. Check separately to see the admission window.
NoWorkAtStop == stopped => ~work
=============================================================================
