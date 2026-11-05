# Beta publication

Canonical repository: **RIRRd/RIRRd**. Current candidate version:
`0.1.0-beta.4`; notes: `docs/releases/v0.1.0-beta.4.md`.
Use a reviewed PR into protected main. Never force-push main or bypass required
reviews/checks. The first organization repository is initialized from the
verified project history; subsequent changes use the normal protected PR path.

## Scoped access

Use a fine-grained token whose resource owner is **RIRRd**, or an installation
token from a GitHub App installed only in that organization. Repository
administration, contents, Actions, Issues, pull requests and workflow-write
permissions support publication. An SSH deploy key alone cannot administer a
repository or inspect private Actions. Do not use account-wide OAuth merely to
work in this organization. Keep credentials in a private secret store outside
Git, and never paste them into issues, command arguments or chat.

The GitHub CLI can use a scoped `GH_TOKEN` supplied by the operator's credential
helper. Read-only preflight:

```sh
python3 tools/release/publication-status.py --repo RIRRd/RIRRd --branch main
```

## Review, checks and visibility

Inspect the source tree/history audit and secret scans, license/notices, beta
limitations, issue forms and contribution/security policies. Confirm GitHub
checks at the exact candidate revision, then use the reviewed protected branch.
Enable Issues and private vulnerability reporting. The owner has authorized
public beta publication. An authorized administrator can change visibility:

```sh
gh repo edit RIRRd/RIRRd --enable-issues \
  --visibility public --accept-visibility-change-consequences
```

Confirm the repository API and clone are accessible without credentials before
updating any separately maintained website metadata. Inspect public issue forms
and their security-reporting links. Historical CI links in old checkpoints
belong to the original repository; current runs must link to RIRRd/RIRRd.
Keep runtime data, caches, private queues and credentials outside Git and assets.

## Tagged beta

Create a new matching `v0.1.0-beta.4` tag only at the verified release commit.
Preserve historical tags rather than moving them. Beta publication calls the
shared CI matrix, web, coverage, audit and workflow gates, plus pinned upstream
oracle, BIRD/FRR integration, Docker deployment and fuzz checks. The tag must be
contained in protected main. It then performs source
hygiene/tag checks and native x86-64/ARM64 DEB/RPM installation/systemd checks.
Only passing packages become a GitHub prerelease with four archives, the
installer and SHA256SUMS. DEB asset basenames replace `~` with `.` before hashing
because GitHub normalizes unsupported upload-name characters. The final upload
check compares every actual server asset name and SHA-256 digest with the manifest. A source version alone is not a published download or stable release.

After publication, verify the real release/artifact URLs and update the site
release-published flag. The public node's runtime build is tracked separately
from the latest software release. Static website publication must not restart
or reimport the running routing registry.
