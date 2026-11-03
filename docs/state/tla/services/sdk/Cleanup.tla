----------------------------- MODULE Cleanup -----------------------------
\* One stop and possible reopening. Bounded to one active old work owner.
\* Physical release and audit acknowledgement may arrive in either order.
VARIABLES phase, released, audit, retired, finalized, reopened
vars == <<phase, released, audit, retired, finalized, reopened>>
Init == /\ phase = "Open" /\ released = FALSE /\ audit = "Pending"
        /\ retired = FALSE /\ finalized = FALSE /\ reopened = FALSE
Stop == /\ phase = "Open" /\ ~reopened /\ phase' = "Stopping"
        /\ UNCHANGED <<released, audit, retired, finalized, reopened>>
Release == /\ phase = "Stopping" /\ ~released /\ released' = TRUE
           /\ UNCHANGED <<phase, audit, retired, finalized, reopened>>
AuditAck == /\ phase = "Stopping" /\ audit = "Pending" /\ audit' = "Acknowledged"
            /\ UNCHANGED <<phase, released, retired, finalized, reopened>>
AuditFail == /\ phase = "Stopping" /\ audit = "Pending" /\ audit' = "Failed"
             /\ UNCHANGED <<phase, released, retired, finalized, reopened>>
Retire == /\ phase = "Stopping" /\ ~retired /\ retired' = TRUE
          /\ UNCHANGED <<phase, released, audit, finalized, reopened>>
Finalize == /\ phase = "Stopping" /\ released /\ audit # "Pending"
            /\ ~finalized /\ finalized' = TRUE
            /\ UNCHANGED <<phase, released, audit, retired, reopened>>
Reopen == /\ phase = "Stopping" /\ released /\ audit = "Acknowledged"
          /\ retired /\ finalized /\ phase' = "Open" /\ reopened' = TRUE
          /\ UNCHANGED <<released, audit, retired, finalized>>
Next == Stop \/ Release \/ AuditAck \/ AuditFail \/ Retire \/ Finalize \/ Reopen
Spec == Init /\ [][Next]_vars
TypeOK == /\ phase \in {"Open", "Stopping"} /\ audit \in {"Pending", "Acknowledged", "Failed"}
          /\ released \in BOOLEAN /\ retired \in BOOLEAN /\ finalized \in BOOLEAN /\ reopened \in BOOLEAN
ReopenRequiresRelease == reopened => released
ReopenRequiresAudit == reopened => audit = "Acknowledged"
ReopenRequiresRetirement == reopened => retired
ReopenRequiresFinalization == reopened => finalized
=============================================================================
