---- MODULE FocusLanding ----
EXTENDS Naturals, FiniteSets, Sequences
VARIABLES requested, marked, captured, phase, authority, live, landed, atLanding, blocked
vars == <<requested, marked, captured, phase, authority, live, landed, atLanding, blocked>>
Init == /\ requested = "A" /\ marked = "A" /\ captured = "none" /\ phase = "Searching" /\ authority = "held" /\ live = TRUE /\ landed = "none" /\ atLanding = "none" /\ blocked = FALSE
Find == /\ live /\ phase = "Searching" /\ requested = marked /\ captured' = marked /\ phase' = "Painting" /\ UNCHANGED <<requested,marked,authority,live,landed,atLanding,blocked>>
ChangePane == /\ requested' \in {"A","B"} /\ UNCHANGED <<marked,captured,phase,authority,live,landed,atLanding,blocked>>
PaintMark == /\ marked' = requested /\ UNCHANGED <<requested,captured,phase,authority,live,landed,atLanding,blocked>>
PersonMoves == /\ authority' \in {"person","modal"} /\ UNCHANGED <<requested,marked,captured,phase,live,landed,atLanding,blocked>>
Recheck == /\ live /\ phase = "Painting" /\ captured # requested /\ phase' = "Searching" /\ UNCHANGED <<requested,marked,captured,authority,live,landed,atLanding,blocked>>
Land == /\ live /\ phase = "Painting" /\ captured = requested /\ requested = marked /\ authority = "held" /\ landed' = captured /\ atLanding' = requested /\ blocked' = (authority # "held") /\ phase' = "Done" /\ UNCHANGED <<requested,marked,captured,authority,live>>
Retire == /\ live /\ live' = FALSE /\ phase' = "Retired" /\ UNCHANGED <<requested,marked,captured,authority,landed,atLanding,blocked>>
Next == Find \/ ChangePane \/ PaintMark \/ PersonMoves \/ Recheck \/ Land \/ Retire
TargetRechecked == landed = "none" \/ landed = atLanding
DeliberateFocusWins == ~blocked
RetirementTerminal == ~live => phase = "Retired"
Spec == Init /\ [][Next]_vars
====
