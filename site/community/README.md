# RIRRd community website

This is a separate product/deployment from the self-hosted Rust daemon.

- `web/dashboard/`: the daemon's independent, bundled query console. It does not
  ship community nodes, sponsorship data or a submission queue.
- `site/community/`: public node discovery, hosting/sponsor credits, public guides,
  software information and a private, reviewed node-submission workflow.
- The community deployment reuses the console assets with public-node metadata.
  The website is not required to run, administer or query your own instance.
- `nodes.json` is the reviewed public directory. The first node is UK 01, hosted
  in the United Kingdom by HYEHOST LLC. Hosting metadata was supplied by the owner;
  no city or financial sponsorship is inferred.

The public community domain is **irrd.tools**. **rirrd.co.uk** is the separate
project showcase, built from `site/project/`. Both have distinct SVG identities.

## Build and deploy

From a clean source checkout, run:

```sh
python3 site/project/build.py
python3 site/community/build.py
sudo sh site/deploy.sh
```

The build requires the distribution's Python Markdown package. Local previews
can use `--allow-dirty --output /private/preview`; published builds record a clean
commit in `build.json`. Only generated static assets are copied to the web root.
Caddy configuration is validated before reload and the existing config/site is
backed up under `/var/backups/rirrd-web/`. The registry daemon is not
restarted by website deployment. The existing read-only gateway supplies live
queries and telemetry on loopback; internal addresses are not public examples.

Services and paths:

- `caddy.service`: `/var/www/irrd-tools`, public HTTPS and HTTP redirect.
- `rirrd-live-gateway.service`: existing public query gateway on loopback 9281.
- `rirrd-community.service`: separate submission service on loopback 9282, under
  the unprivileged `rirrd-community` account, 64 MiB / 20% CPU ceiling.
- Submission database/rate key: `/var/lib/rirrd-community/`, mode 0700; SQLite
  and key mode 0600. These never enter Git or the web root.

## Node submissions and review

The website posts to `/community/node-submissions`. Successful requests receive
202 and a pending receipt. They do not publish a listing or send email. Operator
contact and reviewer notes remain private, with hourly deletion of records older than 90 days. The
service checks field bounds, public endpoint syntax, operator attestation,
request framing, exact origin, a per-client limit and a global pending limit.
IPv6 privacy addresses share a /64 rate bucket. Stored rate identifiers are
keyed hashes, not visitor IP addresses. The service **never fetches a submitted
endpoint**, and exposes no public queue/list/approval route.

Review through the private service account. For example:

```sh
sudo -u rirrd-community python3 /usr/local/lib/rirrd-community/service.py list
# Once, create a private working copy of the reviewed public directory:
sudo install -o rirrd-community -g rirrd-community -m 600 \
  site/community/nodes.json /var/lib/rirrd-community/reviewed-nodes.json
# After independently checking ownership, endpoints, location and credits:
sudo -u rirrd-community python3 /usr/local/lib/rirrd-community/service.py \
  review RECEIPT --decision approve \
  --directory /var/lib/rirrd-community/reviewed-nodes.json \
  --verification-note 'Describe the checks actually performed'
# Or: review RECEIPT --decision reject
```

Approval exports only public node fields. Contact email and review notes are
excluded. Copy reviewed directory changes into `site/community/nodes.json`,
review the Git diff, commit, rebuild and deploy. The private working directory
is independent of the public website; no unreviewed submission is auto-listed.
There is no payment integration or sponsorship contract implied by the form.

## Verification

```sh
python3 -m unittest discover -s site/community -p 'test_*.py'
python3 tools/check_community_ui.py --url https://irrd.tools/
```

The browser's `--submit-preview` option is only for an isolated temporary queue.
It must not submit dummy nodes to the real community service. The live check
covers the directory, query tools, source filtering, IPv4/IPv6 and recursive
AS-set examples, RPKI validation and responsive layouts.
