# Publishing to crates.io

Simple Task Manager is published to [crates.io](https://crates.io/crates/simpletaskmgr)
so users can install it without a source checkout:

```bash
cargo install simpletaskmgr                       # latest stable
cargo install simpletaskmgr --version 1.0.0-rc.1  # a specific prerelease
```

`cargo install` compiles from source, so the installing machine needs the
build-time [Requirements](../README.md#requirements) (Rust 1.75+ and the GTK 4.18+
dev headers). The crate ships source only — no binaries.

There are two ways a version gets published:

- **Automated (CI):** the `publish-crate` job in
  [.github/workflows/release.yml](../.github/workflows/release.yml) runs on every
  `v*` tag and publishes using the repository secret `CARGO_REGISTRY_TOKEN`.
- **Manual (you do it):** follow the steps in
  [Manual publish](#manual-publish). This is the path to use when you want to
  publish a version right now, or when the CI secret isn't set yet.

Both use the exact same `cargo publish` invocation, so the result is identical.

---

## Prerequisites

- A crates.io account for the owner (`paulez`), with **two-factor
  authentication enabled** — crates.io requires 2FA before any token can be
  created and before the first publish.
- A release version already decided in `Cargo.toml` (`version = "…"`),
  SemVer-shaped: `X.Y.Z` (e.g. `1.0.0`) or a prerelease (`1.0.0-beta.3`,
  `1.0.0-rc.1`). Every prerelease sorts below the final version, so the ladder
  `1.0.0-beta.3 → 1.0.0-rc.1 → 1.0.0` works.
- The crate name `simpletaskmgr` is not yet taken (verified: the crates.io API
  returns `crate 'simpletaskmgr' does not exist`). Reserve it by publishing the
  first version.

`Cargo.toml` already carries the fields crates.io wants: `description`,
`license = "MIT OR Apache-2.0"`, `repository`, `keywords`, and `categories`.
There is no `include`/`exclude`, so the package is the usual "everything that's
not git-ignored" set — inspect it with `cargo package --list`.

---

## Getting a publish token

1. Sign in at https://crates.io/ and confirm your account has 2FA on.
2. Open https://crates.io/settings/tokens and **Add token**.
3. Give it a scope:
   - **Type:** `Publishing`
   - **Crate:** `simpletaskmgr` (least privilege; you can broaden later), or
     `All` if you'll own the account going forward.
4. **Copy the token immediately** — it is shown only once. It never appears in
   the API, so there is no way to retrieve it again after closing the page.

Keep the token out of shell history and the repo. You'll hand it to `cargo`
either via `cargo login` (recommended for manual) or the `CARGO_REGISTRY_TOKEN`
environment variable (what CI uses).

---

## Manual publish

Run from a **clean** working tree — `cargo package` (which `cargo publish`
runs first) refuses to proceed if there are uncommitted changes, and the version
must match what you mean to ship.

```bash
# 0) make sure the tree is clean and the gate is green
git status --short                       # must be empty
cargo test && cargo clippy --all-targets && cargo fmt --check

# 1) confirm what would actually be published
cargo package --list

# 2) log in once (stores the token in $CARGO_HOME/credentials, not your history)
cargo login          # paste the token from step "Getting a publish token"
#    (equivalent non-interactive form: CARGO_REGISTRY_TOKEN=… cargo … )

# 3) dry-run — builds the .crate and prints what would happen, publishes nothing
cargo publish --dry-run

# 4) publish — the exact command CI uses
cargo publish --verbose

# 5) verify
curl -sS -A "simpletaskmgr (build tooling)" \
  https://crates.io/api/v1/crates/simpletaskmgr/versions
# or: https://crates.io/crates/simpletaskmgr/versions
# and: cargo install simpletaskmgr --version 1.0.0-rc.1   (proves it resolves)
```

If you'd rather keep the token only in the shell for this one command:

```bash
export CARGO_REGISTRY_TOKEN="your-token"
cargo publish --verbose
unset CARGO_REGISTRY_TOKEN
```

### Automated publish (for reference — what CI runs)

The `publish-crate` job is, in effect, this:

```bash
# env: CARGO_REGISTRY_TOKEN = ${{ secrets.CARGO_REGISTRY_TOKEN }}
cargo publish --verbose
```

To have CI do it instead of you, set the repository secret:

1. GitHub → repo → **Settings → Secrets and variables → Actions → New repository secret**.
2. **Name:** `CARGO_REGISTRY_TOKEN`
3. **Value:** the token from [Getting a publish token](#getting-a-publish-token).

Then tagging + pushing (`git tag v1.0.0 && git push origin main --tags`) runs
both the GitHub release and the crates.io publish. The `release` job fails loudly
if the secret is missing, so you'll always know which path actually published.

---

## Version rules (important)

- **Versions are immutable.** Once `1.0.0-rc.1` is published, you can never
  push different contents under the same version again. Fix the bug, bump the
  version (`1.0.0-rc.2`), and re-publish — don't try to overwrite.
- **Versions must increase.** A lower version than one already published is
  rejected.
- **Revoke, don't delete:** if a published version is broken, `yank` it so
  `cargo install` won't resolve it by default (it can still be installed with an
  explicit `--version`):

  ```bash
  cargo yank simpletaskmgr --version 1.0.0-rc.1
  # to reverse: cargo yank simpletaskmgr --undo --version 1.0.0-rc.1
  ```

- **Prereleases need an explicit `--version`** on `cargo install`; the bare
  command only resolves stable `X.Y.Z`.

---

## Troubleshooting

| Symptom | Cause / fix |
|---|---|
| `error: API request to https://crates.io/api/v1/crates/simpletaskmgr/owners/… failed: 403` | Token missing, expired, or not a **Publishing** type; recreate it and `cargo login` again. |
| `error: 1 files in the working directory contain changes that were not yet committed` | Commit (or `git stash`) first; publish from a clean tree. Add `--allow-dirty` only if you truly mean it. |
| `error: `simpletaskmgr 1.0.0-rc.1` is already published` | That version is taken. Bump to a new higher version (see [Version rules](#version-rules-important)). |
| Name already taken by someone else | The only recourse is to contact crates.io support; otherwise publish under a different name (update the `[package] name` and the `cargo install` examples). |
| `cargo install` fails for users on an old distro | `cargo install` builds from source, so they need GTK 4.18+ headers. Recommend the [AppImage](../README.md#installing-a-release-build) for people who just want to run it. |
