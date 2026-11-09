---------------- MODULE UsageContinuation ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES phase, lease, fenced, identity, attempted
vars == <<phase, lease, fenced, identity, attempted>>
Init == phase = "Scheduled" /\ lease = FALSE /\ fenced = FALSE /\ identity = FALSE /\ attempted = FALSE
Claim == phase = "Scheduled" /\ phase' = "Delivering" /\ lease' = TRUE /\ UNCHANGED <<fenced, identity, attempted>>
Cancel == phase \in {"Scheduled", "Delivering"} /\ ~fenced /\ phase' = "Cancelled" /\ lease' = FALSE /\ UNCHANGED <<fenced, identity, attempted>>
Fence == phase = "Delivering" /\ lease /\ phase' = "Submitting" /\ fenced' = TRUE /\ identity' = TRUE /\ UNCHANGED <<lease, attempted>>
Submit == phase = "Submitting" /\ lease /\ identity /\ attempted' = TRUE /\ UNCHANGED <<phase, lease, fenced, identity>>
Confirm == phase = "Submitting" /\ attempted /\ phase' = "Delivered" /\ UNCHANGED <<lease, fenced, identity, attempted>>
Interrupted == phase = "Submitting" /\ attempted /\ phase' = "Unconfirmed" /\ UNCHANGED <<lease, fenced, identity, attempted>>
Refuse == phase = "Submitting" /\ ~attempted /\ phase' = "Refused" /\ UNCHANGED <<lease, fenced, identity, attempted>>
LoseLease == phase = "Delivering" /\ lease' = FALSE /\ UNCHANGED <<phase, fenced, identity, attempted>>
Next == LoseLease \/ Claim \/ Cancel \/ Fence \/ Submit \/ Confirm \/ Interrupted \/ Refuse
Spec == Init /\ [][Next]_vars
TypeOK == phase \in {"Scheduled", "Delivering", "Submitting", "Cancelled", "Delivered", "Unconfirmed", "Refused"} /\ lease \in BOOLEAN /\ fenced \in BOOLEAN /\ identity \in BOOLEAN /\ attempted \in BOOLEAN
SubmissionNeedsFence == attempted => fenced /\ identity /\ lease
CancellationBeforeEffect == phase = "Cancelled" => ~attempted /\ ~fenced
DeliveryNeedsAttempt == phase = "Delivered" => attempted
FenceNeedsLease == fenced => lease
=============================================================================
