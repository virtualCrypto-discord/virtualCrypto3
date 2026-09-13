/**
 * The API, as the SPA needs it.
 *
 * Every path here is same-origin: in development Vite proxies `/api`, `/oauth2`
 * and `/token` to the API, and in production the API serves these files itself.
 * That is why there is no base URL in this file and no CORS anywhere in either.
 *
 * The session cookie is what identifies a caller, so every request asks for
 * credentials. A request with no session is answered 401 — and where to send the
 * browser to fix that is the `location` of the error body.
 */

/// What the API answers with when it refuses, which is the OAuth2 shape on every
/// endpoint, not only on `/oauth2/token`:
/// `{"error": "invalid_token", "error_description": "..."}`.
export interface ApiErrorBody {
  error: string;
  error_description?: string;
}

export class ApiError extends Error {
  readonly status: number;
  readonly body: ApiErrorBody | undefined;
  readonly loginUrl: string | undefined;

  constructor(status: number, body: ApiErrorBody | undefined, loginUrl?: string) {
    super(body?.error_description ?? body?.error ?? `HTTP ${status}`);

    this.name = "ApiError";
    this.status = status;
    this.body = body;
    this.loginUrl = loginUrl;
  }

  /// Whether refreshing the session would help.
  get needsLogin(): boolean {
    return this.status === 401;
  }
}

/// A bearer token, for the calls that want one. The reads below take it as an
/// argument rather than holding it: a module-level token would outlive the session
/// it came from, and the page that has one knows where to keep it.
function authorize(token?: string): HeadersInit {
  return token === undefined ? {} : { Authorization: `Bearer ${token}` };
}

async function request(path: string, init: RequestInit = {}): Promise<unknown> {
  const response = await fetch(path, {
    ...init,
    credentials: "include",
    headers: { Accept: "application/json", ...init.headers },
  });

  if (response.status === 204) {
    return undefined;
  }

  const body: unknown = await response.json().catch(() => undefined);

  if (!response.ok) {
    const errorBody = body as ApiErrorBody | undefined;

    throw new ApiError(
      response.status,
      errorBody,
      // A 401 from an XHR carries where the browser should be sent instead; the
      // same request made by navigating would have been redirected there.
      response.headers.get("location") ?? undefined,
    );
  }

  return body;
}

/// Numbers arrive as strings — amounts, pool sizes, guild ids — because they are
/// `bigint` in the database and JSON has no integer wide enough to be trusted with
/// them. Nothing in this file parses them; a caller that needs arithmetic should
/// say so explicitly rather than get it by accident.
export interface Account {
  id: number;
  discord_user_id: string | null;
  application_id: string | null;
}

export interface Currency {
  guild: string;
  name: string | null;
  pool_amount: string | null;
  unit: string | null;
}

export interface Balance {
  amount: string | null;
  currency: Currency;
}

/// What `POST /token` answers with. `expires_in` is seconds, and it is a JSON
/// number here — the one number in this API that is not a string, because it is
/// computed rather than stored.
export interface IssuedToken {
  access_token: string;
  expires_in: number;
}

/// Exchanges the session cookie for a token.
///
/// A POST with no body: what identifies the caller is the cookie. The callback
/// deliberately does not issue one — a token written during a navigation is a token
/// nobody holds and cannot be revoked — so this is where a browser gets its own.
export function exchangeToken(): Promise<IssuedToken> {
  return request("/token", { method: "POST" }) as Promise<IssuedToken>;
}

/// An application, as `render` builds it. One shape, answered by the single read,
/// the list and the registration.
///
/// Two more numbers travel as strings — `user_id` and `discord_user_id` — as the
/// balances do. `client_secret_expires_at` does not: it is a zero rather than a
/// null so that a client reading an integer gets one, and `expires_in` is the other
/// such exception.
///
/// `owner_discord_id` is a string even when there is none: `render_application`
/// puts it through `to_string` unconditionally, so an absent one is `""` — unlike
/// `discord_user_id`, two lines above it, which is checked. Both spellings are in
/// the Elixir and this keeps them.
///
/// `client_secret` is null wherever the caller should not see it, which is here.
export interface Application {
  client_id: string;
  client_secret: string | null;
  client_secret_expires_at: number;
  redirect_uris: string[];
  user_id: string;
  discord_user_id: string | null;
  application_type: string;
  client_name: string | null;
  client_uri: string | null;
  discord_support_server_invite_slug: string | null;
  grant_types: string[];
  logo_uri: string | null;
  owner_discord_id: string;
  response_types: string[];
  webhook_url: string | null;
  public_key: string;
}

/// The applications the token's subject owns.
///
/// A user token's, and the endpoint refuses any other kind with 401 `invalid_kind` —
/// an application cannot ask which applications it owns. The list is by
/// `owner_discord_id`, so it is none, one, or several.
///
/// The path is the **collection**, and `/oauth2/clients/@me` is not it: that one is the
/// application's read of itself, with an application token, which is what
/// `registration_client_uri` names. The two shared a path until the Elixir's controllers
/// were read.
export function applications(token: string): Promise<Application[]> {
  return request("/oauth2/clients", {
    headers: authorize(token),
  }) as Promise<Application[]>;
}

/// The account the token belongs to.
export function me(token: string): Promise<Account> {
  return request("/api/v2/users/@me", { headers: authorize(token) }) as Promise<Account>;
}

/// What that account holds, one entry per currency.
export function balances(token: string): Promise<Balance[]> {
  return request("/api/v2/users/@me/balances", {
    headers: authorize(token),
  }) as Promise<Balance[]>;
}

export function currencies(token: string): Promise<Currency[]> {
  return request("/api/v2/currencies", { headers: authorize(token) }) as Promise<Currency[]>;
}

/// The URL to navigate to in order to start a login, and the URL to navigate to in
/// order to end one. Both are navigations rather than fetches: the first leaves for
/// Discord and comes back, and neither has a body to read.
///
/// `continue` is a query parameter here. The old site derived it from the request
/// that failed, which an SPA has no way to see.
export function loginUrl(continueTo?: string): string {
  return continueTo === undefined
    ? "/login"
    : `/login?continue=${encodeURIComponent(continueTo)}`;
}

export const logoutUrl = "/logout";

/// A registration request. Every field is optional except `redirect_uris`, and the
/// names are the API's — snake_case, as the endpoints that read them use.
///
/// `owner_discord_id` is deliberately not here: the handler takes it from the account
/// the token belongs to, having asked Discord, rather than from the request. A form
/// that sent one would be sending something nobody reads.
export interface Registration {
  client_name?: string;
  redirect_uris: string[];
  client_uri?: string;
  logo_uri?: string;
  webhook_url?: string;
  discord_support_server_invite_slug?: string;
  application_type?: string;
  grant_types?: string[];
  response_types?: string[];
}

/// What a registration answers with, and the one moment the secret is legible.
///
/// `registration_access_token` is an application token for what was just created, not
/// a token for the person who registered it: it is what an OIDC client would use to
/// read and edit its own registration.
export interface Registered {
  client_id: string;
  client_secret: string;
  registration_access_token: string;
  registration_client_uri: string;
  client_secret_expires_at: number;
}

/// Registers an application for the token's account.
///
/// If `webhook_url` is given, this does not answer until the service has asked that
/// URL to prove it can answer a signed request: two requests, and a 401 for the one
/// signed with a key the application has never seen. So a form calling this is a form
/// that can take a while, and one that fails for reasons on the far side of the
/// internet.
export function register(token: string, body: Registration): Promise<Registered> {
  return request("/oauth2/clients", {
    method: "POST",
    headers: { ...authorize(token), "Content-Type": "application/json" },
    body: JSON.stringify(body),
  }) as Promise<Registered>;
}

/// What a `PATCH` asks to change.
///
/// The three states are the endpoint's, and they are not the same operation:
///
/// - **absent** — the field is left alone.
/// - **`null`** — the field is cleared.
/// - **a value** — the field is set.
///
/// That is why this is not `Partial<Registration>`: there, absent and null would be
/// one thing, and the endpoint treats them as two. A `null` here is `logo_uri: null`
/// on the wire, which clears it, so a form must omit what it did not touch rather
/// than send what it read.
///
/// `redirect_uris` is the exception: it is a list, and supplying one replaces the set
/// wholesale — an empty list is how they are all removed. There is no null for it,
/// because there is no column to null.
export interface ApplicationChanges {
  client_name?: string | null;
  client_uri?: string | null;
  logo_uri?: string | null;
  webhook_url?: string | null;
  discord_support_server_invite_slug?: string | null;
  application_type?: string | null;
  redirect_uris?: string[];
}

/// Edits the application the token is for. **The token has to be an application's**:
/// the endpoint refuses anything but `kind: app` with `invalid_kind`, because an
/// application editing itself is what it is for (RFC 7592).
///
/// That is not the token a browser holds. `POST /token` answers a user token, so a
/// page cannot call this with its session — it needs the `registration_access_token`
/// that registration answered with, which belongs to the application's operator and is
/// good for as long as they keep it.
///
/// Answers 204 and nothing else — the read is a separate request, and a caller that
/// wants to show the result has to ask again. Naming a `webhook_url` also runs the
/// handshake, signed with the key the application already has rather than a new one,
/// so that what it is asked to verify is what it will be sent.
export function editApplication(
  token: string,
  changes: ApplicationChanges,
): Promise<void> {
  return request("/oauth2/clients/@me", {
    method: "PATCH",
    headers: { ...authorize(token), "Content-Type": "application/json" },
    body: JSON.stringify(changes),
  }) as Promise<void>;
}

/// What a connect attempt sends.
///
/// Both ids are strings, and they must be. A Discord id is a snowflake around 10^18,
/// and JavaScript's number is a double, which counts exactly only to 2^53 — so
/// `Number(guild_id)` silently sends a *different* guild than the one that was typed.
/// The digits are carried as text and parsed by the service, which is where an `i64`
/// is exact.
export interface Connection {
  bot_id: string;
  guild_id: string;
}

/// Binds an application to the bot that speaks for it.
///
/// The path carries the **client id**, which is what `/applications/:id` means in the
/// site this replaces: its list links `"/applications/" ++ application.client_id`, and
/// its connect route is the same `:id`. The application's numeric id is not part of any
/// response here, so it is not something a caller could name.
///
/// The caller must own the application; the service answers 404 to anyone else,
/// deliberately, rather than saying whether the id it was given exists.
///
/// Answers 204 on success and nothing else. Every failure is a sentence in
/// `error_description` about a specific state of the world — which of "the bot is not
/// in that server", "the integration does not name this application", "that id is not
/// a bot" it is — because the point of the check is that the operator has one thing to
/// fix and is told which.
export function connect(
  token: string,
  clientId: string,
  connection: Connection,
): Promise<void> {
  return request(`/applications/${encodeURIComponent(clientId)}/connect`, {
    method: "POST",
    headers: { ...authorize(token), "Content-Type": "application/json" },
    body: JSON.stringify(connection),
  }) as Promise<void>;
}
