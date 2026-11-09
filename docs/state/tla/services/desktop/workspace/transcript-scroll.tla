---- MODULE TranscriptScroll ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES top, height, seenTop, pinned, live, last, movedAway, before
vars == <<top, height, seenTop, pinned, live, last, movedAway, before>>
Init == /\ top = 1 /\ height = 2 /\ seenTop = 1 /\ pinned = TRUE /\ live = TRUE /\ last = "init" /\ movedAway = FALSE /\ before = 1
Grow == /\ live /\ height = 2 /\ height' = 3 /\ last' = "grow" /\ UNCHANGED <<top,seenTop,pinned,live,movedAway,before>>
ReaderMoves == /\ live /\ top' \in 0..(height-1) /\ last' = "move" /\ UNCHANGED <<height,seenTop,pinned,live,movedAway,before>>
ScrollEvent == /\ live /\ pinned' = (height - 1 - top < 1) /\ seenTop' = top /\ last' = "scroll" /\ UNCHANGED <<top,height,live,movedAway,before>>
Resize == /\ live /\ before' = top /\ movedAway' = (top # seenTop /\ height - 1 - top >= 1) /\ pinned' = (IF top # seenTop THEN height - 1 - top < 1 ELSE pinned) /\ top' = (IF (IF top # seenTop THEN height - 1 - top < 1 ELSE pinned) THEN height - 1 ELSE top) /\ seenTop' = top' /\ last' = "resize" /\ UNCHANGED <<height,live>>
ReturnSettles == /\ live /\ top' = height - 1 /\ seenTop' = height - 1 /\ pinned' = TRUE /\ last' = "return" /\ UNCHANGED <<height,live,movedAway,before>>
Retire == /\ live /\ live' = FALSE /\ before' = top /\ last' = "retire" /\ UNCHANGED <<top,height,seenTop,pinned,movedAway>>
LateResize == /\ ~live /\ before' = top /\ last' = "late" /\ UNCHANGED <<top,height,seenTop,pinned,live,movedAway>>
Next == Grow \/ ReaderMoves \/ ScrollEvent \/ Resize \/ ReturnSettles \/ Retire \/ LateResize
ReaderMovementWins == (last = "resize" /\ movedAway) => top = before
RetiredObserverInert == last = "late" => top = before
RememberPublishedPosition == last = "resize" => seenTop = top
Spec == Init /\ [][Next]_vars
====
