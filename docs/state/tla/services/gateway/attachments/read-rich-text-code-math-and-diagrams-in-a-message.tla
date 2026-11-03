---- MODULE RichTextRendering ----
EXTENDS Naturals, FiniteSets
VARIABLES generation, streaming, mounted, pending, active, quarantine, rendered, failed, published
vars == <<generation, streaming, mounted, pending, active, quarantine, rendered, failed, published>>
Init == /\ generation = 1 /\ streaming = TRUE /\ mounted = TRUE /\ pending = {} /\ active = 0 /\ quarantine = 0 /\ rendered = 0 /\ failed = 0 /\ published = 0
Wanted(owner) == mounted /\ owner = generation
View == IF streaming THEN "Generating" ELSE IF rendered = generation THEN "Diagram" ELSE IF failed = generation THEN "Fallback" ELSE "Generating"
CloseFence == /\ mounted /\ streaming /\ streaming' = FALSE /\ UNCHANGED <<generation,mounted,pending,active,quarantine,rendered,failed,published>>
QuietSource == /\ mounted /\ generation \notin pending /\ active # generation /\ quarantine # generation /\ pending' = pending \cup {generation} /\ UNCHANGED <<generation,streaming,mounted,active,quarantine,rendered,failed,published>>
Replace == /\ mounted /\ generation = 1 /\ generation' = 2 /\ streaming' = TRUE /\ published' = 0 /\ UNCHANGED <<mounted,pending,active,quarantine,rendered,failed>>
CancelQueued == \E owner \in pending: /\ ~Wanted(owner) /\ pending' = pending \ {owner} /\ UNCHANGED <<generation,streaming,mounted,active,quarantine,rendered,failed,published>>
Dequeue == \E owner \in pending: /\ active = 0 /\ quarantine = 0 /\ Wanted(owner) /\ pending' = pending \ {owner} /\ active' = owner /\ UNCHANGED <<generation,streaming,mounted,quarantine,rendered,failed,published>>
QuarantinedRefusal == \E owner \in pending: /\ active = 0 /\ quarantine # 0 /\ Wanted(owner) /\ pending' = pending \ {owner} /\ failed' = owner /\ published' = owner /\ UNCHANGED <<generation,streaming,mounted,active,quarantine,rendered>>
RenderSuccess == /\ active # 0 /\ rendered' = (IF Wanted(active) THEN active ELSE rendered) /\ failed' = (IF Wanted(active) THEN 0 ELSE failed) /\ published' = (IF Wanted(active) THEN active ELSE 0) /\ active' = 0 /\ UNCHANGED <<generation,streaming,mounted,pending,quarantine>>
RenderFailure == /\ active # 0 /\ failed' = (IF Wanted(active) THEN active ELSE failed) /\ published' = (IF Wanted(active) THEN active ELSE 0) /\ active' = 0 /\ UNCHANGED <<generation,streaming,mounted,pending,quarantine,rendered>>
Timeout == /\ active # 0 /\ quarantine' = active /\ failed' = (IF Wanted(active) THEN active ELSE failed) /\ published' = (IF Wanted(active) THEN active ELSE 0) /\ active' = 0 /\ UNCHANGED <<generation,streaming,mounted,pending,rendered>>
HungRenderSettles == /\ quarantine # 0 /\ quarantine' = 0 /\ UNCHANGED <<generation,streaming,mounted,pending,active,rendered,failed,published>>
Unmount == /\ mounted /\ mounted' = FALSE /\ published' = 0 /\ UNCHANGED <<generation,streaming,pending,active,quarantine,rendered,failed>>
Next == CloseFence \/ QuietSource \/ Replace \/ CancelQueued \/ Dequeue \/ QuarantinedRefusal \/ RenderSuccess \/ RenderFailure \/ Timeout \/ HungRenderSettles \/ Unmount
NoIncompleteReveal == streaming => View = "Generating"
NoStaleReveal == View = "Diagram" => rendered = generation
QuarantineOwnsRenderer == quarantine # 0 => active = 0
RetiredNoPublication == ~mounted => published = 0
CurrentRenderWinsFailure == (~streaming /\ rendered = generation /\ failed = generation) => View = "Diagram"
PublicationIdentity == published = 0 \/ (mounted /\ published = generation)
Spec == Init /\ [][Next]_vars
====
