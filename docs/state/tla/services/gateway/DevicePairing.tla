---- MODULE DevicePairing ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, revision, admission, claimed, approved, stage, receiver, current, proof, authorized
vars == <<phase, revision, admission, claimed, approved, stage, receiver, current, proof, authorized>>
Init == /\ phase = "Available" /\ revision = 0 /\ admission = 9 /\ claimed = 0 /\ approved = 0 /\ stage = 0 /\ receiver = FALSE /\ current = "None" /\ proof = FALSE /\ authorized = TRUE
Admit == /\ phase # "Active" /\ authorized /\ admission' = revision /\ UNCHANGED <<phase,revision,claimed,approved,stage,receiver,current,proof,authorized>>
Claim == /\ phase = "Available" /\ claimed' \in {1,2} /\ phase' = "Claimed" /\ revision' = 1 /\ UNCHANGED <<admission,approved,stage,receiver,current,proof,authorized>>
Approve == /\ phase = "Claimed" /\ authorized /\ admission = revision /\ \E key \in {1,2}: /\ key = claimed /\ approved' = key /\ phase' = "Approved" /\ revision' = 2 /\ UNCHANGED <<admission,claimed,stage,receiver,current,proof,authorized>>
Stage == /\ phase = "Approved" /\ authorized /\ admission = revision /\ stage' = 1 /\ phase' = "Staging" /\ revision' = 3 /\ UNCHANGED <<admission,claimed,approved,receiver,current,proof,authorized>>
Receiver == /\ phase = "Staging" /\ stage = 1 /\ receiver' = TRUE /\ current' \in {"Held","Revoked","RegrantedAfterFence"} /\ phase' = "ReceiverRemembered" /\ revision' = 4 /\ UNCHANGED <<admission,claimed,approved,stage,proof,authorized>>
Holding == /\ phase = "ReceiverRemembered" /\ receiver /\ current = "Held" /\ proof' = TRUE /\ phase' = "ReceiverProven" /\ UNCHANGED <<revision,admission,claimed,approved,stage,receiver,current,authorized>>
Publish == /\ phase = "ReceiverProven" /\ authorized /\ admission = revision /\ proof /\ phase' = "Active" /\ UNCHANGED <<revision,admission,claimed,approved,stage,receiver,current,proof,authorized>>
RevokeAuthority == /\ phase # "Active" /\ authorized /\ authorized' = FALSE /\ UNCHANGED <<phase,revision,admission,claimed,approved,stage,receiver,current,proof>>
Next == Admit \/ Claim \/ Approve \/ Stage \/ Receiver \/ Holding \/ Publish \/ RevokeAuthority
Spec == Init /\ [][Next]_vars
ExactApprovedKey == approved # 0 => approved = claimed
ActiveNeedsCurrentReceiver == phase = "Active" => receiver /\ current = "Held" /\ proof /\ stage = 1
FreshPublication == phase = "Active" => authorized /\ admission = revision
ApprovalIsNotActivation == phase \in {"Approved","Staging"} => ~proof
====
