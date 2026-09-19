/**
 * The URLs the one page navigates to.
 *
 * A navigation rather than a fetch: `/login` leaves for Discord and comes back,
 * and neither it nor `/logout` has a body to read. Nothing else lives here any
 * more — the pages that made API calls went with the pages themselves
 * (`docs/web-ui.md` records why), and a client with no caller is a client to keep
 * in step for no reason.
 */

/// The URL to navigate to in order to start a login — `/logout` is the other, and
/// nothing here calls it.
///
/// `continue` is a query parameter here. The old site derived it from the request
/// that failed, which an SPA has no way to see.
export function loginUrl(continueTo?: string): string {
  return continueTo === undefined
    ? "/login"
    : `/login?continue=${encodeURIComponent(continueTo)}`;
}
