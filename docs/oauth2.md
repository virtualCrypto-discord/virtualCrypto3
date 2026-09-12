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
| `GET/POST /oauth2/authorize` | browser session | the consent screen — **needs the web UI** |
| `POST /oauth2/token` | client credentials | not read yet |
| `POST /oauth2/token/revoke` | client credentials | not read yet |
| `POST /token` | browser session | a VC API token — **needs the web UI** |

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

## Still to read before implementing

- `VirtualCryptoWeb.Clients.render_application/1` — the shape both the list and
  the single read share, and therefore the one that has to be exact;
- `VirtualCrypto.Auth.register_application/2`, `get_application/1`,
  `get_user_applications/1`;
- `Auth.Application.PatchQuery.patch/3` and the metadata validator
  (`application_metedata_validator.ex`);
- `Auth.RedirectUris`, so the scheme rule above becomes the real one;
- the token and revocation controllers, once the application half stands.

## The database side

`applications`, `grants`, `grant_scopes`, `authorization_codes`,
`access_tokens`, `refresh_tokens` and `redirect_uris` are all in the baseline
already — the schema was reproduced from Ecto — so this milestone needs queries
and handlers, not migrations. `users.application_id` is the link from an
application to the account that owns it.
