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
