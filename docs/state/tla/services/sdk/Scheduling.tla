---------------- MODULE Scheduling ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES phase, saved, retired, fallback, dispatches, receipt, retryIdentity, recovered
vars == <<phase, saved, retired, fallback, dispatches, receipt, retryIdentity, recovered>>
Init == phase = "Fresh" /\ saved = FALSE /\ retired = FALSE /\ fallback = FALSE /\ dispatches = 0 /\ receipt = FALSE /\ retryIdentity = "None" /\ recovered = FALSE
Admit == phase = "Fresh" /\ phase' = "Queued" /\ saved' = FALSE /\ UNCHANGED <<retired, fallback, dispatches, receipt, retryIdentity, recovered>>
Steer == phase = "Fresh" /\ phase' = "Native" /\ saved' = FALSE /\ UNCHANGED <<retired, fallback, dispatches, receipt, retryIdentity, recovered>>
Timeout == phase = "Native" /\ phase' = "Unknown" /\ UNCHANGED <<saved, retired, fallback, dispatches, receipt, retryIdentity, recovered>>
NonConsumption == phase = "Native" /\ phase' = "NotConsumed" /\ UNCHANGED <<saved, retired, fallback, dispatches, receipt, retryIdentity, recovered>>
Fallback == phase = "NotConsumed" /\ phase' = "Queued" /\ fallback' = TRUE /\ UNCHANGED <<saved, retired, dispatches, receipt, retryIdentity, recovered>>
Dispatch == phase = "Queued" /\ saved /\ phase' = "Running" /\ dispatches' = 1 /\ UNCHANGED <<saved, retired, fallback, receipt, retryIdentity, recovered>>
Finish == phase = "Running" /\ phase' = "Settled" /\ UNCHANGED <<saved, retired, fallback, dispatches, receipt, retryIdentity, recovered>>
Retire == phase = "Settled" /\ retired' = TRUE /\ UNCHANGED <<phase, saved, fallback, dispatches, receipt, retryIdentity, recovered>>
PublishReceipt == phase = "Settled" /\ retired /\ receipt' = TRUE /\ UNCHANGED <<phase, saved, retired, fallback, dispatches, retryIdentity, recovered>>
SaveAck == phase \in {"Queued", "Native", "NotConsumed", "Unknown"} /\ saved' = TRUE /\ UNCHANGED <<phase, retired, fallback, dispatches, receipt, retryIdentity, recovered>>
NativeAck == phase = "Native" /\ phase' = "Injected" /\ UNCHANGED <<saved, retired, fallback, dispatches, receipt, retryIdentity, recovered>>
RetryExact == phase # "Fresh" /\ retryIdentity = "None" /\ retryIdentity' = "Exact" /\ UNCHANGED <<phase, saved, retired, fallback, dispatches, receipt, recovered>>
RetryConflict == phase # "Fresh" /\ retryIdentity = "None" /\ retryIdentity' = "Conflict" /\ UNCHANGED <<phase, saved, retired, fallback, dispatches, receipt, recovered>>
RecoverRetry == retryIdentity = "Exact" /\ recovered' = TRUE /\ UNCHANGED <<phase, saved, retired, fallback, dispatches, receipt, retryIdentity>>
Next == NativeAck \/ RetryExact \/ RetryConflict \/ RecoverRetry \/ Finish \/ SaveAck \/ Admit \/ Steer \/ Timeout \/ NonConsumption \/ Fallback \/ Dispatch \/ Retire \/ PublishReceipt
Spec == Init /\ [][Next]_vars
TypeOK == phase \in {"Fresh", "Native", "Injected", "Unknown", "NotConsumed", "Queued", "Running", "Settled"} /\ saved \in BOOLEAN /\ retired \in BOOLEAN /\ fallback \in BOOLEAN /\ dispatches \in 0..1 /\ receipt \in BOOLEAN /\ retryIdentity \in {"None", "Exact", "Conflict"} /\ recovered \in BOOLEAN
FallbackNeedsNonConsumption == fallback => phase \in {"Queued", "Running", "Settled"}
DispatchNeedsDurability == dispatches = 1 => saved
ReceiptNeedsRetirement == receipt => retired
UnknownDoesNotQueue == phase = "Unknown" => ~fallback
RecoveryNeedsExactIdentity == recovered => retryIdentity = "Exact"
InjectedNotSecondInvocation == phase = "Injected" => dispatches = 0 /\ ~fallback
=============================================================================
