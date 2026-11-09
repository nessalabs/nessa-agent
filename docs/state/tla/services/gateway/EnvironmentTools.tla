---- MODULE EnvironmentTools ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, asked, granted, sandbox, leaseLive, hold, issued, ran, artifact, valid, recordedAtLive
vars == <<phase, asked, granted, sandbox, leaseLive, hold, issued, ran, artifact, valid, recordedAtLive>>
Init == /\ phase = "Source" /\ asked \in 1..2 /\ granted \in 1..2 /\ sandbox \in {"None","Other"} /\ leaseLive = TRUE /\ hold = FALSE /\ issued = FALSE /\ ran = FALSE /\ artifact = "None" /\ valid \in BOOLEAN /\ recordedAtLive = FALSE
Source == /\ phase = "Source" /\ leaseLive /\ phase' = "Policy" /\ UNCHANGED <<asked,granted,sandbox,leaseLive,hold,issued,ran,artifact,valid,recordedAtLive>>
PolicyAllows == /\ phase = "Policy" /\ phase' = "Grant" /\ UNCHANGED <<asked,granted,sandbox,leaseLive,hold,issued,ran,artifact,valid,recordedAtLive>>
PolicyDenies == /\ phase = "Policy" /\ phase' = "Refused" /\ UNCHANGED <<asked,granted,sandbox,leaseLive,hold,issued,ran,artifact,valid,recordedAtLive>>
Grant == /\ phase = "Grant" /\ granted <= asked /\ sandbox = "None" /\ hold' = TRUE /\ phase' = "Granted" /\ UNCHANGED <<asked,granted,sandbox,leaseLive,issued,ran,artifact,valid,recordedAtLive>>
Issue == /\ phase = "Granted" /\ hold /\ leaseLive /\ issued' = TRUE /\ phase' = "Issued" /\ UNCHANGED <<asked,granted,sandbox,leaseLive,hold,ran,artifact,valid,recordedAtLive>>
IssueFailure == /\ phase = "Granted" /\ hold' = FALSE /\ phase' = "Refused" /\ UNCHANGED <<asked,granted,sandbox,leaseLive,issued,ran,artifact,valid,recordedAtLive>>
ParentEnds == /\ leaseLive /\ leaseLive' = FALSE /\ UNCHANGED <<phase,asked,granted,sandbox,hold,issued,ran,artifact,valid,recordedAtLive>>
Run == /\ phase = "Issued" /\ issued /\ hold /\ ran' = TRUE /\ phase' = "Running" /\ UNCHANGED <<asked,granted,sandbox,leaseLive,hold,issued,artifact,valid,recordedAtLive>>
KeepArtifact == /\ phase = "Running" /\ artifact = "None" /\ valid /\ artifact' = "Held" /\ UNCHANGED <<phase,asked,granted,sandbox,leaseLive,hold,issued,ran,valid,recordedAtLive>>
RecordArtifact == /\ phase = "Running" /\ artifact = "Held" /\ leaseLive /\ artifact' = "Recorded" /\ recordedAtLive' = TRUE /\ UNCHANGED <<phase,asked,granted,sandbox,leaseLive,hold,issued,ran,valid>>
ArtifactRecordFailure == /\ artifact = "Held" /\ artifact' \in {"Reverted","Uncertain"} /\ UNCHANGED <<phase,asked,granted,sandbox,leaseLive,hold,issued,ran,valid,recordedAtLive>>
End == /\ phase = "Running" /\ phase' = "Done" /\ hold' = FALSE /\ UNCHANGED <<asked,granted,sandbox,leaseLive,issued,ran,artifact,valid,recordedAtLive>>
Next == Source \/ PolicyAllows \/ PolicyDenies \/ Grant \/ Issue \/ IssueFailure \/ ParentEnds \/ Run \/ KeepArtifact \/ RecordArtifact \/ ArtifactRecordFailure \/ End
Spec == Init /\ [][Next]_vars
GrantedTermsDoNotWiden == ran => granted <= asked /\ sandbox = "None"
SaveIssueBeforeRun == ran => issued
ArtifactRecordHasVerifiedHold == artifact = "Recorded" => valid /\ recordedAtLive /\ issued /\ ran
====
