# The web UI, derived from the site it replaces

> **The SPA is dropped, by decision.** Nothing below is a plan to build `web/` any more.
> What stays true, and is why this document is worth keeping, is everything read out of
> the old site that the **service** needs regardless of who is calling it: the connect
> flow's two conditions and its six refusals, the id `/applications/:id` means, the
> RFC 7592 read that `registration_client_uri` names, the session and callback the
> browser routes still use, and the note at the top of the old repository's history being
> a rewrite rather than a site.
>
> The requirements list below is left as it was written — as what the SPA would have
> needed — because that is the honest record of the reading. Treat the items marked
> **done** as service routes that exist, and the two not built (the claims page, the
> documents) as things nobody has to do.
>
> What was built in `web/` stays where it is and still passes `svelte-check`; removing it
> is a decision for whoever drops it, not a consequence of this note.

The requirements below come from the Elixir app's own pages rather than from
imagination: the router's browser scope, its three LiveViews, and the controllers
behind them. Where something was not read, it says so.

**Read the old repository's history and its Elm, not its tip.** `74b991d` — "Rewrite as
phoenix 1.7 way, and remove elm" — deleted the working front end and left stubs in its
place: the mypage is now `<div>My Page</div>`, `DashboardApplication` is a module with a
`use` and nothing else, and `live/app/overview.ex` is the `<h1>Content</h1>` in the table
below. The application list, its links, its routes and everything else a person actually
used are in `assets/elm/src/` (add `--find-object` to a `git log -S` and they come back)
and in the commits before the rewrite. Reading the tip for behaviour reads the deletion,
which is a mistake already made once here: it is where "the old site never linked the
connect page" came from, on a site whose list linked it as
`"/applications/" ++ application.client_id`.

## What the old site is

| Path | What it is | Notes |
| --- | --- | --- |
| `GET /` | landing page | |
| `GET /logout` | ends the session | |
| `GET /invite`, `/support` | redirects | to the bot and the support guild — **built**, from `Links` |
| `GET /callback/discord` | the Discord OAuth2 callback | see the session model below |
| `POST /token` | the session's VC API token | re-issues on demand |
| `live /app` | **a stub at the tip** | `<h1>Content</h1>`; the mypage it replaced is Elm |
| `live /me` | the router points at `DashboardApplication` | which is empty at the tip |
| `GET /applications/:id` | one application | renders the JSON *into* an HTML page |
| `live /applications/:id/connect` | the connect flow | two events: `verify`, `change`; built below |
| `live /contract/:id` | approving a contract | not read — and see the warning above |
| `GET /applications/verification` | a warning about this flow | **built**, from blob `616e250f` |
| `GET /document/{,,about,commands,api}` | four documents | static prose |
| `GET/POST /oauth2/authorize` | the consent screen | needs a session |

Two things worth saying plainly: **`/app` is a stub in the tree as it stands**, but the
dashboard is not something the old site lacked — it was Elm, and `assets/elm/src/`, whose
`Mypage/Route.elm` names `DashboardPage`, `ClaimPage` and `ApplicationsPage`, is where it
went; and the documents under `/document` are prose, so they belong to the docs site
rather than to a rebuild.

## The session, read out of the old app

The cookie is **signed, not encrypted** — `Plug.Session` is configured with
`store: :cookie`, `key: "_virtualCrypto_key"`, `signing_salt` and
`same_site: "Lax"`, `secure` in production only — so its contents are readable by
the browser and only tamper-proof. A Rust session should be the same kind of
cookie rather than something more elaborate.

`browser_auth` is an interceptor rather than a guard: a request with no session
is **redirected** to Discord, and so is one whose stored Discord authorization
cannot be refreshed.

The login is three steps:

1. generate a `state` of 32 random bytes, base64-url without padding, store
   `discord_oauth2: {state, continue: <the url that was asked for>}` in the
   session, and redirect to
   `https://discord.com/api/oauth2/authorize` with `client_id`, `redirect_uri`,
   that `state`, `scope=identify` and **`prompt=none`** — Discord shows no
   consent screen to someone who has already agreed;
2. `/callback/discord` compares the returned `state` with the session's, then
   exchanges the code at `https://discord.com/api/oauth2/token`
   (`grant_type=authorization_code`, `client_id`, `client_secret`,
   `redirect_uri`, and `Accept: application/json`);
3. the token is used to read the Discord user, the authorization is stored, and
   a VC API token is issued — see the next section for what happens to it.

An SPA changes one thing and one thing only: step 3's answer. Because
`browser_auth` redirects, an `XMLHttpRequest` from the SPA that meets a stale
session gets a 302 to Discord, which it can do nothing sensible with. Requests
the SPA makes should therefore be answered `401` with somewhere to log in,
while the browser's own navigation keeps the redirect that makes the old flow
work.

## What the old callback does with the token

The old flow is built for a browser that reloads pages, and it shows:

- the session cookie holds `user: {id}` and, while a login is in flight,
  `discord_oauth2: {state, continue}` — the `continue` is where to go afterwards;
- `/callback/discord` compares the returned `state` with the session's, exchanges
  the code, stores the Discord authorization, and issues a VC API token with
  `oauth2.register`, `vc.pay` and `vc.claim`;
- **the token comes back in response headers** (`x-access-token`, `x-redirect-to`,
  `x-expires-in`) and in a rendered page, and the session is renewed. `POST /token`
  re-issues one from the session later.

An SPA wants the same three steps with JSON: a login URL to send the browser to, a
callback that answers JSON (or redirects and leaves the token to `/token`), and
`POST /token` for renewals. The cookie session stays — it is what the consent
screen needs — but the token no longer has to travel in headers.

## Developer features in a Discord DM

Asked for after the SPA was dropped, and answered: **all three** of registering an
application, listing and reading the caller's own, and connecting a bot — **not by typing
slash commands**, on the newer Discord surface, with the `client_secret` answered
**ephemerally** rather than kept back.

What is known, and what has to be checked before it is built:

- **The interaction endpoint already knows the types.** `routes/interactions.rs` dispatches
  `2` to a command, `3` to a component, `4` to an autocomplete, `5` to a modal, so buttons
  and modals need no new entry point — only the handlers for what they do.
- **Ephemeral is only possible from an interaction**, which is consistent with "not
  pseudocommands": the answers here are interaction responses. The secret can be sent on
  that basis, and it must be `ephemeral` — a DM the bot can no longer edit is still a
  message Discord keeps.
- **The registration is not missing, it was never ported.** I wrote here that it lived
  outside this tree and had to be found; it is in the old one, at
  **`priv/register-commands.exs`**, and this repository has no equivalent — no command
  definitions, no call that sets them. That file defines all nine commands (`help`,
  `invite`, `give`, `pay`, `info`, `create`, `delete`, `bal`, `claim`) with their full
  option trees, and sends them as one `PUT` to
  `https://discord.com/api/v10/applications/{client_id}/commands`, or to
  `.../guilds/{guild}/commands` when given a guild id, with `Authorization: Bot {token}`.

  So the port is incomplete in a way nothing in this tree says out loud, and the DM work
  starts by closing that: the definitions, and something that sends them.

- **`dm_permission` is already the DM answer, and it is the deprecated spelling.**
  `give`, `create` and `delete` carry `"dm_permission" => false`; the other six let DMs
  through. Discord replaced the field with `contexts` (where a command may run) and
  `integration_types` (who may install it), so anything built now should be expressed in
  those rather than carried over as it was — which is also where "not slash commands, a
  newer feature" lands. Read the current shape before writing it.
- **The commands assume a guild.** `bal`, `pay` and `give` read the guild's currencies, and
  `interactions.rs` gets the guild from the payload. A DM has none, so anything guild-shaped
  — connecting a bot above all — has to take the id as an argument or have a DM meaning
  decided.
- **Which surface is not settled here.** "A new feature, not slash commands" is not
  specific enough to code against, and guessing Discord's API is how the connect route came
  to take the numeric id. It needs to be read, not assumed.

Whoever builds this should start by finding the registration and reading the surface, in
that order — the same rule as the rest of this document.

## The SPA is the session's own

Everything the SPA had for managing an application — the list, its detail page,
the connect flow, the registration form, the account page, and the contracts page
this document's contract item described — **has been deleted**. Not
ported away, not deferred: removed, because every one of those operations is a
command or an endpoint that Discord can do, and a screen that only repeats a
command is a second place for the same rules to be got wrong. What is left is the
landing page the old site had, and the verification warning — and nothing that
manages anything. The login link went with the rest: the only flow that needs a
session is OAuth2's, and the consent screen sends a browser without one to
`/login` itself, so a button on the landing page was a second way in for nobody.

## The requirements, in the order they matter

**What is left, in one place**: the **claims page** (the item below the contract flow,
which this list had missed until the Elm was read), and the **documents** (another
repository's prose). The contract flow was closed — neither site implemented it — and
the feature itself has since been built as an addition rather than a port
(`docs/contracts.md`), with a page below and a command in Discord.

`/invite` and `/support` are built: `web::invite` and `web::support` redirect from
`Links::invite_url` and `Links::support_guild_invite_url`, which `vc-server` already
fills from `INVITE_URL` and `SUPPORT_GUILD_INVITE_URL`, so the bot's
invite and the support guild's address stayed where the command responses read them
from. 307 rather than the Elixir's 302, because that is what axum has for a move that is
not permanent and a `GET` cannot tell them apart.

`/applications/verification` is built, from the prose in the history the warning at the
top is about: `lib/virtualCrypto_web/templates/application/readme.html.eex`, blob
`616e250f`, kept word for word — spelling mistakes and all — in
`web/src/pages/Verification.svelte`.

It turned out to be worth more than a tidy-up. It is **a warning**: this is the token
page for binding an application to a bot, and anyone who asks you to paste its URL into a
Discord application's description is doing something to you, because pasting it there is
what gets an application bound to a bot it does not own. That is the other face of the
check the connect flow runs, and the connect page now links to it — the page whose
paragraph asks for a description is where somebody would be asked.

It is also the same shape as the detail page's path, and comes before it in the shell
for the same reason the old site's router put it first. The **edit form** is not on this list on
purpose: it is a decision about holding an application's credentials in a page, recorded
under item 3.

Each one says which it is — built, or not read — because a list that mixes the two
without saying which reads as unbuilt, which this one has already been read as once.

1. **The consent screen** (`/oauth2/authorize`). It is what blocks the rest of
   OAuth2: no application can obtain a token without it, and it is the one page
   that must work when JavaScript does not, because OAuth2 sends browsers to it.

   **This one is answered and not by the SPA.** It is a server-rendered route in
   `routes/consent.rs` — `GET` shows the form, `POST` decides — so a browser sent
   here by a client library needs no script to get through it, which is what the
   paragraph above asks for. Nothing of it belongs in `web/`.
2. **Discord login and logout**, with the `continue` return path and the CSRF
   state check the old callback already does — **done, and tested**. `/login` mints a
   state (`routes/web.rs:38`) and puts the browser at Discord with it (`:46`), the
   callback refuses an answer whose state is not the one it sent (`:111`), and it
   returns the browser to the `continue` it was given, home when it was given none
   (`:152`). `logout` clears the cookie (`:61`). `tests/login.rs` covers the redirect
   and its state, the callback in full, a state that does not match, and the logout.
   Nothing here was unread: this item was on the list without saying which it was.
3. **The registration form** (`/applications/register`) now asks for the
   subscription at registration: two checkboxes, both checked, for the claim
   update and the grant decision. What is checked is what is sent — checked is
   sent and unchecked is not, and both boxes checked sends both, which is also
   what absent would get, but the form says what it means rather than saying
   nothing and meaning it.
4. **The application list and detail** — **half done, and this said "done" for a day**.
   The list is built (`web/src/pages/Application.svelte`) and `GET /oauth2/clients/@me`
   answers it. The **detail page is not**, and the old site had one: the Elm list links
   `/applications/<client_id>` (`Applications.elm` l.119), which is the same `:id` the
   connect route takes.

   **Now built**, and it needed no read of its own. The worry above was that a page
   rendering from the list would only work for visitors who came through the list; that
   was wrong. The list is the caller's own applications, so fetching it and picking the
   one whose `client_id` matches is a complete answer for a visitor who typed the URL —
   and for one who did not. There is no `GET /oauth2/clients/:id` because there does not
   need to be one: what such a read would be allowed to answer is exactly what the list
   already does.

   **Editing is not a page in this site.** `PATCH /oauth2/clients/@me` requires an
   application token — it answers `invalid_kind` to anything else — and a browser
   holds a user token, because that is what `POST /token` issues. It is RFC 7592: the
   application manages its own registration with the `registration_access_token`
   registration answered with. A page could keep that token and offer the edit, but
   that is a decision about storing an application's credentials in a page, and not
   something to assume from the shape of an endpoint.
5. **The connect flow** (`/applications/:id/connect`) — **done**: the service's route,
   its refusals, the page and the round trip the browser makes (`5805529`), with what the
   Elm list said about the id recorded below.
6. **The contract approval** (`/contract/:id`) — **closed: there was never anything to
   port**. The LiveView is one blob, eight lines, no assigns, the page beside it is
   static sample data, and the Elm has no contract route at all. Both sites shipped the
   same mockup.

   **The feature itself is an addition rather than a port** (`docs/contracts.md`):
   `/contract list` in Discord, drawn from what the service answers, with the
   buttons a contract takes. Nothing of the mockup's markup or sample data is in
   it, and there is no page for it — the SPA is the login and nothing else (see
   below).
7. **`Mypage/Claim.elm` — the claims page — is not in this list and should be.** It is
   the Elm's `ClaimPage`: claims sent and claims received, paged, read from
   `GET /api/v2/users/@me/claims`, which is implemented and tested here (`v2_claims.rs`,
   `v2_claims_list.rs`) with the goldens the capture gave. `web/` has no page for it, so
   the API's own answers have nowhere to be seen.
8. **The landing page**, which is prose plus a login button — **done**.
9. **The documents** — **built**: the prose lives in `crates/vc-api/src/docs/`,
   which draws both `/help` in Discord and `/document/*` on the site. One source
   for the two, so they cannot come to say different things.

## What the SPA will need that does not exist yet

- `GET /users/@me` and `GET /users/@me/balances` — the frontend's own endpoints,
  all of them implemented (`3c5e119`, and the four captured goldens are read by
  tests as of `2e7056b`).
- A JSON login callback, since the old one answers with headers and HTML.

## The connect flow, read out of `live/connect_application.ex`

The one LiveView with real events, and what it does is prove that the bot an
application owns is really that application's.

`mount` refuses to run without an `id`, and sends the browser to
`/applications/<id>` when the application is not found (l.5-33). Otherwise it
assigns the application, its owner account and the entered `bot_id`/`guild_id`, with
`edit` false.

`verify` (l.41) is the check, and the mechanism is worth stating: it reads the
guild's **integrations** from Discord (`get_guild_integrations_with_status_code`,
l.46) and looks for one whose `application.description` **contains the
application's uuid** (l.60). The bot's integration description is therefore the
thing the application's owner writes the uuid into, and the service verifies
ownership by reading it back from Discord rather than by trusting the browser. On a
match it records the bot's id and answers 「認証成功しました。トークンは削除して差し支えありません。」
— the token may be deleted (l.65-70).

Two failure paths read the guild (l.73) and the user (l.97) for a message. One
of them, at l.66, calls `String.to_integer` on the bot id — the same shape as the
crash noted elsewhere, and reachable with a non-numeric id, since the id comes from
the form.

`change` (l.146) does nothing but put the two ids into the assigns and set `edit`
true — the LiveView's live form, which an SPA does not need.

### It cannot be a page alone, which is the thing to know before starting

The LiveView calls Discord itself. It can: the service holds the bot token. A page
cannot — the token is not the browser's and must not be — so this flow needs endpoints
in front of it, and the frontend work starts by writing them.

What those endpoints have to do is the part that *is* read here: read the guild's
integrations from Discord (`get_guild_integrations_with_status_code`, l.46) and find
one whose `application.description` contains the application's uuid (l.60). That is the
whole verification — ownership is proved by reading back from Discord what the
operator wrote into the bot's integration description, not by trusting the form.

### What it actually does, now read

Two conditions, and the first is the one that is easy to miss: the target integration
is the one whose `application.bot.id` **equals** the submitted bot id (l.50-57), and
only then must its `application.description` **contain** the application's uuid (l.60).
So the uuid is a second signature on an integration already identified by its bot, not
the way it is found.

And it writes something. On success it calls
`VirtualCrypto.ConnectUser.set_discord_user_id(app_user_id, bot_id)` (l.62-67) — the
application account's `discord_id` is set to the bot's. That is the point of the whole
flow: an application's account is created with no Discord id, and connecting binds it
to the bot that speaks for it, having checked with Discord that the bot is in the guild
and that its description carries this application's uuid.

The failures are distinguished rather than collapsed, which is most of the file:

- integrations 403 → the guild is read to say which of the two it is: the service is
  not in that server at all, or it is there without Manage Server (l.72-87).
- integrations 404 → that server id does not exist (l.89-90).
- anything else → the status is named (l.92-93).
- no matching integration → the bot id is looked up, and then the guild, to say that
  the bot is not in that server (l.95-105). A user id that is not a bot is a further
  branch (l.108-).

So a Rust endpoint doing this needs three Discord calls the seam does not have and one
write to `users.discord_id`. None of it is guessable from the shape of the page, which
is why it is written here.

### The page needs an id that the read does not hand out

`POST /applications/{id}/connect` takes the **numeric** application id — the same one the
Elixir's `/applications/:id` takes — and `GET /oauth2/clients/@me` does not return it.
`render` answers `client_id`, `client_secret`, `user_id`, `discord_id` and the rest of
RFC 7592's fields with this service's extensions beside them, and no `id` among them.

So the page cannot be built from what the API hands out, and the list has nowhere to
link to. One of two things has to change, and which one is a decision about the
contract rather than about the page:

- `render` gains `"id"`. The document is already extended (`owner_discord_id`,
  `discord_support_server_invite_slug`, `public_key`), so one more field is not a
  departure from it — and the route's 404-not-403 is exactly what keeps an enumerable
  id from telling a caller anything, so exposing it is harmless.
- the route takes the `client_id` instead. That is the application's public identifier
  and the string the description check already turns on, so an operator has it — and it
  needs no change to the read at all. It parts company with the Elixir's path, though,
  and the route is tested against the numeric id.

**The old site does not settle it, and that is now checked rather than assumed.** There
is no dashboard that lists applications: `live/app/overview.ex` is the stub this document
already called a stub, and there is no application *show* LiveView beside
`connect_application.ex`. So `/applications/:id/connect` was a URL an operator typed,
with an id they had from `GET /applications/:id`, and the old read answers nothing about
what a list should hand out — the old `/me` never had to.

What that leaves is this project's own choice, and the old site still says which way to
lean. Both of its routes are keyed by the **numeric** id: the router's
`live "/applications/:id/connect"`, and `connect_application.ex` sending the browser to
`"/applications/" <> params["id"]` when it cannot find one (l.14, l.33). Porting the id
as a `client_id` would be a departure invented here, not a reading of there.

So `render` gains `"id"`. The cost is that the reads through `render` —
`GET /oauth2/clients/:id` and `GET /oauth2/clients/@me` — change shape, and if either has
a golden captured from the Elixir then that golden says what the Elixir answered and has
to be looked at before this is written, the same way every other golden question in this
project has been. That check is the first thing to do, not the last.

**And it is answered, which is why none of the above was done.** The old site's list is
not in the Phoenix code at all: it is Elm, at `assets/elm/src/Mypage/Applications.elm`,
and the Phoenix mypage that replaced it is `<div>My Page</div>`. That is the state the
whole repository is in after the 1.7 rewrite — `DashboardApplication` is a module with a
`use` and nothing else, `live/app/overview.ex` is the stub called a stub above — so
reading the current tree for how the old site behaved reads the rewrite, not the site.

The line that settles it is l.119 of that file:

```elm
a [href ("/applications/" ++ application.client_id)] [p [class "title"] [text ...]]
```

and l.79, which navigates the same way after an application is created. **The `:id` in
`/applications/:id` is the `client_id`, not the numeric one.** The connect route is the
same `:id` — the router's `live "/applications/:id/connect"`, and
`connect_application.ex` sending a browser it cannot satisfy to `"/applications/" <> id`
— so a connect is asked for by `client_id`, which is the uuid the description check
already turns on and which the list already has.

`connect.rs` takes `Path<i64>` and matches it against `owned_by`'s numeric ids, and its
tests pass because they were written against that. Both are wrong together, and the
frontend problem disappears with them: no field has to be added to `render`, because
`client_id` is already in it.

What to change: the path parameter and its lookup, and the tests with them. The Elixir
loads the application by this id in `mount` and still has the struct's own `.id` for
`set_discord_user_id`, which is the numeric one — the two are not interchangeable, and
that is the trap here.

**Done** (`92f8a9b`). `connect.rs` takes `Path<String>` and looks the client id up among
the ids `owned_by` returned, and the tests name applications by the id they actually
have. Ownership is still decided first and by id: checking the path first would answer
whether a client id is real to somebody who owns nothing.

The page is now buildable — the list already has `client_id`, so the link is
`/applications/<client_id>/connect` and nothing has to be added to any response.

Where those calls go, since it is four places and not one: the `DiscordApi` trait in
`crates/vc-api/src/discord.rs` (l.41-), `HttpDiscordApi` (l.320-), **`CachedDiscord`**
(l.230-), and `FakeDiscord` in `crates/vc-api/tests/support/mod.rs` (l.130-). Every
call the service makes is a trait method, and `CachedDiscord` delegates all of them to
the inner one — which is easy to miss, because the place a call is implemented is not
the place a method has to exist.

Two of the three are not new calls but **status-aware versions of ones that exist**.
`get_guild` and `get_user` answer `Option`, folding every failure into "not there", and
this flow needs to tell 403 from 200: it reads the guild only to say which of "the
service is not in that server" and "it is there without Manage Server" it is. That is
the difference between two messages the operator acts on differently, so it cannot be
folded. The Elixir has `get_guild_with_status_code` and `get_user_with_status` for the
same reason, under `Raw`, which is also where the integrations call belongs — none of
the three should be cached, since a permission question answered from fifteen minutes
ago is a permission question answered wrongly.

The three signatures, since the Elixir answers a status and a body rather than folding
them:

```rust
async fn get_guild_integrations_with_status(&self, guild_id: i64)
    -> Result<(u16, Vec<Map<String, Value>>), DiscordError>;
async fn get_guild_with_status(&self, guild_id: i64)
    -> Result<(u16, Map<String, Value>), DiscordError>;
async fn get_user_with_status(&self, user_id: i64)
    -> Result<(u16, Map<String, Value>), DiscordError>;
```

The idiom is `get_roles` (l.419-442) and there is nothing else to it: the base is a
literal `https://discord.com/api`, the bot's calls carry
`.header(AUTHORIZATION, format!("Bot {}", self.bot_token))` while a user's carry
`.bearer_auth(token)`, and a failure to send or parse is
`DiscordError::Request(error.to_string())`. A status is read with
`response.status().as_u16()` instead of being compared to `NOT_FOUND`, which is the
whole difference between these and the calls they sit beside.


### The tail, and the constraint it depends on

The remaining branches are all about the two ids being wrong in different ways:

- the bot id names a **user rather than a bot** → said by reading the user and checking
  `bot`, naming them (l.108-113).
- the bot id names **nobody** → 404, and said so (l.115-120).
- the integration was found but its description **does not contain the uuid** → the
  bot's own username is given, because the operator is looking at it when they edit
  the description (l.123-136). The message calls the uuid 「トークン」, which is the
  word the page's own copy uses.

And one branch that is a rule rather than a message:

- `{:error, :conflicted_user_id}` → 「すでにそのBotは別のApplicationに紐付けられています。」
  (l.138-139). **One bot belongs to one application**, and the schema says so:
  `users_discord_id_index` is `UNIQUE` on `users.discord_id`, so the second
  application to try to bind that bot gets a unique violation rather than taking the
  binding away. The write has to answer that violation as this message, and it is
  worth knowing that the constraint is what catches it — a lookup that returned the
  first match would have overwritten the other application's binding silently.

An earlier version of this section said the persistence was unread and asked whether
there was any. There is: the binding, and this is the branch that says it is exclusive.

Also to add on the Rust side, before any of it can be called: the Discord seam has
members and roles (`get_guild_member`, `get_roles`) and no integrations call.

## The contract flow is a sketch, not a flow

### Read, and still a sketch: `/contract/:id` in the tip

`live/contract/approve_application.ex` is eight lines and its `mount` returns
`{:ok, socket}` with **no assigns at all**; the 99-line `.heex` beside it is therefore
static sample data — `@sizumita`, `@tignear`, `100v`, two hard-coded avatar URLs and a
placeholder that is not even this site's — with 未承認/承認済み badges and a キャンセル /
承認する pair of buttons.

**And there is no earlier version to find.** `approve_application.ex` has exactly **one
blob** in the whole repository — the eight lines read just below — so no version of it
ever had an assign, and no `git log -S` will produce a flow. The rewrite did not delete
this page: **the old site never implemented the contract approval**, and what it shipped
is the mockup described below, down to buttons that carry no `phx-click`.

**And it was never implemented anywhere.** The Elm has no contract page either — its
routes are `DashboardPage`, `ClaimPage`, `ApplicationsPage` and `ErrorPage` — so
`/contract/:id` is a mockup in the LiveView and nothing more than a mockup in the site it
came from.

`Mypage/Claim.elm` is a different page and a real one: **the mypage's own claims**,
`ClaimType = Sent | Received`, paged, reading `GET /api/v2/users/@me/claims` (290 lines of
it). That API is implemented here and tested. **The SPA has no page for it**, which is an
item this list never had and now does — see the requirements below.

`/contract/:id` is routed (`router.ex` l.74) to
`live/contract/approve_application.ex`, and that module is eight lines: it aliases
`VirtualCrypto.Auth`, and its `mount` ignores both `params` and `session` and
answers `{:ok, socket}`. No assigns, no `handle_event`, no queries.

Its template is a hard-coded picture of what the page was meant to be: two named
users (`@sizumita`, `@tignear`), one 承認待ち and one 承認済み, a stock avatar from
unsplash, `100v from @sizumita` / `pay to @tignear`, and キャンセル and 承認する
buttons that carry no `phx-click` at all.

Nothing in `lib/` links to it — not the application pages, not the layout, not the
router beyond the route itself. So it is unreachable, unwired, and not backed by
anything: **there is no behaviour here to port.**

What it does hold is the shape somebody intended, which is worth keeping in view if
the feature is ever wanted: a contract proposes a transfer (`100v`, one user to
another) and the users it names approve or refuse it. That is a design to be made,
not a LiveView to be translated, and it has no endpoint, no table in this service
that is used, and no reader.

## How it is served

`web/dist` is the router's **fallback**, not a mount: `/api` and `/health` keep
their own routes and only what nothing else claims is treated as a client-side
route. That order is the whole risk of serving a SPA from an API's own origin,
and `tests/web.rs` pins it — an asset is served as it is, an unknown path answers
with `index.html`, and `/health` still answers JSON.

`WEB_ROOT` says where the built assets are (`web/dist` by default), because a
deployment unpacks them somewhere else. Two things follow: **the frontend has to
be built before the server is started**, and nothing yet gives the hashed assets
the immutable caching they are built for — they are served without a
`Cache-Control`, which is correct but wasteful.

## What the callback needs that is not there, verified

Two gaps, both found by looking rather than assuming:

- ~~`DiscordAuth.insert_user` does not exist here.~~ Written: `vc_core::user::insert_user`
  records the authorization and creates the account in one transaction. It
  composes `insert_if_not_exists` with an upsert rather than repeating either,
  and it is not yet covered by a test — `vc-core` has no test harness, so the
  callback's own test is where it will be exercised.
- **Token issuance exists only in the test support.** `vc_auth::jwt::sign` over
  a `Claims` of `sub`, `exp`, `iss`, `aud`, `jti`, `kind`, `scopes` and `typ` is
  how the tests mint tokens, and that builder lives in `tests/support/mod.rs`.
  Production has no equivalent, so the callback and `POST /token` have nothing to
  call — and when one exists, the test helper should call it rather than own it.

The scopes a browser login is issued are the old app's three:
`oauth2.register`, `vc.pay` and `vc.claim`, with `kind` of `user`. `Kind::App`
already exists for the client-credentials tokens the OAuth2 provider will need.

### The exact shape of a token, so it does not have to be rediscovered

Issuance is two steps, and the second is the one that is easy to miss: sign the
claims, **and** write the `jti` down. A token whose id is not recorded cannot be
revoked, and `Guardian.revoke/1` is a `DELETE` against exactly that row.

```sql
INSERT INTO user_access_tokens (user_id, token_id, expires, inserted_at, updated_at)
VALUES ($1, $2, $3, $4, $4)
```

and the claims, whose values the API verifier already expects:

```rust
Claims {
    sub:   user_id.to_string(),
    exp:   issued_at + 3600,
    iat:   Some(issued_at),
    nbf:   Some(issued_at),
    iss:   "virtualCrypto",          // vc_auth::ISSUER
    aud:   Some("virtualCrypto"),    // vc_auth::AUDIENCE
    jti:   uuid,                      // the same one that was inserted
    kind:  "user",
    scopes: [...],
    typ:   Some("access".to_string()),
}
```

The hour is Guardian's, not a choice, and `iat`/`nbf`/`typ` are all set — a
token missing any of them fails verification, which is how the test support
learned to build them.

## The application object, as an SPA receives it

One shape, built in one place, `routes/oauth2_clients.rs::render` (l.62), so an error in
it is an error in every endpoint that answers with it.

**Two of those endpoints exist here, not three.** This said three — the single read, the
list, the registration — and the third is the old site's, not this one's. The Elixir's
single read is `GET /applications/:id` (`ApplicationController.index`), a **browser**
page that takes a **uuid** (`UUID.info(id)`, so a non-uuid is a 404 before anything is
looked up), reads `Auth.get_user_application(user.id, id)` — the caller's own, and a 404
when it is not theirs or not there — and renders it. This service answers the list
(`GET /oauth2/clients/@me`) and the registration (`POST /oauth2/clients`), and the
frontend's detail page picks one out of the list by `client_id`, which is the same
question the Elixir's page asked, answered where the answer already was.

### `@me` means two different things, and the one here is the wrong one

**Corrected again, and this correction is the one that matters.** The paragraph that
stood here said the Elixir's four client-registration routes all have a counterpart and
that one of them changed path. Reading the controllers says otherwise.

In the Elixir:

| Route | What it is |
| --- | --- |
| `GET /oauth2/clients?user=@me` | the **list** — matched on the query parameter, and a request without it is 400 `invalid_request`/`required_user_parameter` |
| `POST /oauth2/clients` | registration, which answers `registration_client_uri: /oauth2/clients/@me` |
| `GET /oauth2/clients/@me` | the **client the token belongs to**, `kind == "app"` required — RFC 7592's read |
| `PATCH /oauth2/clients/@me` | the same client, edited — RFC 7592's update |

Here, `GET /oauth2/clients/@me` is the **list**, on a user token, and an app token is
answered 401 `invalid_kind`. So the path that registration hands out as
`registration_client_uri` (`routes/oauth2_clients.rs:559`, and `web/src/api.ts:216`) is
*not* the read it names: an RFC 7592 client that does what the registration told it to
do is refused.

**The read was missing**, which was a gap in the client-registration surface rather than
a naming difference — the same shape of mistake as the connect route taking the numeric
id, and written down here because the paragraph above said the opposite while looking
convincing.

**Fixed.** `GET /oauth2/clients/@me` is now the read: an **app** token, the same refusals
`PATCH` on that path already gave, answering the application its account belongs to
(`oauth2_clients.rs::me`). The list moved to the collection, `GET /oauth2/clients`, where
`POST /oauth2/clients` already was — the Elixir put the list behind a `user=@me` query
parameter that only ever had one valid value, and neither path is free to mean both.

The list's own tests moved with it (`oauth2_clients_mine.rs`), and the round trip test
that reads the list and connects with what it answered caught the move immediately, which
is the second time that test has earned its place. `oauth2_clients_me.rs` covers the read,
and its last test registers and then follows `registration_client_uri` with the token the
registration answered with.

That last test was first written off as untestable, on the claim that there was no
fixture for a stored Discord authorization. There is, and has been:
`support::insert_discord_auth`, with `set_discord_updated_at` and `discord_auth_row`
beside it. The claim came from not looking at `support`, which is how three of the
corrections in this document came about — so it is written down rather than quietly
fixed.

```
client_id, client_secret, client_secret_expires_at (0),
redirect_uris: string[],
user_id (string), discord_user_id (string|null),
application_type, client_name, client_uri, discord_support_server_invite_slug,
grant_types: string[], logo_uri,
owner_discord_id (string|null), response_types: string[], webhook_url,
subscribed_events: number[] (checked is sent, unchecked is not),
public_key (hex)
```

*The `subscribed_events` travel as numbers because the `type` values are numbers
on the wire — the same reason docs/oauth2.md gives. Checked is sent and
unchecked is not, and empty is nothing: what the read answers is the set the
row holds. `client_secret_expires_at` is a number too — see below.*

**`client_secret` is not null, and this said it was.** Registration stores one
(`vc-core/src/application.rs` l.604) into `applications.client_secret`, and `details`
selects that column, so every read that renders an application answers with it — the list
(`GET /oauth2/clients`), the RFC 7592 read (`GET /oauth2/clients/@me`), and the
registration. That is RFC 7592's shape, where the read of a client includes its secret.

What is **not** readable again is the `registration_access_token`: it is a token and not a
column, so registration is the only time it is answered — which is what the page showing
it says, and what it said wrongly about the secret until this was checked.

That the list carries them is right rather than merely tolerable. It is answered from
`owned_by` by the caller's **own** subject, so it can only ever be the applications that
caller already owns — and owning one is what lets you connect its bot, read it and patch
it. A secret shown to a party that can already rewrite the application is not a leak; it
is the same party. The RFC 7592 read is narrower still: it takes the application's own
token, so the application is reading itself.

The Elixir answered the same way and its detail page displayed them, which is where
"visible at any time" comes from. Reproduced, deliberately, and no longer disclaimed.

Two numbers again travel as strings, `user_id` and `discord_user_id`, which is the
same rule the balances follow. `client_secret_expires_at` is a number, and is the
second exception to it in this API, after `expires_in`: a zero rather than a null, as
the comment there says, so that a client reading an integer gets one.

`client_secret` is null wherever the caller should not see it, which is the list and
the single read; registration answers with it once.

### A list, and the Elixir's own answer for each kind

**Corrected.** An earlier version of this section said an account owns at most one
application and that the page should therefore be about *the* application. That was
wrong, and it came from reading the Rust `mine` instead of the Elixir:

`clients_controller.ex::get/2` (l.8-14) requires `kind == "user"` — an **app token is
answered 401 `invalid_token` / `invalid_kind`** (l.16-22) — and answers
`Auth.get_user_applications(user_id)`, which is plural. The index that is unique is
`users_application_id_index`, and what it makes unique is the link between an
application and *its own account* — the row registration inserts (l.637-642 of
`application.rs`), the one with no `discord_id`. `applications.owner_discord_id` has a
plain index (`applications_owner_discord_id_index`), so **a person may own several
applications**.

So the page is a list after all, and the Rust `mine` does not match the Elixir on
either branch:

- `Kind::App` answers the application — or `null` — where the Elixir answers 401.
- `Kind::User` looks the applications up through `users.application_id`, which is the
  *application account's* link and is null for a person. A person who owns an
  application would be answered `[]`. It should be looking through
  `applications.owner_discord_id`, by the account's `discord_id`.

Also from `render_application`, which is the shape: `owner_discord_id` goes through
`to_string` unconditionally, so it is **`""` rather than null** when absent — unlike
`discord_user_id`, two lines above it, which is checked.

## Before the connect route, one decision

Everything else about it is read: the two conditions, the six failures and the write
are above. What is not is **who may call it**, because the Elixir did not have to answer
that — the LiveView knew from the session which application the page was about, and this
is a JSON endpoint that will be given an id.

The obvious answer is the application's owner, and `vc_core::application::owned_by` is
already the read for it: the same one `GET /oauth2/clients/@me` uses to answer "the
applications this account owns". A route that took an application id and checked it
against that list would be consistent with the read endpoint next to it, and would not
need a new query.

The other thing to decide is the shape of the answers. The Elixir answers sentences
because a person is reading them in a page; six messages with numbers in them are not
what an API should send. The convention next door is `{"error": "...",
"error_description": "..."}` and a status — which would mean the six cases become a
smaller number of errors with the distinctions in the description, rather than one
error per sentence.

Both are decisions rather than readings, so they are here rather than in the code. What
is not a decision: the write is `bind_bot`, and its `Taken` outcome is
`すでにそのBotは別のApplicationに紐付けられています` — that one has a message because
the Elixir has one.

