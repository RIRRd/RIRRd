# Third-party notices

RIRRd project code is MIT licensed. Dependency licenses remain their own. The exact graph and checksums are pinned in Cargo.lock. RIRRd does not bundle an RPKI validator or RTR server.

Native beta archives also include a generated `THIRD_PARTY_LICENSES.txt` inventory
and deduplicated texts for the selected Linux dependency graph. These are collected
from the locked source packages, not fetched at application runtime. See the
[package guide](deploy/packages/README.md) for the explicit Inflector clarification.
The BSD-2-Clause reference text in `licenses/SPDX-BSD-2-Clause.txt` comes from
SPDX license-list-data v3.27.0, `text/BSD-2-Clause.txt`; it is labeled as reference
text rather than a reconstructed upstream copyright notice.
