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

/// The account the session belongs to.
export function me(): Promise<Account> {
  return request("/api/v2/users/@me") as Promise<Account>;
}

/// What that account holds, one entry per currency.
export function balances(): Promise<Balance[]> {
  return request("/api/v2/users/@me/balances") as Promise<Balance[]>;
}

export function currencies(): Promise<Currency[]> {
  return request("/api/v2/currencies") as Promise<Currency[]>;
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
