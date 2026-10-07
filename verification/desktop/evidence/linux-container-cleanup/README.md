# Linux container cleanup verification

Round1 records the clean `937610ee85494b53e4591a3b3b2146118f864971` candidate against main `cffdabba64f821c012b2df04c2f02e5754511df3`. Four real container cases passed; two independent reviewers subsequently found malformed evidence could pass and image inspection missed cancellation. This historical evidence is not approval of that source or a later source. Corrected-head evidence will be recorded separately.

Artifacts contain developer fixture output and container-local identities; no live provider credentials or product UI changes. SDK production code was unchanged.

Round2 records corrected frozen source `9364794177684a846b9071f268b5b870b198036a` against unchanged main base. Required report types and process identities are validated before acceptance; image inspection observes cancellation. The actual four cases passed with the same explicit main-base SDK binary. 32 pure tests and 22 new independent mutation failures are recorded separately from the historical 32 probes. Fresh whole-diff review is pending at publication; no review approval or CI completion is claimed here.
