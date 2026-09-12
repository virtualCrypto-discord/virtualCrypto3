# OAuth2 and applications

**There is not one Elixir test for any of this.** `test/` has no oauth2, token or
client file, so unlike every other part of this migration there is no ported
spec to fall back on: the contract below was read out of the controllers, and
the tests will have to be written from the same reading. Treat it as design
work, not porting work.

## The endpoints

| Endpoint | Token it wants | Answers |
| --- | --- | --- |
| `POST /oauth2/clients` | `user` kind, `oauth2.register` scope | 201 with the new client, 401/403/400 |
| `GET /oauth2/clients/@me` | `user` kind, `oauth2.register` scope | the caller's applications |
| `GET /oauth2/clients/@me` | `app` kind, `oauth2.register` scope | one application |
| `PATCH /oauth2/clients/@me` | `app` kind, `oauth2.register` scope | 204, or 400 |
| `GET/POST /oauth2/authorize` | browser session | the consent screen — read below |
| `POST /oauth2/token` | client credentials | not read yet |
| `POST /oauth2/token/revoke` | client credentials | not read yet |
| `POST /token` | browser session | a VC API token — **implemented** |

Note the same path means different things by method: `GET /oauth2/clients/@me`
lists what the *user* owns, and the sibling `ClientController` answers the *app*
that is asking about itself. They are separate controllers in Elixir, and the
`kind` claim is what tells them apart.

## Registration (`POST /oauth2/clients`)

The order matters, and each step is a different refusal:

1. the token must be `kind: "user"` with `oauth2.register`, else
   `invalid_token` / `invalid_kind` (401) or `insufficient_scope` (403);
2. the owner's Discord authorization is refreshed (`DiscordAuth.refresh_user`);
3. Discord's `/users/@me` is consulted with that token, and **a bot account may
   not register** — `bot: true` is `user_verification_failed`;
4. the redirect URIs are checked, and only `http` and `https` schemes pass:
   `invalid_redirect_uri` / `redirect_uri_scheme_must_be_http_or_https`;
5. the webhook URL is verified by handshake — `webhook_verification_failed`
   otherwise. **This is the part that is not built**: the handshake goes through
   the Cloudflare Workers proxy with mutual TLS, which is the same missing piece
   the claim notifications need.
6. the application is created, and a fresh `app` token with the
   `oauth2.register` scope is issued as the registration access token.

The answer is:

```json
{
  "client_id": "...",
  "client_secret": "...",
  "registration_access_token": "...",
  "registration_client_uri": "<site_url>/oauth2/clients/@me",
  "client_secret_expires_at": 0
}
```

`client_secret_expires_at` of `0` is the dynamic-client-registration convention
for "never", and is not a bug.

## Where the rules actually live

Each of these was located, so the next reader does not have to look:

- **the public shape** of an application, shared by the list and the single read:
  `lib/virtualCrypto_web/controllers/oauth2/client_json.ex` and `clients_json.ex`.
  If this is wrong, both endpoints are wrong together.
- **registration**: `Auth.register_application/2` and the validation it runs,
  `lib/virtualCrypto/auth/internal/application.ex` — the `req` fetches and their
  errors (`redirect_uris_must_be_array`, and the scheme rule at line 60:
  `URI.parse(&1).scheme in ["http", "https"]`);
- **the metadata validator**: `application_metedata_validator.ex` (the spelling is
  the file's), which is where `client_uri`, `webhook_url` and `logo_uri` are each
  checked — the last of them allowing `data:` or `https`, and nothing else;
- **editing**: `application_patch_query_service.ex`, which repeats the same
  redirect-URI scheme rule at lines 141 and 165;
- **`Auth.RedirectUris` is only an Ecto schema** — no logic, so nothing to port
  from it; the rule above is the whole of it;
- **the code and its exchange**: `Auth.InternalAction.Util.make_secure_random_code/0`
  is 32 random bytes base64-url without padding, and the service delegates the
  real work to `lib/virtualCrypto/auth/internal/service.ex`;
- **the token and revocation controllers**: `oauth2/token_controller.ex`,
  `token_revocation_controller.ex` and their `_json.ex` siblings, once the
  application half stands.

## The database side

`applications`, `grants`, `grant_scopes`, `authorization_codes`,
`access_tokens`, `refresh_tokens` and `redirect_uris` are all in the baseline
already — the schema was reproduced from Ecto — so this milestone needs queries
and handlers, not migrations. `users.application_id` is the link from an
application to the account that owns it.

## The consent screen (`GET/POST /oauth2/authorize`)

Read out of `AuthorizeController`. It is a **guild-scoped** consent: the request
names a guild, and the grant it produces belongs to that guild, which is why the
`grants` and `authorization_codes` tables carry a guild id.

The request carries `response_type=code`, `client_id`, `redirect_uri`, `scope`
(optional, empty by default), **`guild_id`** (required) and optionally `state`.

`GET` validates, in this order:

1. `Auth.preauthorize/1` — the client exists, the redirect URI is one of its own,
   and the scopes are known. A bad client or redirect URI is an **error page**,
   never a redirect: the redirect target is exactly what has not been trusted yet.
2. the guild exists;
3. **the bot is a member of it** — looked up by the bot's own client id;
4. **the logged-in user may act for it**: the guild owner, or a member whose
   roles' permissions include the administrator bit.

and then renders a form with one **Approve** button and hidden fields carrying all
of the above plus a CSRF token. There is no deny button and no `deny` action.

`POST` repeats the validations, calls `Auth.authorize/1` for the code, and answers
**303 to the redirect URI** with `code`, `guild_id`, `scope` and `state`. Its
errors do *not* redirect, unlike `GET`'s, which redirects with `error`,
`error_description` and `state` for everything except the two that cannot.

### Two things that cannot have worked, and what was decided

**Both are repaired, not ported.** Both are recorded in `docs/known-gaps.md` as
deliberate differences, and `/login` existing is what made the second answerable.

- **`validate_executor` looks the logged-in user up in Discord by the wrong id.**
  It passes the session's `user.id` to `get_guild_member_with_status_code/2` as
  a *Discord* user id, and compares it against `guild["owner_id"]` — but the
  session stores the **VirtualCrypto** user id (`save_token` puts `vc.id` there),
  which is a small serial while Discord ids are snowflakes. So the lookup cannot
  return 200 and the comparison cannot hold: **nobody could ever have reached the
  Approve button.** Repairing it means reading the user's `discord_id` from the
  database and asking Discord with that.
- **A missing session raises.** With no session the `with` falls through to
  `raise "session validation error!"`, which is a 500 rather than a trip to the
  login page. The answer is to send the browser to
  `/login?continue=<the consent url>` and let it come back, which is the same
  `continue` the login page already honours.

### CSRF, which the old form had and this one gets from the cookie

The Elixir's consent form carries a hidden `_csrf_token`, minted and checked by
Phoenix. The SPA's consent POST does not need one, because the session cookie is
`SameSite=Lax`: a cross-site POST does not carry it, so a forged consent cannot
be submitted with somebody's session attached. That is the whole of the
protection, and it is worth knowing that it rests on that attribute.

## The token endpoint (`POST /oauth2/token`), read out of `TokenController`

Four grants, and the shape they answer in is not the same for all of them.

| The request | Answers |
| --- | --- |
| `grant_type=authorization_code` with `client_id`, `redirect_uri` and `code` **in the body** | the exchange's own JSON |
| `grant_type=refresh_token` with `refresh_token` | the same |
| `grant_type=client_credentials` with basic auth and `guild_id` | a guild-scoped token |
| `grant_type=client_credentials` with basic auth and `scope` | the application's own scopes |
| any other `grant_type` | 400 `unsupported_grant_type` |
| no `grant_type` at all | 400 `invalid_request`, `grant_type_parameter_missing` |

Three things in it are worth knowing before writing any of it.

**The code exchange authenticates with the body, not with basic auth.** It reads
`client_id` from the parameters, where the other grants use
`Plug.BasicAuth.parse_basic_auth/1`. Both are legal; this service picked the body
for that one grant and basic auth for the others.

**There are two scope sets, for different grants.** The browser flow's scopes go
through `is_valid_scopes?/1`, which allows `openid` and nothing else; client
credentials are checked against `vc.pay`, `vc.claim` and `oauth2.register` — no
duplicates, and nothing outside that set. A set of one is not a set of all, so
neither check can be reused for the other.

**A `client_credentials` request with neither `guild_id` nor `scope` answers
`unsupported_grant_type`.** Not because the grant is unsupported — it is
supported twice over — but because both clauses for it name one of those two
parameters, so a request with neither matches the catch-all clause instead. It is
a quirk of how the clauses are written, and worth reproducing rather than
correcting, since a client that sees it can act on it.

Errors are `400` with `{ "error", "error_description" }`, except on the
credentials path, which answers `{ "error" }` alone.

## What the exchanges do, read out of `InternalAction`

### `token_authorization_code/4`

It **deletes the code as it reads it**, so a code is good exactly once, and the
refusals distinguish why:

| | |
| --- | --- |
| the code was never issued, and no grant remembers it either | `invalid_grant`, `invalid_code` |
| the code was issued and already redeemed | `invalid_grant`, `used_code` |
| it has expired | `invalid_grant`, `invalid_code` |
| no application has that `client_id` | `invalid_request`, `not_found_client` |
| the code belongs to a different application | `invalid_grant`, `issued_to_other_client` |
| the redirect URI is not one of the application's | `invalid_grant`, `redirect_uri_mismatch` |
| the application may not take an authorization code | `invalid_grant_type` |

Then a grant is found or created for (application, guild), its scopes are
recorded, an access token is made, and — **only if the application lists
`refresh_token` among its grant types** — so is a refresh token. The answer is
`access_token`, `token_type` of `Bearer`, `expires_in` of 3600, `scopes`, and
`refresh_token` when there is one.

Note `expires_in` is the literal `3600` rather than a difference of clocks, and
that a *used* code is distinguished from an *unknown* one: a client can tell
"somebody already spent this" from "I made this up", and only the first is worth
retrying with a fresh authorization.

### `token_refresh_token/1`

Replaces the refresh token and issues a new access token. The answer carries no
`scopes`, unlike the code exchange's — the grant already knows them. A token that
is not one answers `invalid_grant`, `invalid_refresh_token`, and there is a
`retry_limit` error which comes from the refresh token's own machinery.

### `token_client_credentials/3`

Verifies the secret, finds the grant for (application, guild), and issues an
access token. If the application lists `refresh_token`, it then finds the grant's
refresh token, creates one if there is none and **replaces** it if there is — a
rotating refresh token, so a client cannot keep a stale one alive.

Its answer computes `expires_in` by subtracting the clock from the row's expiry,
where the code exchange writes the literal `3600`. Two ways of saying an hour in
one file, and worth reproducing as they are rather than harmonising: a client
that reads one and not the other is not a client we get to choose.

A `retry_limit` from the refresh token's machinery raises `"UNEXPECTED!"` here.
That is a crash where the same error is handled properly on the refresh path, and
there is no reason to reproduce it — a refusal is available and the branch exists
only to avoid using it.

### `AccessToken.create_access_token/2`

A row in `access_tokens`: a random `token_id`, an `expires` an hour out truncated
to the second, and the grant it belongs to. The token the client receives **is**
that `token_id` — a UUID, not a JWT, and revocable by deleting a row.

**It has a retry that cannot run, and is not worth porting.** Five attempts are
made for "an insert that comes back without an id", and the retry is:

```elixir
case Repo.get(Auth.AccessToken, grant_id: grant_id) do
```

`Repo.get/2` takes a primary key, not a keyword list — `Repo.get_by/2` is the one
that does that — so the single path that reaches this branch raises. An insert
that fails is not retried by wrapping it in a branch that also fails, so the
port is the insert and nothing else.

## What implementing the token endpoint needs

The `grants`, `grant_scopes`, `access_tokens` and `refresh_tokens` tables, and
the three pieces of machinery that write to them: creating a grant for an
(application, guild) pair, recording its scopes, and issuing and replacing
tokens. None of that exists here yet, and none of it is in `vc_core` either —
the browser flow's `/token` issues a *user* access token, which is a different
table and a different question.

The first piece is `create_access_token`, which is one insert.

## The two `client_credentials` shapes are two mechanisms

They share a name and a grant type, and that is all. The Elixir answers them with
different code, different tokens and different ways of computing the same field.

**With `guild_id`** it verifies the secret, finds the grant for (application,
guild), and issues a row in `access_tokens` — the opaque, revocable token the
code exchange also issues. A refresh token comes with it if the application takes
them, created if the grant has none and **replaced** if it has one. `expires_in`
is computed from the clock.

**With `scope`** it verifies the secret, resolves the *application's user id*, and
calls `Guardian.issue_token_for_app/2` — so this one is a **signed JWT**, with
`kind: "app"`, the scopes the request asked for, and the scopes are checked
against `vc.pay`, `vc.claim` and `oauth2.register`. `expires_in` is the token's
`exp` minus now.

So one grant type answers with a database row and the other with a JWT, and
`vc_auth::Kind::App` already exists here for the second. Anything that treats
"client credentials" as one thing will be wrong about half of it.

**A `client_credentials` request with neither parameter answers
`unsupported_grant_type`**, because both clauses name one of them and a request
with neither matches the catch-all instead. See the table above.

### What it needs that does not exist

- basic auth parsing, which is `Plug.BasicAuth.parse_basic_auth/1`: the
  `Authorization: Basic` header, decoded — no handler here reads that header yet;
- `get_application_by_client_id_and_verify_secret/2`, and with it a decision the
  Elixir did not make deliberately: it compares the secret with a pattern match,
  which is not a constant-time comparison. That is worth deciding rather than
  inheriting, since the secret is the whole of a client's authentication;
- `get_application_user_id_by_client_id/2`, which needs `applications` tied to a
  user — `users.application_id` in the schema is that link — and a token issued
  with `Kind::App` through the machinery `vc_auth` already has.

## Revocation (`POST /oauth2/token/revoke`)

Two shapes, both answering `200` with an empty object:

| The request | What it does |
| --- | --- |
| `token=<a token>` | `Guardian.revoke/1` |
| `jti=<uuid>`, `typ=access`, `kind=app` or `user` | `Guardian.revoke_with_jti/1` |
| anything else | `400` with an `invalid_request` whose description is one long sentence |

The description is
`token_or_token_id_type_and_kind_is_not_found_or_invalid_kind_or_type`, which is
the Elixir's naming and not a mistake here.

### The hole in it, which is worth deciding on

`Guardian.revoke/1` is the **JWT** path: it verifies the token and deletes the
`user_access_tokens` row belonging to its `jti`. Both of these shapes go there,
including the one that takes a plain `token`.

But the tokens this endpoint's sibling issues — the `access_tokens` and
`refresh_tokens` rows, which are held as UUIDs rather than signed — are **not
JWTs**, so `Guardian.revoke/1` cannot verify them and reaches nothing. The
functions that would delete them exist in the service and nothing calls them:

```elixir
def revoke_access_token(access_token)
def revoke_refresh_token(refresh_token)
```

So a client can revoke a `client_credentials` JWT through this endpoint, and
**cannot revoke the token it was handed by `POST /oauth2/token`** — which is the
one RFC 7009 exists for. Both of those functions are unreachable code as it
stands, so this is a decision rather than a port: wire them in, or delete them
and accept that the issued tokens are not revocable. The tables are built for
the former, and a client that cannot revoke a token it has leaked is the reason
the endpoint exists.

