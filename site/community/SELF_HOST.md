# Your instance. Your infrastructure.

RIRRd is the independently self-hostable Rust IRR daemon with a bundled console
for lightweight IRR hosting. The community website is a separate
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

Project code is MIT licensed, with source and issue tracking in the RIRRd GitHub
organization. The daemon is independent of the public community service. These
commands build from source; tagged release archives are listed separately.

```sh
git clone https://github.com/RIRRd/RIRRd.git
cd RIRRd
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
from idle queries. Use a bounded cache and only the sources you need. RIRRd does not bundle an
RPKI validator or RTR service. See [the small-host guide](https://github.com/RIRRd/RIRRd/blob/main/docs/operations/SMALL_HOSTS.md)
and [beta status](https://rirrd.co.uk/docs/beta/) for limits and open work.
