---- MODULE PanelSizePublication ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES live, pending, css, before, published, valid, last
vars == <<live, pending, css, before, published, valid, last>>
Init == /\ live = TRUE /\ pending = TRUE /\ css = "fallback" /\ before = "fallback" /\ published = FALSE /\ valid = TRUE /\ last = "init"
InitialRead == \E length \in {"small","large","invalid"}: /\ pending /\ live /\ pending' = FALSE /\ before' = css /\ css' = (IF length = "invalid" THEN css ELSE length) /\ published' = (length # "invalid") /\ valid' = (length # "invalid") /\ last' = "read" /\ UNCHANGED live
Resize == \E length \in {"small","large","invalid"}: /\ live /\ before' = css /\ css' = (IF length = "invalid" THEN css ELSE length) /\ published' = (length # "invalid") /\ valid' = (length # "invalid") /\ last' = "resize" /\ UNCHANGED <<live,pending>>
Retire == /\ live /\ live' = FALSE /\ before' = css /\ published' = FALSE /\ last' = "retire" /\ UNCHANGED <<pending,css,valid>>
LateRead == /\ ~live /\ pending /\ pending' = FALSE /\ before' = css /\ published' = FALSE /\ last' = "late" /\ UNCHANGED <<live,css,valid>>
Next == InitialRead \/ Resize \/ Retire \/ LateRead
OnlyValidLengths == published => valid /\ css \in {"small","large"}
InvalidPreservesFallback == ((last = "read" \/ last = "resize") /\ ~valid) => css = before
RetiredNoPublication == ~live => ~published
Spec == Init /\ [][Next]_vars
====
