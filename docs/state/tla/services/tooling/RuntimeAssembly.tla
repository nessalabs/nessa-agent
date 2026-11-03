---------------- MODULE RuntimeAssembly ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES phase, oldManifest, verified, outputValid
vars == <<phase, oldManifest, verified, outputValid>>
Init == phase = "Building" /\ oldManifest = FALSE /\ verified = FALSE /\ outputValid = FALSE
Stage == phase = "Building" /\ phase' = "Staging" /\ UNCHANGED <<oldManifest, verified, outputValid>>
Verify == phase = "Staging" /\ verified' = TRUE /\ UNCHANGED <<phase, oldManifest, outputValid>>
Publish == phase = "Staging" /\ verified /\ phase' = "Published" /\ UNCHANGED <<oldManifest, verified, outputValid>>
ValidateOutput == phase = "Published" /\ outputValid' = TRUE /\ UNCHANGED <<phase, oldManifest, verified>>
Complete == phase = "Published" /\ outputValid /\ phase' = "Complete" /\ UNCHANGED <<oldManifest, verified, outputValid>>
Fail == phase \in {"Building", "Staging", "Published"} /\ phase' = "Failed" /\ UNCHANGED <<oldManifest, verified, outputValid>>
Next == Stage \/ Verify \/ Publish \/ ValidateOutput \/ Complete \/ Fail
Spec == Init /\ [][Next]_vars
TypeOK == phase \in {"Building", "Staging", "Published", "Complete", "Failed"} /\ oldManifest \in BOOLEAN /\ verified \in BOOLEAN /\ outputValid \in BOOLEAN
NoStaleSuccess == ~oldManifest
PublicationNeedsVerification == phase \in {"Published", "Complete"} => verified
CompleteNeedsOutput == phase = "Complete" => outputValid
=============================================================================
