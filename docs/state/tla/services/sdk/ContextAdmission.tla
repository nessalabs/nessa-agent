---------------- MODULE ContextAdmission ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES phase, count, prefix, captured, boundary, sameId, sameProvider, nativeMissing, identityCommitted, contextCommitted
vars == <<phase, count, prefix, captured, boundary, sameId, sameProvider, nativeMissing, identityCommitted, contextCommitted>>
Init == phase = "BeforeOpen" /\ count \in 1..2 /\ prefix = 1 /\ captured = 0 /\ boundary = 0 /\ sameId \in BOOLEAN /\ sameProvider \in BOOLEAN /\ nativeMissing \in BOOLEAN /\ identityCommitted = FALSE /\ contextCommitted = FALSE
Begin == phase = "BeforeOpen" /\ phase' = "Opening" /\ captured' = prefix /\ UNCHANGED <<count, prefix, boundary, sameId, sameProvider, nativeMissing, identityCommitted, contextCommitted>>
Admit == phase = "Opening" /\ count = 1 /\ count' = 2 /\ UNCHANGED <<phase, prefix, captured, boundary, sameId, sameProvider, nativeMissing, identityCommitted, contextCommitted>>
Withdraw == phase = "Opening" /\ count = 2 /\ prefix' = 2 /\ UNCHANGED <<phase, count, captured, boundary, sameId, sameProvider, nativeMissing, identityCommitted, contextCommitted>>
OpenReturns == phase = "Opening" /\ phase' = "Candidate" /\ UNCHANGED <<count, prefix, captured, boundary, sameId, sameProvider, nativeMissing, identityCommitted, contextCommitted>>
Publish == phase = "Candidate" /\ phase' = "Published" /\ boundary' = (IF sameId /\ sameProvider THEN 0 ELSE captured) /\ identityCommitted' = TRUE /\ contextCommitted' = TRUE /\ UNCHANGED <<count, prefix, captured, sameId, sameProvider, nativeMissing>>
PublicationFails == phase = "Candidate" /\ phase' = "Cleanup" /\ UNCHANGED <<count, prefix, captured, boundary, sameId, sameProvider, nativeMissing, identityCommitted, contextCommitted>>
OpenReopened == phase = "Published" /\ (~nativeMissing \/ count = boundary) /\ phase' = "Ready" /\ UNCHANGED <<count, prefix, captured, boundary, sameId, sameProvider, nativeMissing, identityCommitted, contextCommitted>>
MissingUsedHistory == phase = "Published" /\ nativeMissing /\ count # boundary /\ phase' = "Refused" /\ UNCHANGED <<count, prefix, captured, boundary, sameId, sameProvider, nativeMissing, identityCommitted, contextCommitted>>
Next == Begin \/ Admit \/ Withdraw \/ OpenReturns \/ Publish \/ PublicationFails \/ OpenReopened \/ MissingUsedHistory
Spec == Init /\ [][Next]_vars
TypeOK == phase \in {"BeforeOpen", "Opening", "Candidate", "Published", "Ready", "Refused", "Cleanup"} /\ count \in 1..2 /\ prefix \in 1..2 /\ captured \in 0..2 /\ boundary \in 0..2 /\ sameId \in BOOLEAN /\ sameProvider \in BOOLEAN /\ nativeMissing \in BOOLEAN /\ identityCommitted \in BOOLEAN /\ contextCommitted \in BOOLEAN
CapturedBoundaryPreserved == contextCommitted /\ ~(sameId /\ sameProvider) => boundary = captured
ReusedIdKeepsAdmission == contextCommitted /\ sameId /\ sameProvider => boundary = 0
LaterAdmissionStillUsed == contextCommitted /\ count > captured => count # boundary
AtomicIdentityPublication == identityCommitted = contextCommitted
MissingUsedContextRefused == phase = "Ready" /\ nativeMissing => count = boundary
=============================================================================
