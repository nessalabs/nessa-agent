# Linux container cleanup verification

Round1 records the clean `937610ee85494b53e4591a3b3b2146118f864971` candidate against main `cffdabba64f821c012b2df04c2f02e5754511df3`. Four real container cases passed; two independent reviewers subsequently found malformed evidence could pass and image inspection missed cancellation. This historical evidence is not approval of that source or a later source. Corrected-head evidence will be recorded separately.

Artifacts contain developer fixture output and container-local identities; no live provider credentials or product UI changes. SDK production code was unchanged.

Round2 records corrected frozen source `9364794177684a846b9071f268b5b870b198036a` against unchanged main base. Required report types and process identities are validated before acceptance; image inspection observes cancellation. The actual four cases passed with the same explicit main-base SDK binary. 32 pure tests and 22 new independent mutation failures are recorded separately from the historical 32 probes. Fresh whole-diff review is pending at publication; no review approval or CI completion is claimed here.

Round3 records clean `d9ac477436605a5ac4b360d198aa7fedd830b1e0` against unchanged main base. One publisher arbitrates cancellation around acceptance writes after independent removal; negative cause proof identifies the selected test panic and exact cleanup error. The harness exited zero and all four real cases passed with a preserved byte-identical copy of the Cargo-selected SDK binary (`932566e1324fb80f9e30d0512e823d04d6c97b375af3a565306e39b88dd6c28d`). 37 restored pure tests and seven new mutation failures are recorded; older probe iterations remain distinct. Two fresh whole-PR reviewers are pending at this artifact publication.
