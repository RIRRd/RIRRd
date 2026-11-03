# Your instance. Your infrastructure.

RIRRd is the independently self-hostable Rust IRR daemon with a bundled console
and an optional embedded RPKI validator. The community website is a separate
service for finding public nodes, crediting hosting partners and reviewing node
submissions. Your own instance does not depend on that service or a community
account. Listing your node is optional.

## Choose how you want to use RIRRd

- **Use a public service:** open the [community query tools](/dashboard/#query).
- **Run your own instance:** build the daemon, choose your own data sources and
  use its local console. You control storage, listeners and update policy.
- **Contribute a public node:** operate an instance and [submit its public
  endpoints and hosting credits](/#submit-node) for directory review.

## Source availability

Project code is MIT licensed. The repository is currently private during
  development; a public open-source release is planned. The build instructions
below require repository access until that release. There is no public package
or container download implied by these commands.

```sh
git clone git@github.com:Peravix/Rirrd.git
cd Rirrd
cargo build --locked --release
./target/release/rust-irrd --config irrd.toml config check
```

The daemon uses embedded storage for IRR data. PostgreSQL and Redis are not
runtime dependencies. The bundled console belongs to your instance and does not
include the community node directory, sponsorship workflow or submission queue.
The public website is deployed independently from `site/community/`.

## Start small, enable what you need

Choose a limited source set for a smaller host. Keep development listeners
private, use the configured source order and monitor import peaks separately
from idle queries. RPKI is optional: omitting `[rpki]` keeps the IRR-only profile.
Full-source imports and a cold global validator need more resources than a warm
query service. No whole-VM minimum is promised by the synthetic benchmark.

The validator uses the attributed NLnet Labs Routinator Rust library in the same
process. It does not require a separate Routinator daemon. RRDP-only collection,
rejected publication points and incomplete policy integration are reported in
the running console. Full IRRd and validator production qualification remain open.

[Development repository (currently private)](https://github.com/Peravix/Rirrd)

## Contribute

The project is built with AI assistance. AI tools and agents are welcome to
contribute; changes still require human review, reproducible tests and clear
license/provenance. Code can be used, modified and redistributed under the MIT
license, with dependency licenses retained. Imported registry data keeps its own
provenance.
