# Beta publication

Source version: `0.1.0-beta.1`. Release notes: `docs/releases/v0.1.0-beta.1.md`.
Use a reviewed PR into protected main; never force-push main or bypass a required
review/check. The branch is `release/v0.1.0-beta.1`, with focused source commits.

The SSH deploy key can push source branches and tags, but cannot administer the
repository, inspect private Actions or change visibility. An authenticated GitHub
account with the appropriate repository/admin permissions is required for those
operations. Authenticate on the VM with `gh auth login`, or connect GitHub; never
paste an access token into an issue, file, command transcript or chat.

Read-only access/preflight:

```sh
python3 tools/release/publication-status.py
```

After the review and required checks, merge through GitHub. Enable Issues and
private vulnerability reporting, inspect the public tree/history scan and confirm
that the default branch contains the reviewed beta. The owner has authorized public
visibility and a beta release. An authenticated administrator can then perform:

```sh
gh repo edit Peravix/Rirrd --enable-issues \
  --visibility public --accept-visibility-change-consequences
```

Confirm the public repository API is readable without credentials before changing
`site/project/release.json` and source-availability wording. Repository visibility
must not be claimed from a successful SSH push. Do not expose any runtime cache,
queue, key, database or local backup in Git or release assets.

Push the matching `v0.1.0-beta.1` tag only at the verified release commit. The beta
workflow checks the tag/version, format, strict Clippy, tests, audit and publication
filenames, then builds/installs Debian and RPM packages on native x86-64/ARM64
runners. Only after those jobs pass does it create an explicit GitHub prerelease
with four native archives and SHA256SUMS. No latest/stable release is implied.
Verify the resulting workflow/release URLs through authenticated GitHub access.

Obsolete AI-named branches may be removed after verifying every tip is preserved
in the pushed release branch/tag. Keep an administrator-only Git bundle before
cleanup; no history rewrite is needed. Historical compatibility docs retain their
exact tested commits and do not become new release evidence automatically.
