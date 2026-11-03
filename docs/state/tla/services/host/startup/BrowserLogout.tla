---- MODULE BrowserLogout ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, local, pending, server
vars == <<phase, local, pending, server>>
Init == /\ phase = "SignedIn" /\ local = TRUE /\ pending = FALSE /\ server = TRUE
Dispose == /\ phase = "SignedIn" /\ local' = FALSE /\ phase' = "Logout" /\ UNCHANGED <<pending,server>>
Request == /\ phase = "Logout" /\ pending' = TRUE /\ phase' = "Waiting" /\ UNCHANGED <<local,server>>
Lost == /\ phase = "Waiting" /\ phase' = "Unconfirmed" /\ UNCHANGED <<local,pending,server>>
Retry == /\ phase = "Unconfirmed" /\ pending /\ phase' = "Waiting" /\ UNCHANGED <<local,pending,server>>
Acknowledge == /\ phase = "Waiting" /\ server' = FALSE /\ pending' = FALSE /\ phase' = "SignedOut" /\ UNCHANGED local
Next == Dispose \/ Request \/ Lost \/ Retry \/ Acknowledge
Spec == Init /\ [][Next]_vars
LocalTeardownFirst == pending => ~local
AcknowledgedLogout == phase = "SignedOut" => ~server /\ ~pending
====
