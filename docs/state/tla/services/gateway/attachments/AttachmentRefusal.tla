---- MODULE AttachmentRefusal ----
EXTENDS Naturals, FiniteSets
VARIABLES active, scope, notice, answered, fact, resolved
vars == <<active, scope, notice, answered, fact, resolved>>
Init == /\ active = 1 /\ scope = 1 /\ notice = [t \in {1,2} |-> FALSE] /\ answered = {} /\ fact = TRUE /\ resolved = FALSE
Switch == /\ active' = 3 - active /\ UNCHANGED <<scope,notice,answered,fact,resolved>>
RefuseCaptured == /\ scope = 1 /\ \E target \in {1,2}: /\ notice' = [notice EXCEPT ![target] = TRUE] /\ answered' = answered \ {target} /\ UNCHANGED <<active,scope,fact,resolved>>
Answer == /\ \E target \in {1,2}: /\ notice' = [notice EXCEPT ![target] = FALSE] /\ answered' = answered \cup {target} /\ UNCHANGED <<active,scope,fact,resolved>>
RestoreDraft == /\ UNCHANGED <<active,scope,notice,answered,fact,resolved>>
ResolveFact == /\ fact /\ fact' = FALSE /\ resolved' = TRUE /\ UNCHANGED <<active,scope,notice,answered>>
ReplaceScope == /\ scope = 1 /\ scope' = 2 /\ notice' = [t \in {1,2} |-> FALSE] /\ UNCHANGED <<active,answered,fact,resolved>>
Next == Switch \/ RefuseCaptured \/ Answer \/ RestoreDraft \/ ResolveFact \/ ReplaceScope
Spec == Init /\ [][Next]_vars
AnsweredAttemptStaysClear == \A target \in answered: ~notice[target]
DismissDoesNotResolveFact == fact = ~resolved
RetiredScopeCannotShowNotice == scope = 2 => \A target \in {1,2}: ~notice[target]
====
