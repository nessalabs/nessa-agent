-------------------------- MODULE CloudBoxSleep --------------------------
EXTENDS TLC
VARIABLES phase, work, evidence, stop, accepted, stopped, receipt
vars == <<phase, work, evidence, stop, accepted, stopped, receipt>>

Init == /\ phase = "Probe" /\ work \in BOOLEAN
        /\ evidence = "None" /\ stop = "None" /\ accepted = FALSE
        /\ stopped = FALSE /\ receipt = "Running"

\* One box, one sleep caller. Gateway admission can run independently of
\* the per-box application lock. Preserve work at stop to witness overlap.
Activity == /\ ~stopped /\ work' = ~work
            /\ UNCHANGED <<phase, evidence, stop, accepted, stopped, receipt>>

Observe == /\ phase = "Probe"
           /\ evidence' \in {"Unknown", IF work THEN "Busy" ELSE "Idle"}
           /\ phase' = IF evidence' = "Idle" THEN "Requesting" ELSE "Refused"
           /\ UNCHANGED <<work, stop, accepted, stopped, receipt>>

RequestStop == /\ phase = "Requesting" /\ phase' = "AwaitingReply"
               /\ stop' = "Pending"
               /\ UNCHANGED <<work, evidence, accepted, stopped, receipt>>

ProviderAccepted == /\ phase = "AwaitingReply" /\ phase' = "Recording"
                    /\ accepted' = TRUE
                    /\ UNCHANGED <<work, evidence, stop, stopped, receipt>>

ProviderError == /\ phase = "AwaitingReply" /\ phase' = "Failed"
                 /\ stop' \in (IF stop = "Pending" THEN {"Pending", "Refused"} ELSE {stop})
                 /\ UNCHANGED <<work, evidence, accepted, stopped, receipt>>

StopEffect == /\ stop = "Pending" /\ stop' = "Settled" /\ stopped' = TRUE
              /\ UNCHANGED <<phase, work, evidence, accepted, receipt>>

Persist == /\ phase = "Recording"
           /\ receipt' \in {"Running", "Sleeping"}
           /\ phase' = IF receipt' = "Sleeping" THEN "Auditing" ELSE "Unrecorded"
           /\ UNCHANGED <<work, evidence, stop, accepted, stopped>>

Audit == /\ phase = "Auditing" /\ phase' \in {"Done", "AuditFailed"}
         /\ UNCHANGED <<work, evidence, stop, accepted, stopped, receipt>>

Wake == /\ phase \in {"Failed", "Unrecorded", "AuditFailed", "Done"}
        /\ phase' = "Woke" /\ stopped' = FALSE /\ receipt' = "Running"
        /\ work' = IF stopped THEN FALSE ELSE work
        /\ UNCHANGED <<evidence, stop, accepted>>

Next == Activity \/ Observe \/ RequestStop \/ ProviderAccepted \/ ProviderError
        \/ StopEffect \/ Persist \/ Audit \/ Wake
Spec == Init /\ [][Next]_vars
IdleEvidenceBeforeStop == stop # "None" => evidence = "Idle"
AcknowledgementBeforeReceipt == receipt = "Sleeping" => accepted
SuccessRequiresReceipt == phase = "Done" => receipt = "Sleeping"

\* Desired product guarantees. Check separately to reproduce current gaps.
NoWorkAtStop == stopped => ~work
PhysicalStopBeforeReceipt == receipt = "Sleeping" => stopped
NoEarlierStopAfterWake == phase = "Woke" => ~stopped
=============================================================================
