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

### What a guild-scoped token is good for

`POST /api/v2/currencies/issue`, and so far only that. The Elixir answers with
the token but accepts it nowhere — its `verify_claims/2` refuses any kind but
`user` and `app` — and this rewrite makes the token mean something instead of
refusing an unknown kind. The token is the `access_tokens` row's own id; its
grant carries both the guild (so the endpoint spends that guild's pool and no
other's) and its scopes (so taking `vc.issue` away stops it issuing). See
`docs/issue.md` for the endpoint, the `vc.issue` scope, and the three ways a
guild grants it — the consent screen, the application's own page, and `/grant`.

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

## The metadata validator, rule by rule

`Application.Metadata.Validator` is where registration decides what it will
accept. Every field except `application_type` accepts `nil` and means "not
given", and every refusal is an `invalid_client_metadata` with a description that
is the field's name and the rule, spelled out:

| Field | Rule | Refusal |
| --- | --- | --- |
| `response_types` | a subset of `["code"]` | `response_types_must_constructed_from_code` |
| `grant_types` | a subset of `["authorization_code", "refresh_token"]` | `grant_types_must_constructed_from_authorization_code_or_refresh_token` |
| `client_uri` | `http` or `https` | `client_uri_scheme_must_be_http_or_https` |
| `webhook_url` | `http` or `https` | `webhook_url_scheme_must_be_http_or_https` |
| `logo_uri` | `https`, or `data:` with an image mediatype and at most 2048 bytes | three, below |
| `discord_support_server_invite_slug` | at least one `[0-9a-zA-Z]` | `..._must_construct_from_half_width_alphanumeric` |
| `application_type` | `web` or `native` | `application_type_must_be_web_or_native` |

Four things in that table are worth more than the table.

**The two list fields are returned as sets.** `response_types` and `grant_types`
go through `MapSet.new/1` and come back from `MapSet.to_list/1`, so a request that
names `code` twice is stored once. The deduplication is not a side effect; the
validated value is what the row gets.

**`logo_uri` has three refusals and their order matters.** A `data:` URI is
checked for its mediatype first (`logo_uri_mime_type_must_be_image`) and its size
second (`logo_uri_must_not_bigger_than_2048_bytes`), and the size is that of the
**rebuilt** URI rather than the string that arrived. The allowed mediatypes are
bmp, `vnd.microsoft.icon`, gif, jpeg, png, `svg+xml`, tiff and webp — no others,
so a `data:text/html` logo is refused whatever else is right about it. Anything
that is neither `https` nor `data` is `logo_uri_scheme_must_be_data_or_https`.

**The slug rule is not anchored.** `Regex.match?(~r/[0-9a-zA-Z]+/, slug)` is a
search, not a match of the whole string, so `"!!!abc!!!"` passes and so does
anything else containing one alphanumeric character. The description says "must
construct from half-width alphanumeric", which is what it was meant to say; this
is the difference between what a rule says and what it does, and it is recorded
rather than quietly tightened, since a client's slug is stored as it was sent.

**`application_type` is the one field with no `nil` clause**, so the others'
"not given" cannot be said of it. The column has a default of `web`, so it is
never absent in practice.

## What an application looks like when it is answered

`Clients.render_application/1`, which the single read, the list and the client
registration all go through — so an error here is an error in three endpoints at
once. Sixteen fields, and the encodings are the part that is easy to get wrong:

| Field | |
| --- | --- |
| `client_id`, `client_secret` | as stored |
| `client_secret_expires_at` | the literal `0`, the dynamic-registration convention for "never" |
| `redirect_uris` | the registered rows, each one's `redirect_uri` |
| `user_id` | the owning user's id, **as a string** |
| `discord_user_id` | **as a string**, and `null` when the account has none |
| `owner_discord_id` | **as a string** |
| `public_key` | the key as **lowercase hex** |
| `application_type`, `client_name`, `client_uri`, `logo_uri`, `webhook_url`, `grant_types`, `response_types`, `discord_support_server_invite_slug` | as stored |

Three numbers travel as strings and one travels as hex, which is why this is
written down rather than inferred from the columns.

**The list groups by application.** Its query returns a tuple per redirect URI —
`{application, user, redirect_uri}` — and `ClientsJSON` groups those by the
application's id and maps the group, so an application with three redirect URIs is
one entry with three, not three entries. Redirect URIs that are `nil` are filtered
out of the group, which is how an application with none registered answers with an
empty list rather than a list containing a null.

**Implementing it needs a fuller `Application` than exists here.** What is in
`vc_core` carries an id, a client name and the grant types — the three fields
`preauthorize` asks about — and this answer shows fourteen more.

## What registration actually does, now that it is read

`Auth.register_application/2`, inside a transaction, and three details in it are
worth more than the steps around them.

**Two defaults.** `application_type` becomes `"web"` when the request does not
name one, and `grant_types` becomes **`[]`** — an empty list, not the column's
default. So a registration that asks for nothing gets an application that can do
nothing: without `authorization_code` among its grants, `preauthorize` refuses
every consent request it could ever make. That is the Elixir's behaviour and it is
silent, which is why it is written here.

**The owning account is created by registration,** not before it:

```elixir
{:ok, user} = Repo.insert(%VirtualCrypto.User.User{application_id: data.application.id})
```

A `users` row with an application id and nothing else. That is why `users.discord_id`
is nullable, and why the answer to `GET /oauth2/clients/@me` shows
`"discord_user_id": null` for an application's own account — the account is not a
person and never had a Discord id. The two facts are the same fact, seen from two
ends.

**The keypair is generated here,** by the service, with the same call the
handshake uses for its fresh one: `:public_key.generate_key({:namedCurve, :ed25519})`.
The private half is stored for signing deliveries and the public half for the
application to verify with.

And the whole of it — the application, its owner, its scopes — is one transaction,
so a registration that fails half way leaves nothing.

### The last details of registration, including one that is probably a bug

The order the fields are validated in is fixed and observable, because the first
failure is the one reported: `response_types`, `grant_types`, `application_type`,
`client_name`, `client_uri`, `logo_uri`, `webhook_url`,
`discord_support_server_invite_slug`, then that `redirect_uris` is an **array**
(`redirect_uris_must_be_array`), then that each of them has an http or https
scheme, then the client id and the keypair, and only then the webhook handshake.

**The handshake only happens when a webhook URL was given.** `if webhook_url do
… else :ok end` — so an application that registers none is registered without one,
which is the same `:nop` its deliveries take later.

**`response_types` used to be validated and then thrown away, and is not any
more.** The Elixir builds the row with `response_types: []` — a literal — so
whatever the request asked for and passed `validate_response_types/1` is not what
it stores, and its answer to `GET /oauth2/clients/@me` shows an empty array
whatever was asked for. `grant_types` is taken from the validated value, so the
Elixir treats its two list fields differently and treats this one wrongly.

The port did the same, because that was the honest thing to do while the question
was whether to port a bug, and it stores the validated value now. Nothing
downstream reads `response_types` — `preauthorize` checks the redirect URI and the
grant types and the scopes and never this — so the difference is visible in the
answer and nowhere else, and it is a difference from the Elixir rather than from
the specification.

**And the two generated values.** The client id is `Ecto.UUID.generate/0`, so a
UUID. The client secret is `:crypto.strong_rand_bytes(32)` base64-url encoded
without padding — thirty-two bytes, the same entropy as the authorization code,
spelled differently.

### Two of registration's steps already exist, in another endpoint

Registration's second and third steps — refresh the owner's Discord authorization
when it is near expiry, then ask Discord who they are — are the same thing
`GET /api/v2/users/@me` needed, and they are written there as `resolve_token` in
`routes/v2/users.rs`:

```rust
async fn resolve_token(state: &AppState, discord_id: i64, authorization: &DiscordAuth)
    -> Result<String, ApiError>
```

It refreshes inside the last fifteen minutes of the seven-day lifetime, stores the
new authorization, and answers with the token to use.

**It is private to that file, and registration should not copy it.** The behaviour
is two constants and a comparison — the fifteen minutes and the seven days — and a
second copy is a second place for both to be got wrong, in a flow where getting
them wrong means asking Discord with a token that has expired. It wants to be
shared: the natural move is to lift it out of `v2/users.rs` into a module of its
own, and have both callers use it.

What registration adds after it is its own: the profile the token fetches answers
with `bot`, and a bot account may not register — `user_verification_failed`. The
same profile's `id` is the `owner_discord_id` the application row is written with.

### The handler's last obstacle: the handshake needs the proxy, and the state has only the notifier

Registration's fifth step is the webhook handshake. `verify` and `Proxy` are
written and tested, and `fresh_keypair` with them — but a handler cannot reach
them. `AppState` carries a `Notifier`, which holds a `Proxy` privately, and a
trait object is not a proxy: `Notifier::notify_claim_update` takes a claimant and
events and answers nothing.

So the state wants the proxy itself — `Option<Arc<Proxy>>`, the same one
`main.rs` builds for the notifier, shared rather than built twice. Then a handler
can call the handshake, and the notifier can be built from the same value.

**And a registration without a webhook must not need one.** The Elixir's
`if webhook_url do … else :ok end` means an application that registers none is
registered without a handshake — so "no proxy configured" is only a problem for a
registration that asked for a webhook.

When it is a problem, which refusal it is matters, and the distinction is the one
`Handshake` already draws: a handshake that ran and came back wrong is
`verification_failed`, and the application is at fault. A handshake that could not
be attempted, because this service has no proxy, is **the service's fault** and
belongs in the five-hundreds — malformed client metadata is not what happened, and
saying so would send a user to look at their own request.

## The edit (`PATCH /oauth2/clients/@me`)

`Application.PatchQuery.patch/3`, and it is a patch rather than a replacement:
every field is optional, and a field that is absent is left alone. The Elixir
writes it as a pipeline of setters, each of which either changes the query or
returns `:nop` for "not in this request".

Four things in it are worth knowing.

**`redirect_uris` is replaced wholesale when it is given.** The rows for the
application are deleted and the ones in the request inserted. So a PATCH that
sends one URI leaves the application with one URI rather than two, while a PATCH
that does not mention them leaves them exactly as they were — the difference
between an empty list and no list, and the reason the Elixir asks
`Map.fetch/3` for the parameter rather than reading it.

**The webhook is verified only when `webhook_url` is in the request.** The
handshake is the same one registration performs, and it is skipped for an edit
that does not name a webhook — which is what makes it possible to change a client
name on an application whose webhook has since gone silent, rather than being
unable to edit it at all.

**`Repo.update_all/2` does not touch `updated_at`.** This is the second place that
matters, after the Discord authorization's refresh: the Elixir updates the row
without a changeset, so no timestamp is applied. An edit therefore leaves
`updated_at` where it was, which is observable and worth reproducing rather than
tidying.

**And it is all one transaction**, so an edit that fails half way — a bad redirect
URI after a good client name — changes nothing.

The refusals are the metadata validator's, unchanged, and the redirect URI's is the
same pair as registration's: `invalid_redirect_uri` with
`redirect_uri_scheme_must_be_http_or_https`.

