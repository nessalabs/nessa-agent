---- MODULE GatewayDeferredStage ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, plan, ready, copyStarted, settled
vars == <<phase, plan, ready, copyStarted, settled>>
Init == /\ phase = "Planning" /\ plan = FALSE /\ ready = FALSE /\ copyStarted = FALSE /\ settled = FALSE
Plan == /\ phase = "Planning" /\ phase' \in {"Accepted","Refused"} /\ plan' = (phase' = "Accepted") /\ UNCHANGED <<ready,copyStarted,settled>>
PublishReady == /\ phase = "Accepted" /\ plan /\ ready' = TRUE /\ phase' = "Copying" /\ copyStarted' = TRUE /\ UNCHANGED <<plan,settled>>
CopyOutcome == /\ phase = "Copying" /\ settled' = TRUE /\ phase' \in {"Completed","Failed"} /\ ready' = (phase' = "Completed") /\ UNCHANGED <<plan,copyStarted>>
Next == Plan \/ PublishReady \/ CopyOutcome
Spec == Init /\ [][Next]_vars
PlanBeforeCopy == copyStarted => plan
ReadyNotSettlement == phase = "Copying" => ready /\ ~settled
RefusedDoesNotCopy == phase = "Refused" => ~ready /\ ~copyStarted
====
