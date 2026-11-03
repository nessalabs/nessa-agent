-------------------------- MODULE ResourceTickets --------------------------
EXTENDS Naturals
\* One successfully issued ticket. Time is abstracted to an expiry event.
\* Each Next action is one store/route decision, not an atomic network call.
VARIABLES phase, held, redemptions, delivered, issueRecorded
vars == <<phase, held, redemptions, delivered, issueRecorded>>
Init == /\ phase = "Pending" /\ held = TRUE /\ redemptions = 0
        /\ delivered = FALSE /\ issueRecorded = FALSE
IssueAck == /\ phase = "Pending" /\ phase' = "Active"
            /\ issueRecorded' = TRUE
            /\ UNCHANGED <<held, redemptions, delivered>>
IssueFail == /\ phase \in {"Pending", "EndedPending"}
             /\ phase' = "Discarded" /\ held' = FALSE
             /\ UNCHANGED <<redemptions, delivered, issueRecorded>>
EndPending == /\ phase = "Pending" /\ phase' = "EndedPending" /\ held' = FALSE
              /\ UNCHANGED <<redemptions, delivered, issueRecorded>>
AckEnded == /\ phase = "EndedPending" /\ phase' = "Ended"
            /\ issueRecorded' = TRUE
            /\ UNCHANGED <<held, redemptions, delivered>>
EndActive == /\ phase = "Active" /\ phase' = "Ended" /\ held' = FALSE
             /\ UNCHANGED <<redemptions, delivered, issueRecorded>>
Redeem == /\ phase = "Active" /\ held
          /\ phase' = "Redeemed" /\ held' = FALSE
          /\ redemptions' = redemptions + 1
          /\ UNCHANGED <<delivered, issueRecorded>>
Deliver == /\ phase = "Redeemed" /\ phase' = "Delivered" /\ delivered' = TRUE
           /\ UNCHANGED <<held, redemptions, issueRecorded>>
AuditFail == /\ phase = "Redeemed" /\ phase' = "SpentWithoutDelivery"
             /\ UNCHANGED <<held, redemptions, delivered, issueRecorded>>
Next == IssueAck \/ IssueFail \/ EndPending \/ AckEnded \/ EndActive \/ Redeem \/ Deliver \/ AuditFail
Spec == Init /\ [][Next]_vars
TypeOK == /\ phase \in {"Pending", "Active", "EndedPending", "Ended", "Discarded", "Redeemed", "Delivered", "SpentWithoutDelivery"}
          /\ held \in BOOLEAN /\ delivered \in BOOLEAN /\ issueRecorded \in BOOLEAN
          /\ redemptions \in Nat
AtMostOnce == redemptions <= 1
HoldingMatchesPhase == (held <=> phase \in {"Pending", "Active"})
DeliveryRequiresConsumption == delivered => (redemptions = 1 /\ ~held /\ issueRecorded)
PendingCannotRedeem == redemptions = 1 => issueRecorded
=============================================================================
