# Scheduled database backup

The backup runner belongs to virtualCrypto3. `.github/workflows/backup.yml` tests pull requests without secrets and deploys changes on main to the existing `virtualcrypto-backup` Fly app. There is no separate approval step. A manual workflow dispatch on main can redeploy or retry an update that encountered a running backup.

The trust boundary is preservation of existing backups, not isolation of database reads from vc3. A compromised vc3 deployment can already damage the live database. A compromised backup deployment can disclose database data, stop future backups, or submit bogus data. It must not gain authority to change the independent upload gate or delete retained backups.

## Separation

- This directory owns the Fly image and its deployment. The app runs once per day, dumps PostgreSQL 18, encrypts with the embedded age public recipient, uploads to the independent gate, then exits.
- [virtualcrypto-backup](https://github.com/virtualCrypto-discord/virtualcrypto-backup) owns the Cloudflare Worker: authentication, size/operation limits, inventory, and independent freshness/capacity notifications.
- R2 lifecycle and the 30-day deletion lock are managed separately. Neither vc3 Actions nor the Fly app receives R2 or Cloudflare administration credentials.
- The age private key stays outside GitHub, Fly, and Cloudflare.

## Deployment and secrets

The GitHub `backup` environment permits only the main branch and holds `FLY_BACKUP_API_TOKEN`, a deploy token scoped to the backup Fly app. This token can replace code that uses the app's secrets: **database disclosure through a compromised deployment is an accepted risk**. Direct database passwords, WireGuard keys, Worker deployment tokens, R2 keys, and the age private key are not stored in GitHub.

Fly app secrets are `PGPASSWORD`, `MTLS_CLIENT_CERT_B64`, `MTLS_CLIENT_KEY_B64`, and `DISCORD_WEBHOOK_URL`. The existing SELECT-only database role is retained as an accidental-write safeguard, not as protection against vc3 compromise. It needs SELECT on current tables/sequences and future tables/sequences owned by `web`.

The workflow updates only the existing stopped Machine by immutable image digest. It neither creates Machines nor starts an extra backup. Daily scheduling, restart policy `no`, 256 MiB memory, 2 GB ephemeral rootfs, no public services, and no volumes are checked before and after the update. If the daily job is running, deployment fails safely and can be retried. `deploy/machine.json` records the initial deployment for disaster recovery; routine updates preserve the live configuration. Keep the previous image printed in the deployment log for rollback.

The deploy token expires after one year; replace the GitHub environment secret before expiry. Fly's periodic job keeps running if the CI token expires.

## Bounds, monitoring, and restoration

The dump and ciphertext are limited to 32 MiB; the process exits after ten minutes. The independent Worker reserves at most three attempts/day and 1 GiB total, with at most 128 cleanup HEAD requests/day. Existing objects are locked for 30 days and expire after 35 days. Operators must not reset the Worker's ledger while its objects remain.

Fly sends job failures to Discord. The independent Worker reports more than 30 hours without a successful upload, or capacity exhaustion, at most once per UTC day. A forged upload can fool freshness monitoring; periodic restore verification is still necessary.

Initial verification restored all 28 public tables from an R2-downloaded, checksum-verified, decrypted archive into an isolated PostgreSQL 18 container. Restore with `age -d -i /safe/identity.txt -o backup.dump backup.dump.age`, then `pg_restore --no-owner --no-acl --exit-on-error` into a disposable database. Never test restoration over production.

R2 remains within its free allocation for this workload when that allocation is available. Fly adds about $0.30/month for 2 GB stopped rootfs, plus short runtime and egress. These are bounded-workload estimates, not account-wide spending caps. No automatic quota, VM size, or plan increases are allowed.

## mTLS upload authentication

Uploads go only to `https://vc-backup.tignear.com`. Fly secrets contain the client certificate and private key as base64-encoded PEM (base64 is an encoding, not encryption). The runner loads them into a default verifying TLS context from permission-0600 temporary files and immediately removes those files. A missing or invalid certificate/key aborts before `pg_dump`. Server certificate verification remains enabled, and redirects are rejected rather than forwarding the backup to another host. There is no upload bearer token.

The independent Worker checks Cloudflare's client-certificate verification and an allowed SHA-256 fingerprint. The client certificate expires on 2041-10-18 UTC; coordinate renewal with the Worker allowlist before removing/revoking the old certificate. GitHub receives only the app-scoped deployment token, not the certificate or key directly. Disclosure via replaced Fly code remains an accepted risk.
