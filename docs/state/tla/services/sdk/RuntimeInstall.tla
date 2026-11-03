---------------- MODULE RuntimeInstall ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES phase, digest, audit, oldUse, reclaimed
vars == <<phase, digest, audit, oldUse, reclaimed>>
Init == phase = "Downloading" /\ digest = FALSE /\ audit = FALSE /\ oldUse = TRUE /\ reclaimed = FALSE
Verify == phase = "Downloading" /\ phase' = "Verified" /\ digest' = TRUE /\ UNCHANGED <<audit, oldUse, reclaimed>>
Reject == phase = "Downloading" /\ phase' = "Rejected" /\ UNCHANGED <<digest, audit, oldUse, reclaimed>>
Prepare == phase = "Verified" /\ digest /\ phase' = "Prepared" /\ audit' = TRUE /\ UNCHANGED <<digest, oldUse, reclaimed>>
Publish == phase = "Prepared" /\ digest /\ audit /\ phase' = "Published" /\ UNCHANGED <<digest, audit, oldUse, reclaimed>>
ReleaseOld == oldUse /\ oldUse' = FALSE /\ UNCHANGED <<phase, digest, audit, reclaimed>>
Reclaim == phase = "Published" /\ ~oldUse /\ reclaimed' = TRUE /\ UNCHANGED <<phase, digest, audit, oldUse>>
CompleteReclaim == phase = "Published" /\ reclaimed /\ phase' = "Reclaimed" /\ UNCHANGED <<digest, audit, oldUse, reclaimed>>
Next == Verify \/ Reject \/ Prepare \/ Publish \/ ReleaseOld \/ Reclaim \/ CompleteReclaim
Spec == Init /\ [][Next]_vars
TypeOK == phase \in {"Downloading", "Verified", "Prepared", "Published", "Reclaimed", "Rejected"} /\ digest \in BOOLEAN /\ audit \in BOOLEAN /\ oldUse \in BOOLEAN /\ reclaimed \in BOOLEAN
PublicationNeedsPin == phase \in {"Published", "Reclaimed"} => digest /\ audit
NoReclaimLiveUse == reclaimed => ~oldUse
ReclaimedNeedsRelease == phase = "Reclaimed" => reclaimed
=============================================================================
