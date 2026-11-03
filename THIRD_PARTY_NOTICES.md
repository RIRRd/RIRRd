# Third-party notices

RIRRd project code is MIT licensed. Dependency licenses remain their own.

Integrated RPKI certificate and signed-object validation uses **NLnet Labs
Routinator 0.15.3** as an embedded Rust library, with its `rpki` library for RTR.
Routinator's BSD 3-Clause notice is retained in
[licenses/ROUTINATOR-BSD-3-CLAUSE.txt](licenses/ROUTINATOR-BSD-3-CLAUSE.txt).
This project does not claim to have independently rewritten Routinator's
cryptographic validator. Official source: https://github.com/NLnetLabs/routinator.
The exact dependency graph and checksums are pinned in Cargo.lock.

The validator is optional at runtime. Its initial integration uses RRDP with
rsync disabled, private loopback RTR and explicit dataset expiry. Complete
Routinator feature parity and integrated production qualification remain open.
