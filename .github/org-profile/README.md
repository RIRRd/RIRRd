<p align="center"><img src="https://rirrd.co.uk/brand/rirrd-avatar.png" width="112" height="112" alt="RIRRd"></p>

# RIRRd

A lightweight Internet Routing Registry daemon, built in Rust. One daemon,
an embedded database and a local console: no PostgreSQL or Redis runtime is
required. Choose your source and cache budgets for your own host.

**Beta software. Full IRRd compatibility and production qualification remain
incomplete.** RIRRd focuses on IRR data, mirroring and queries. Cryptographic
RPKI validation is a separate service for operators to choose independently.

- [Source and README](https://github.com/RIRRd/RIRRd)
- [Beta releases and Linux packages](https://github.com/RIRRd/RIRRd/releases)
- [Issues and feature requests](https://github.com/RIRRd/RIRRd/issues)
- [Self-hosting documentation](https://rirrd.co.uk/docs/getting-started/)
- [Build verification](https://rirrd.co.uk/testing/)
- [Project and measured benchmarks](https://rirrd.co.uk/)

[irrd.tools](https://irrd.tools/) is the separate community network and node
directory. Its first experimental public node is in the United Kingdom,
hosted by **HYEHOST LLC**. Independently operated sponsored nodes can be
submitted for review through the community website.

## Open source and contributions

Project code is **MIT licensed**: use, modify and redistribute it.
Human and AI-assisted contributions are welcome. RIRRd is built with AI
assistance; every change still needs review, reproducible tests and provenance.
Read [the contribution guide](https://github.com/RIRRd/RIRRd/blob/main/CONTRIBUTING.md)
and [security policy](https://github.com/RIRRd/RIRRd/security/policy).

Report reproducible beta bugs and compatibility differences through the project
issue tracker. Keep private registry data and credentials out of reports.
