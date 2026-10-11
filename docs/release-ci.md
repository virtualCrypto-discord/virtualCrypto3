# Fly.io release CI: research and rehearsal

Research date: 2026-10-11. The recommended pipeline is **CI → build once →
deploy to staging → smoke checks → promote the same image digest to production**.
Recovery restores recorded working Machine images and their runtime configuration.
The workflows implement this pipeline after merge to `main`; production remains
an explicit manual promotion.

## Running the workflows

Pushes to `main` (or manually running `ci` on `main`) run all checks, build once,
deploy the recorded digest to staging and execute the smoke tests. A successful
run publishes `staging-release-ATTEMPT` for 30 days and prints its CI run ID,
source SHA and digest in the workflow summary. A rerun has its own artifact name;
promotion always resolves the selected run's latest attempt.

Promote using Actions → **release** → Run workflow on **main**, operation
**promote**, and the successful **CI run ID**. Equivalent CLI:

```sh
gh workflow run release.yml --ref main -f operation=promote -f run_id=CI_RUN_ID
```

The resolve job accepts only successful main push/manual CI runs from this
repository, with matching artifact run/attempt/source metadata and successful
staging verification. It never accepts an arbitrary image input. Production
uses the candidate revision's `fly.toml`, the exact verified staging digest and
the release tools from the workflow's main revision. The image is copied into
`registry.fly.io/virtualcrypto-prod` with `skopeo --preserve-digests --all`, then
the destination digest is checked before deployment. No production rebuild runs.
Both destinations serialize deployments without cancellation.

The `production` GitHub environment needs `FLY_PRODUCTION_API_TOKEN`, an app-scoped
deploy token for `virtualcrypto-prod`. The existing `staging` environment uses
`FLY_STAGING_API_TOKEN`. Production also holds `FLY_STAGING_IMAGE_TOKEN`, a separate
staging-scoped token used only to read the source image during the copy. Restrict
both environments to branch `main`. Production
is manually triggered; no additional reviewer gate is required by the workflow.
Create/import a token without printing it, using the same lifetime policy as
the existing staging token:

```sh
set -o pipefail
flyctl tokens create deploy --app virtualcrypto-prod \
  --name github-actions-production-release --expiry 175200h |
  gh secret set FLY_PRODUCTION_API_TOKEN --env production
flyctl tokens create deploy --app virtualcrypto-staging \
  --name github-actions-production-image-source --expiry 175200h |
  gh secret set FLY_STAGING_IMAGE_TOKEN --env production
```

Direct cross-app image lookup with the production app-scoped token failed in
the live check (`Not authorized to access this imageref`). Separate source and
destination registry credentials retain app-scoped authorization while allowing
the digest-preserving copy. Temporary Docker credentials are deleted after the
copy; tokens are never passed on the command line or retained in release artifacts.
The copy trusts the already verified CI digest over TLS, without requiring an
image signature. See [skopeo copy](https://github.com/containers/skopeo/blob/main/docs/skopeo-copy.1.md).

Each production attempt preserves `production-release-ATTEMPT`, including the
previous Machine images/configuration, app configuration, secret fingerprints
(never values), candidate metadata and observed Machine versions. It is retained
for 30 days, including after failed rollouts. Keep records elsewhere if recovery
must remain available longer; Fly registry image retention is also not guaranteed.

For a compatible schema, recover the state **before** a particular promotion:

```sh
gh workflow run release.yml --ref main -f operation=recover -f run_id=PRODUCTION_RELEASE_RUN_ID
```

Recovery uses that production run's snapshot, not a CI run. It rejects changed
Machine versions, Machine membership or secret fingerprints. It restores each
Machine's exact prior digest, startup command, environment and services through
a complete [Machines API configuration update](https://docs.fly.io/machines/api/machines-resource/#update-a-machine),
with a lease and version guard. It verifies the public root and restores any
previously stopped Machines to stopped state. This also supports the legacy
`/app/bin/server` command, whose app has no `/health` route.

Recovery does not run migrations or Discord command registration, restore data,
revert secret values, or rewind the app's release history/declarative config.
The saved `fly.before.json` records that declarative config for reconciliation.
Check old-code/new-schema compatibility before requesting recovery; restore the
compatible Discord command list separately if registration changed it. A run
cancelled before recording Machine versions/uploading artifacts may require
manual recovery from the logs and the before snapshot.

## Recommendations and evidence

| Area | Recommendation for this repository | Official source |
| --- | --- | --- |
| Authorization | Separate app-scoped deploy tokens in GitHub `staging` and `production` environments; `contents: read`. Choose a practical token expiry and rotation policy. The existing staging token uses the documented 20-year default; changing it is a separate operational decision. | [Fly access tokens](https://docs.fly.io/security/tokens/), [GitHub secure use](https://docs.github.com/en/actions/reference/security/secure-use) |
| Release ordering | Require all CI jobs to pass for the exact revision. Serialize each destination with `cancel-in-progress: false`, so a newer commit cannot interrupt migrations or Machine updates. Restrict production to the selected release revision. | [Fly GitHub Actions deployment](https://docs.fly.io/launch/continuous-deployment-with-github-actions/), [GitHub deployments](https://docs.github.com/en/actions/how-tos/deploy/configure-and-manage-deployments/control-deployments) |
| Artifact identity | Give each build a unique revision/run tag, record its digest and source SHA, then use `--image registry.fly.io/…@sha256:…` for promotion. Staging and production are in the same Fly organization, which supports image reuse across apps. Pin flyctl and Actions; use `cargo build --locked` and consider base-image digests when implementing the pipeline. | [Fly registry](https://docs.fly.io/blueprints/using-the-fly-docker-registry/), [GitHub secure use](https://docs.github.com/en/actions/reference/security/secure-use) |
| Migrations | Keep `/app/bin/release` before rollout. A nonzero exit prevents Machine updates, but completed database changes and external Discord registration are not undone. Review old/new schema compatibility for each release. | [Fly configuration](https://docs.fly.io/reference/configuration/), [Seamless deployments](https://docs.fly.io/blueprints/seamless-deployments/) |
| Traffic readiness | Configure service-level HTTP checks in both environments. `/health` confirms the listener, not an ongoing SQL connection; additionally smoke-test the public currency lookup, SPA assets and authentication/signature rejection. | [Fly health checks](https://docs.fly.io/reference/health-checks/) |
| Rollout | Staging currently has one Machine and uses rolling updates. For production, rolling with at least two running Machines is a straightforward option; bluegreen can temporarily duplicate capacity for a stateless app. Verify concurrent background workers and request draining before claiming zero downtime. | [Fly deployment strategies](https://docs.fly.io/reference/configuration/#picking-a-deployment-strategy), [Seamless deployments](https://docs.fly.io/blueprints/seamless-deployments/) |
| Recovery | Capture the previous image, config, Machine inventory and secret names before deploying. Redeploy the previous digest and verify service behavior. Image recovery does not restore database schema, secrets or config; Fly registry retention is not indefinite. | [Fly rollback guide](https://docs.fly.io/blueprints/rollback-guide/) |

The Fly OIDC documentation describes **Machines authenticating to external cloud
providers**, rather than GitHub Actions exchanging a token for Fly deploy access.
Use the documented app deploy token for this pipeline.
See [Fly OIDC](https://docs.fly.io/security/openid-connect/).

## Repository findings

The existing `.github/workflows/ci.yml` already gates staging deployment on
`review-workbench`, `check` and `web`, uses a staging environment secret, serializes
deployments without cancellation, and pins flyctl to `0.4.97` and its setup Action
to a commit. Both application configs run migrations followed by Discord global
command registration. Staging uses a separate database and Discord application.

The implementation adds service-level HTTP checks to production, records exact
artifact identity and snapshots, runs smoke tests after deployment, and provides
manual promotion/recovery. Production `fly.toml` now matches the existing `lax`
region and 512 MiB memory, explicitly runs `/app/bin/vc-server` in process group
`app`, and keeps the app running. Docker's Cargo build uses `--locked`.

Before the initial production cutover, resolve these remaining operational points:

- Production's `DATABASE_URL` points to `virtualcrypto-db-prod-pg18.flycast`,
  database `virtualCrypto`, user `web`. An initial `Staged` status was misleading:
  fingerprints of the running value and the current vault value matched, and
  the running Ecto repository already used that database. `fly secrets deploy`
  synchronized the deployment record while retaining the current image/runtime
  configuration. All secrets now report `Deployed`; a read-only SQL query after
  restart confirmed PostgreSQL 18.6 and the same database. The preflight still
  rejects genuine pending/partial/unknown secrets and missing required secrets;
  this specific discrepancy has been resolved.
- Handle schema compatibility explicitly. Migration `0025` drops
  `browser_sessions`, so arbitrary older Rust images cannot safely be assumed to
  work with today's schema. Baseline CI checks the original Ecto schema; that is
  not a rehearsal on a restored copy of the current production database.
- Production has one Machine and currently uses rolling updates with
  `--ha=false`, preserving its existing capacity. This can interrupt traffic.
  The server has no graceful shutdown. Adding multiple Machines/bluegreen and
  evaluating request draining/background workers is separate availability work;
  this pipeline does not promise zero downtime.

The staging rehearsal below tests the current Rust release on an already migrated
staging database. It does not prove the initial production cutover, backup restore,
Discord user journeys, or production traffic continuity.

## Repeatable rehearsal

Use flyctl `0.4.97`, matching CI. Keep all mutations explicitly scoped to staging.
Start from a revision whose CI has passed, with no uncommitted application changes.

1. Record `flyctl status --app virtualcrypto-staging --json` and
   `flyctl releases --app virtualcrypto-staging --json`. Preserve each deployed
   Machine's full image digest and configuration, not just the latest release:
   the latest release can be a failed attempt.
2. Run `flyctl config validate --config fly.staging.toml --strict`, then warm up
   and check the existing app:

   ```sh
   python3 scripts/fly-smoke.py https://virtualcrypto-staging.fly.dev
   ```

3. Build and push without changing application Machines. Use a unique tag for
   each attempt; capture the manifest digest printed by the builder:

   ```sh
   flyctl deploy --config fly.staging.toml --app virtualcrypto-staging \
     --remote-only --build-only --push --image-label "$REHEARSAL_TAG" \
     --label "org.opencontainers.image.revision=$SOURCE_SHA" --yes
   ```

4. Copy the staging config into a temporary file and replace only its
   `release_command` with `/bin/false`. Deploy the candidate digest with that
   config, `--app virtualcrypto-staging --ha=false --deploy-retries 0 --yes`.
   Expect a nonzero exit **and a release-command failure in the logs**. Verify
   that the serving Machine ID, Machine version and image digest are unchanged,
   and that the smoke checks still pass. Remove the temporary config afterward.
5. Deploy normally using the original config:

   ```sh
   flyctl deploy --config fly.staging.toml --app virtualcrypto-staging \
     --image "$CANDIDATE_IMAGE" --ha=false --deploy-retries 0 --yes
   python3 scripts/fly-smoke.py https://virtualcrypto-staging.fly.dev
   flyctl checks list --app virtualcrypto-staging
   ```

   Confirm the release command completes, Discord registration returns `200`,
   and the Machine's actual digest equals the candidate digest. A package version
   in `/health` is insufficient to identify a particular deployment.

   Also test the sleeping-database case: stop the staging application and then its
   database using their recorded IDs, verify both are stopped, and repeat this
   digest deployment without HTTP warm-up first. Run the smoke checks afterward.
   A deploy to an already stopped app can update its image while leaving it
   stopped, so a successful deploy exit alone does not prove serving readiness.

6. For a known compatible image, rehearse recovery without rebuilding:

   ```sh
   flyctl deploy --config fly.staging.toml --app virtualcrypto-staging \
     --image "$PREVIOUS_IMAGE" --skip-release-command --ha=false \
     --deploy-retries 0 --yes
   python3 scripts/fly-smoke.py https://virtualcrypto-staging.fly.dev
   ```

   `--skip-release-command` is appropriate here because this rehearsal starts and
   ends on the same source revision, with no schema or command changes. It avoids
   old migration-checksum failures and another Discord overwrite. For an actual
   release with changed commands, restore the compatible Discord command list
   separately; never assume that skipping the release command undoes it.
7. Compare final Machine count and image digest with the starting inventory.
   If the app and database started stopped, stop the app and then its staging DB
   after checks, using their recorded Machine IDs. Autostart remains enabled.

The smoke script checks health, SPA HTML/cache policy, every referenced built
asset, a nonexistent currency lookup through PostgreSQL, missing authentication
(`400 invalid_request`), and an unsigned Discord PING (`401`). It needs no secrets
and makes no business-data writes. It does not simulate a signed Discord request
or an authenticated currency transaction.

## Rehearsal results: 2026-10-11

Source revision: `e81b33a999bc4b72003878af33464d7c47c0bb14` (current main).
Its existing [CI run](https://github.com/virtualCrypto-discord/virtualCrypto3/actions/runs/37207113303)
passed all jobs, including staging deployment. The rehearsal used the same
flyctl `0.4.97` as CI, downloaded from its official release with its published
SHA-256 verified. Application source was unchanged; the new smoke script and
documentation are excluded from the Docker build context.

| Check | Observed result |
| --- | --- |
| Config validation | Both application configs passed `flyctl config validate --strict`. |
| Initial stopped state | App `87e175c025e228` and DB `811d676a423318`, both in `iad`, were stopped. HTTP warm-up woke staging successfully. |
| Baseline smoke | All six checks passed, including both built assets and the PostgreSQL lookup. |
| Remote build/push | Candidate tag `rehearsal-e81b33a-20261011` built successfully (34 MB); no application Machine changed during the build. |
| Release failure gate | Release v10 failed with `/bin/false`, exit 1. Serving Machine ID, version and image digest were unchanged; all smoke checks still passed. The temporary release Machine was destroyed. |
| Normal deployment | Release v11 completed; migration command succeeded and Discord registration logged `200`. Candidate digest and revision label matched the serving Machine. HTTP service check and smoke checks passed. |
| Image recovery | Release v12 redeployed the original digest with `--skip-release-command`. Original Machine ID/count, runtime config and digest matched the starting inventory; service and smoke checks passed. |
| Migration inventory | `sqlx migrate info` reported all migrations 1–28 installed. This was an already migrated database, so no new migration/data conversion was rehearsed. |
| Deployment with sleeping DB | Both Machines were explicitly stopped and their states verified. Release v13 succeeded without prewarming; Flycast started the DB for `/app/bin/release --connect-timeout 60`. Subsequent HTTP smoke checks woke the app and passed against the candidate digest. |
| Final restoration | Release v14 restored the original digest and runtime configuration again. All smoke checks and the service check passed; app and DB were then stopped, matching their initial state. Each app still has exactly one Machine. |

Original image:

```text
registry.fly.io/virtualcrypto-staging@sha256:1ecf6933a63746b1193805dfdd8c790896d68e61f10e42b86c16807b2cc2d98c
```

Candidate image:

```text
registry.fly.io/virtualcrypto-staging@sha256:af2616e575fb202a7f74011b75623eafcf5a92ea25b8b6ca37152e59a0a0df82
```

Detailed local logs and inventories are under
`.review-artifacts/release-rehearsal/` (Git-ignored). The production app was only
inspected; no production deploy, secret update or database operation was performed.
The rehearsal used local Fly authentication, not the GitHub environment's hidden
deploy token, and did not exercise a new production Actions workflow. The
implementation follow-up below verifies the app-scoped registry authorization.

## Implementation verification: 2026-10-11

- Eleven automated checks cover CI artifact provenance, rerun identity, unverified
  images, failed production recovery records, pending/missing secrets, stale
  snapshots (including config-only changes), restoration of legacy startup, and
  digest preservation/credential cleanup during registry copying.
  Both workflow files pass actionlint; both application configs pass strict Fly
  validation. Cargo metadata resolves offline with the lockfile unchanged.
- The Docker image builds and pushes with `cargo build --locked`. A local staging
  rehearsal runs the new snapshot/digest/verification/recovery helpers. Adding an
  explicit server CMD and then recovering restores the original empty init,
  original digest and runtime configuration. The CLI's config merge initially
  retained the newer CMD; the implementation now uses a complete Machines API
  update to restore it exactly.
- The production secret status discrepancy was investigated and synchronized,
  preserving the running legacy image. No Rust production deployment or
  production schema migration was performed.
- GitHub `staging` and `production` environments now allow only branch `main`.
  The production deploy and staging image-source tokens have been installed in
  the production environment. Live app-scoped tokens copied the staging image
  into the production registry with the identical digest, and a build-only
  lookup using the production token succeeded. Temporary test tokens were revoked.
- Detailed follow-up records are under `.review-artifacts/release-ci-verification/`
  and `.review-artifacts/release-ci-production-preflight/`, both Git-ignored.
