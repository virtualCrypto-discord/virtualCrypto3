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
export function applications(token: string): Promise<Application[]> {
  return request("/oauth2/clients/@me", {
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
