------------------------- MODULE PoolMembership -------------------------
EXTENDS FiniteSets, Naturals
CONSTANTS Callers, Capacity
VARIABLES permits, members, retained
vars == <<permits, members, retained>>

Init == /\ permits = {} /\ members = {} /\ retained = {}

\* Each caller identifies one connection lifetime. A pending listener
\* completion can retain the endpoint after the carrier ends.
Admit(c) == /\ c \notin (permits \cup retained)
            /\ Cardinality(permits) < Capacity
            /\ permits' = permits \cup {c}
            /\ members' = (members \cap retained) \cup {c}
            /\ retained' = retained \cup {c}

End(c) == /\ c \in permits
          /\ members' = members \ {c}
          /\ permits' = permits \ {c}
          /\ UNCHANGED retained

ObserveCompletion(c) == /\ c \in retained /\ c \notin permits
                        /\ retained' = retained \ {c}
                        /\ UNCHANGED <<members, permits>>

Next == \E c \in Callers: Admit(c) \/ End(c) \/ ObserveCompletion(c)
Spec == Init /\ [][Next]_vars
RegistryFits == Cardinality(members) <= Capacity
EveryPermitCanBeWoken == permits \subseteq members
=========================================================================
