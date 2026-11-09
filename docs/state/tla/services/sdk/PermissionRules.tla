---------------- MODULE PermissionRules ----------------
EXTENDS Naturals, FiniteSets
\* Finite safety abstraction; stuttering and incomplete operations are allowed.
VARIABLES phase, policy, rule, ruleAudit, notice, consumed
vars == <<phase, policy, rule, ruleAudit, notice, consumed>>
Init == phase = "Pending" /\ policy = FALSE /\ rule = FALSE /\ ruleAudit = FALSE /\ notice = FALSE /\ consumed = FALSE
Notice == phase = "Pending" /\ notice' = TRUE /\ UNCHANGED <<phase, policy, rule, ruleAudit, consumed>>
PolicyAllow == phase = "Pending" /\ policy' = TRUE /\ UNCHANGED <<phase, rule, ruleAudit, notice, consumed>>
Remember == phase = "Pending" /\ policy /\ phase' = "Remembered" /\ rule' = TRUE /\ ruleAudit' = TRUE /\ UNCHANGED <<policy, notice, consumed>>
Select == phase = "Remembered" /\ phase' = "Selected" /\ consumed' = TRUE /\ notice' = FALSE /\ UNCHANGED <<policy, rule, ruleAudit>>
DeliveryFail == phase = "Selected" /\ phase' = "DeliveryFailed" /\ UNCHANGED <<policy, rule, ruleAudit, notice, consumed>>
Written == phase = "Selected" /\ phase' = "Answered" /\ UNCHANGED <<policy, rule, ruleAudit, notice, consumed>>
Cancel == phase = "Pending" /\ phase' = "Cancelled" /\ notice' = FALSE /\ UNCHANGED <<policy, rule, ruleAudit, consumed>>
Next == Notice \/ PolicyAllow \/ Remember \/ Select \/ DeliveryFail \/ Written \/ Cancel
Spec == Init /\ [][Next]_vars
TypeOK == phase \in {"Pending", "Remembered", "Selected", "DeliveryFailed", "Answered", "Cancelled"} /\ policy \in BOOLEAN /\ rule \in BOOLEAN /\ ruleAudit \in BOOLEAN /\ notice \in BOOLEAN /\ consumed \in BOOLEAN
RuleRequiresPolicy == rule => policy /\ ruleAudit
ConsumedClearsNotice == consumed => ~notice
DeliveryFailureKeepsRule == phase = "DeliveryFailed" => rule
CancelledClearsNotice == phase = "Cancelled" => ~notice
=============================================================================
