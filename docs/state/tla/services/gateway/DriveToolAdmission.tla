---- MODULE DriveToolAdmission ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, mode, current, admitted, retained, held, savedAt, steeredAt, decision, audited, effect
vars == <<phase, mode, current, admitted, retained, held, savedAt, steeredAt, decision, audited, effect>>
Init == /\ phase = "Policy" /\ mode \in {"Queued","Native"} /\ current = 1 /\ admitted = 1 /\ retained = FALSE /\ held = FALSE /\ savedAt = 0 /\ steeredAt = 0 /\ decision = "None" /\ audited = FALSE /\ effect = FALSE
PolicyRead == /\ phase = "Policy" /\ retained' = TRUE /\ phase' = "Holding" /\ UNCHANGED <<mode,current,admitted,held,savedAt,steeredAt,decision,audited,effect>>
Revise == /\ ~held /\ current = 1 /\ current' = 2 /\ UNCHANGED <<phase,mode,admitted,retained,held,savedAt,steeredAt,decision,audited,effect>>
Hold == /\ phase = "Holding" /\ retained /\ current = admitted /\ held' = TRUE /\ phase' = "Saving" /\ UNCHANGED <<mode,current,admitted,retained,savedAt,steeredAt,decision,audited,effect>>
Save == /\ phase = "Saving" /\ held /\ savedAt' = current /\ held' = FALSE /\ phase' = (IF mode = "Native" THEN "Recheck" ELSE "ToolReady") /\ UNCHANGED <<mode,current,admitted,retained,steeredAt,decision,audited,effect>>
Recheck == /\ phase = "Recheck" /\ current = admitted /\ held' = TRUE /\ phase' = "Steer" /\ UNCHANGED <<mode,current,admitted,retained,savedAt,steeredAt,decision,audited,effect>>
Steer == /\ phase = "Steer" /\ held /\ steeredAt' = current /\ held' = FALSE /\ phase' = "ToolReady" /\ UNCHANGED <<mode,current,admitted,retained,savedAt,decision,audited,effect>>
Evict == /\ savedAt # 0 /\ retained /\ retained' = FALSE /\ UNCHANGED <<phase,mode,current,admitted,held,savedAt,steeredAt,decision,audited,effect>>
Allow == /\ phase = "ToolReady" /\ retained /\ decision' = "Allow" /\ phase' = "Decided" /\ UNCHANGED <<mode,current,admitted,retained,held,savedAt,steeredAt,audited,effect>>
Deny == /\ phase = "ToolReady" /\ decision' = "Deny" /\ phase' = "Decided" /\ UNCHANGED <<mode,current,admitted,retained,held,savedAt,steeredAt,audited,effect>>
Audit == /\ phase = "Decided" /\ audited' = TRUE /\ phase' = "Audited" /\ UNCHANGED <<mode,current,admitted,retained,held,savedAt,steeredAt,decision,effect>>
AuditFailure == /\ phase = "Decided" /\ phase' = "Refused" /\ UNCHANGED <<mode,current,admitted,retained,held,savedAt,steeredAt,decision,audited,effect>>
Effect == /\ phase = "Audited" /\ decision = "Allow" /\ audited /\ effect' = TRUE /\ phase' = "Done" /\ UNCHANGED <<mode,current,admitted,retained,held,savedAt,steeredAt,decision,audited>>
Next == PolicyRead \/ Revise \/ Hold \/ Save \/ Recheck \/ Steer \/ Evict \/ Allow \/ Deny \/ Audit \/ AuditFailure \/ Effect
Spec == Init /\ [][Next]_vars
SaveUsesHeldRevision == savedAt # 0 => savedAt = admitted
NativeSteerRechecksRevision == steeredAt # 0 => steeredAt = admitted
ToolEffectHasDecisionAudit == effect => decision = "Allow" /\ audited /\ savedAt = admitted
====
