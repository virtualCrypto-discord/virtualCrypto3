# OAuth consent browser regression

This runs the real API router over TCP against an isolated PostgreSQL database,
with Playwright Chromium driving authorization and clicking the consent form.
Only Discord is replaced by a local identity-provider stand-in, on a different
origin. No request Origin, Referer, cookie or Fetch Metadata headers are supplied
by the test. It does not test Discord's login UI or contact live services.

The test covers public and Basic clients, missing challenges, native form POST
CSRF headers, client state round trips, missing/wrong/correct PKCE verifiers,
refresh rotation, and continued rejection of null origins. The client callback
also checks that cross-origin redirects do not leak the service callback URL.

The old consent `no-referrer` response fails with `Origin=null` and HTTP 403;
`same-origin` lets a valid approval succeed without weakening the CSRF check.

Run from the repository root, with the normal development database available:

```sh
npm --prefix tools/oauth-browser ci --include=dev
(cd tools/oauth-browser && npx playwright install --with-deps chromium)
SQLX_OFFLINE=true cargo test -p vc-api --test oauth2_browser -- --ignored --nocapture
```

The browser test is ignored by the ordinary Rust suite because it needs Node.js
and Chromium. CI explicitly installs them and runs it before staging deployment.
`OAUTH_CHROMIUM_EXECUTABLE` may point to an existing compatible browser executable
(or an environment-specific launcher, for example on Nix).
