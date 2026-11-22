---- MODULE ShareATerminalBetweenViews ----
\* Several views of one terminal session: who sets the size, which frames a view
\* paints, and in what order. One terminal; each view has its own fit (the grid
\* that fills it). Transport is abstracted into per-view FIFO queues of frames.
EXTENDS Naturals, Sequences, FiniteSets
CONSTANTS Views, Fits, StartFit, MaxSeq, None
ASSUME StartFit \in Fits
ASSUME None \notin Views

VARIABLES attached, fit, owner, size, seq, sizeAt, queue, shown, resizedByOwner
vars == <<attached, fit, owner, size, seq, sizeAt, queue, shown, resizedByOwner>>

Frame(n) == [seq |-> n, size |-> sizeAt[n]]
Blank == [seq |-> 0, size |-> None]

Init ==
  /\ attached = {}
  /\ fit = [v \in Views |-> StartFit]
  /\ owner = None
  /\ size = StartFit
  /\ seq = 0
  /\ sizeAt = [n \in 0..MaxSeq |-> IF n = 0 THEN StartFit ELSE None]
  /\ queue = [v \in Views |-> <<>>]
  /\ shown = [v \in Views |-> Blank]
  /\ resizedByOwner = TRUE

\* Publish frame seq+1 at size s to every attached view.
Publish(s) ==
  /\ seq < MaxSeq
  /\ seq' = seq + 1
  /\ sizeAt' = [sizeAt EXCEPT ![seq + 1] = s]
  /\ queue' = [v \in Views |-> IF v \in attached
                                THEN Append(queue[v], [seq |-> seq + 1, size |-> s])
                                ELSE queue[v]]

\* A view opens: it receives a checkpoint of the current screen, then live frames.
Attach(v) ==
  /\ v \notin attached
  /\ attached' = attached \cup {v}
  /\ queue' = [queue EXCEPT ![v] = <<Frame(seq)>>]
  /\ UNCHANGED <<fit, owner, size, seq, sizeAt, shown, resizedByOwner>>

\* Focus claims the size. Only the claiming view's fit is ever applied.
Focus(v) ==
  /\ v \in attached
  /\ owner # v
  /\ owner' = v
  /\ IF fit[v] # size
       THEN /\ Publish(fit[v]) /\ size' = fit[v] /\ resizedByOwner' = TRUE
       ELSE /\ UNCHANGED <<seq, sizeAt, queue, size, resizedByOwner>>
  /\ UNCHANGED <<attached, fit, shown>>

\* A view's own box changes (window resize, split). Only the owner resizes the terminal.
Refit(v, f) ==
  /\ v \in attached
  /\ f # fit[v]
  /\ fit' = [fit EXCEPT ![v] = f]
  /\ IF owner = v
       THEN /\ Publish(f) /\ size' = f /\ resizedByOwner' = TRUE
       ELSE UNCHANGED <<seq, sizeAt, queue, size, resizedByOwner>>
  /\ UNCHANGED <<attached, owner, shown>>

\* The program writes; the frame keeps the current size.
Output ==
  /\ attached # {}
  /\ Publish(size)
  /\ UNCHANGED <<attached, fit, owner, size, shown, resizedByOwner>>

\* A view paints the next frame it received.
Deliver(v) ==
  /\ queue[v] # <<>>
  /\ shown' = [shown EXCEPT ![v] = Head(queue[v])]
  /\ queue' = [queue EXCEPT ![v] = Tail(queue[v])]
  /\ UNCHANGED <<attached, fit, owner, size, seq, sizeAt, resizedByOwner>>

\* A view closes. The size stays where it is until another view takes focus.
Detach(v) ==
  /\ v \in attached
  /\ attached' = attached \ {v}
  /\ owner' = IF owner = v THEN None ELSE owner
  /\ queue' = [queue EXCEPT ![v] = <<>>]
  /\ shown' = [shown EXCEPT ![v] = Blank]
  /\ UNCHANGED <<fit, size, seq, sizeAt, resizedByOwner>>

Next ==
  \/ Output
  \/ \E v \in Views:
       \/ Attach(v) \/ Focus(v) \/ Deliver(v) \/ Detach(v)
       \/ \E f \in Fits: Refit(v, f)

Spec == Init /\ [][Next]_vars

\* ---- Invariants ----
OwnerAttached == owner = None \/ owner \in attached
SizeFollowsOwner == owner = None \/ size = fit[owner]
OnlyOwnerResizes == resizedByOwner
\* A view never paints an older frame after a newer one (no jumping back).
QueueAhead == \A v \in Views:
  \A i \in 1..Len(queue[v]):
    /\ queue[v][i].seq >= shown[v].seq
    /\ (i > 1 => queue[v][i].seq > queue[v][i-1].seq)
\* What a view paints is the server's grid at that frame: size and cursor come from the frame.
ShownIsServer == \A v \in Views: shown[v].seq = 0 \/ shown[v].size = sizeAt[shown[v].seq]
\* A detached view holds nothing.
DetachedIsEmpty == \A v \in Views: v \notin attached => (queue[v] = <<>> /\ shown[v] = Blank)

TypeOK ==
  /\ attached \subseteq Views
  /\ owner \in Views \cup {None}
  /\ size \in Fits
  /\ seq \in 0..MaxSeq
====
