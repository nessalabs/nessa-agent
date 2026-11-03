---- MODULE NativeSubmissions ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES arrived, queue, selected, running, tickets, owned, done, neverRun, order, closing, calls, receipts, startedBeforeClose
vars == <<arrived, queue, selected, running, tickets, owned, done, neverRun, order, closing, calls, receipts, startedBeforeClose>>
Range(s) == {s[i] : i \in DOMAIN s}
Remove(s,item) == SelectSeq(s,LAMBDA each: each # item)
Init == /\ arrived = 0 /\ queue = <<>> /\ selected = 0 /\ running = 0 /\ tickets = {} /\ owned = {} /\ done = {} /\ neverRun = {} /\ order = <<>> /\ closing = FALSE /\ calls = 0 /\ receipts = {} /\ startedBeforeClose = FALSE
Arrive == /\ arrived < 3 /\ ~closing /\ arrived' = arrived + 1 /\ queue' = Append(queue,arrived + 1) /\ owned' = owned \cup {arrived + 1} /\ calls' = calls + 1 /\ UNCHANGED <<selected,running,tickets,done,neverRun,order,closing,receipts,startedBeforeClose>>
Promote == /\ ~closing /\ selected = 0 /\ running = 0 /\ Len(queue) > 0 /\ selected' = Head(queue) /\ queue' = Tail(queue) /\ UNCHANGED <<arrived,running,tickets,owned,done,neverRun,order,closing,calls,receipts,startedBeforeClose>>
StartBody == /\ selected # 0 /\ running = 0 /\ running' = selected /\ order' = Append(order,selected) /\ selected' = 0 /\ startedBeforeClose' = ~closing /\ UNCHANGED <<arrived,queue,tickets,owned,done,neverRun,closing,calls,receipts>>
CancelWaiting == \E attempt \in Range(queue): /\ queue' = Remove(queue,attempt) /\ done' = done \cup {attempt} /\ neverRun' = neverRun \cup {attempt} /\ calls' = calls - 1 /\ UNCHANGED <<arrived,selected,running,tickets,owned,order,closing,receipts,startedBeforeClose>>
CancelSelected == /\ selected # 0 /\ done' = done \cup {selected} /\ neverRun' = neverRun \cup {selected} /\ selected' = 0 /\ calls' = calls - 1 /\ UNCHANGED <<arrived,queue,running,tickets,owned,order,closing,receipts,startedBeforeClose>>
BodyRefuses == /\ running # 0 /\ done' = done \cup {running} /\ calls' = calls - 1 /\ running' = 0 /\ UNCHANGED <<arrived,queue,selected,tickets,owned,neverRun,order,closing,receipts,startedBeforeClose>>
BodyReturnsTicket == /\ running # 0 /\ startedBeforeClose /\ tickets' = tickets \cup {running} /\ running' = 0 /\ UNCHANGED <<arrived,queue,selected,owned,done,neverRun,order,closing,calls,receipts,startedBeforeClose>>
CaptureCompletion == \E ticket \in tickets: /\ tickets' = tickets \ {ticket} /\ done' = done \cup {ticket} /\ receipts' = receipts \cup {ticket} /\ calls' = calls - 1 /\ UNCHANGED <<arrived,queue,selected,running,owned,neverRun,order,closing,startedBeforeClose>>
Close == /\ ~closing /\ closing' = TRUE /\ done' = done \cup Range(queue) /\ neverRun' = neverRun \cup Range(queue) /\ calls' = calls - Len(queue) /\ queue' = <<>> /\ UNCHANGED <<arrived,selected,running,tickets,owned,order,receipts,startedBeforeClose>>
Next == Arrive \/ Promote \/ StartBody \/ CancelWaiting \/ CancelSelected \/ BodyRefuses \/ BodyReturnsTicket \/ CaptureCompletion \/ Close
OneBodyAtATime == selected = 0 \/ running = 0
BodiesStayInArrivalOrder == \A i,j \in DOMAIN order: i < j => order[i] < order[j]
CancelledQueueNeverRuns == Range(order) \cap neverRun = {}
DrainAdmissionOwnedOnce == calls = Cardinality(owned \ done)
ClosedQueueReleased == closing => queue = <<>>
TicketCompletionRetainsIdentity == receipts \subseteq done /\ receipts \subseteq Range(order)
Spec == Init /\ [][Next]_vars
====
