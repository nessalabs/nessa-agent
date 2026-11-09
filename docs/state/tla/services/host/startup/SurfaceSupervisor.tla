---- MODULE SurfaceSupervisor ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, generation, captured, handle, disposed
vars == <<phase, generation, captured, handle, disposed>>
Init == /\ phase = "Opening" /\ generation = 0 /\ captured = 0 /\ handle = FALSE /\ disposed = FALSE
Connected == /\ phase = "Opening" /\ ~disposed /\ generation = captured /\ phase' = "Owned" /\ handle' = TRUE /\ UNCHANGED <<generation,captured,disposed>>
Reconnecting == /\ phase = "Owned" /\ phase' = "Reconnecting" /\ handle' = FALSE /\ UNCHANGED <<generation,captured,disposed>>
Reconnected == /\ phase = "Reconnecting" /\ ~disposed /\ generation = captured /\ phase' = "Owned" /\ handle' = TRUE /\ UNCHANGED <<generation,captured,disposed>>
Failure == /\ phase \in {"Opening","Owned","Reconnecting"} /\ phase' \in {"Backoff","Terminal"} /\ handle' = FALSE /\ generation' = 1 /\ UNCHANGED <<captured,disposed>>
Dispose == /\ ~disposed /\ disposed' = TRUE /\ phase' = "Disposed" /\ generation' = 1 /\ handle' = FALSE /\ UNCHANGED captured
Next == Connected \/ Reconnecting \/ Reconnected \/ Failure \/ Dispose
Spec == Init /\ [][Next]_vars
PublishedOnlyOwned == handle => phase = "Owned" /\ ~disposed /\ generation = captured
RetiredHandleCleared == disposed => ~handle
====
