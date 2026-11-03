---- MODULE GithubRead ----
EXTENDS Naturals, FiniteSets
VARIABLES phase, generation, captured, stopped, answered
vars == <<phase, generation, captured, stopped, answered>>
Init == /\ phase = "Waiting" /\ generation = 1 /\ captured = 1 /\ stopped = FALSE /\ answered = FALSE
Lock == /\ phase = "Waiting" /\ ~stopped /\ generation = captured /\ phase' = "Reading" /\ UNCHANGED <<generation,captured,stopped,answered>>
Stop == /\ ~stopped /\ stopped' = TRUE /\ UNCHANGED <<phase,generation,captured,answered>>
Replace == /\ generation = 1 /\ generation' = 2 /\ UNCHANGED <<phase,captured,stopped,answered>>
Answer == /\ phase = "Reading" /\ ~stopped /\ generation = captured /\ answered' = TRUE /\ phase' = "Answered" /\ UNCHANGED <<generation,captured,stopped>>
Refuse == /\ phase \in {"Waiting","Reading"} /\ (stopped \/ generation # captured) /\ phase' = "Refused" /\ UNCHANGED <<generation,captured,stopped,answered>>
Next == Lock \/ Stop \/ Replace \/ Answer \/ Refuse
Spec == Init /\ [][Next]_vars
CapturedIdentity == captured = 1
RefusalHasNoAnswer == phase = "Refused" => ~answered
====
