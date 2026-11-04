# Contributing to RIRRd

RIRRd is MIT-licensed. Use, modify and redistribute it, including commercially,
with the license notice preserved. Dependencies and upstream material retain
their own licenses. Development and issue tracking are hosted in the RIRRd GitHub organization.

Follow the [community conduct policy](CODE_OF_CONDUCT.md).

AI tools, agents and AI-assisted contributions are welcome. Review and understand the submitted code, test it through normal interfaces, and follow [the AI disclosure](docs/AI_DISCLOSURE.md). Do not submit unverified generated output or private prompts/credentials.

The project aims for full IRRd compatibility in one efficient Rust daemon,
practical on smaller hosts. Resource profiles tune budgets; they are not a
separate product with fewer features. Authentication, source isolation, durable
updates and correct routing policy take priority over benchmark gains.

Use a focused `feature/`, `fix/` or `release/` branch, preserve local work and submit
reviewable changes. Do not push directly to main or force-push shared branches.
Read the compatibility matrix and checkpoint before claiming a feature works.
Use the pinned official IRRd oracle for behavior comparisons and record exact
commits, fixture hashes, commands and results. Local results and passing CI
are distinct evidence.

Before submitting Rust changes, run `cargo fmt --all -- --check`,
`cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked
--all-targets` and `cargo audit`. Run relevant oracle and integration checks.
Performance reports must include memory, storage, throughput, tail latency,
resource limits and failed profiles. Preserve slower results and trade-offs.

Use synthetic or license-cleared fixtures. Keep production registry dumps,
credentials, keys, caches, databases and benchmark runtime data out of Git.
Never send test updates to public registries. Keep development listeners on
loopback or an isolated container network, separate from the public website.

Documentation, router integration tests, accessibility work, packaging and
small-host measurements are useful contributions. Community node listings
require confirmed operator consent and verification; sponsorship is not a
substitute for source policy or security review. See
[the project direction](docs/PROJECT_DIRECTION.md) and
[community network plan](docs/community/NETWORK.md).
