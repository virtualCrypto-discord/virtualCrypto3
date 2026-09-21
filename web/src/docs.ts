// The documentation as the service answers it.
//
// One read, and the pages the SPA draws from it. The shape is written by
// `crates/vc-api/src/docs/json.rs` and by nothing else — prose arrives with its
// marks already taken off (`Span`), so nothing here parses or escapes anything.

/** A run of prose. `href` is the only mark that changes what happens on a click. */
export type Span = {
  text: string;
  bold: boolean;
  code: boolean;
  href: string | null;
};

export type Block =
  | { kind: "text"; spans: Span[] }
  | { kind: "list"; items: Span[][] }
  | { kind: "code"; lines: string[] };

export type Section = {
  heading: string | null;
  blocks: Block[];
};

/** What a page shows under its prose: the list it is a page of, if any. */
export type Listing = "commands" | "endpoints" | null;

/** Prose travels as spans; `slug`, `title`, `usage`, `method` and `path` do not:
 *  a mark in a tooltip or in a fence is a mark people would see. */
export type Page = {
  slug: string;
  title: string;
  /** Plain, because it is a tooltip for the navigation. */
  summary: string;
  listing: Listing;
  sections: Section[];
};

export type Option = {
  name: string;
  description: Span[];
  required: boolean;
  /** Discord's type, as a word: `subcommand`, `text`, `integer`, `boolean`, `user`. */
  kind: string;
  /** The same word in Japanese, so the page does not keep a second copy of it. */
  kind_label: string;
  autocomplete: boolean;
  children: Option[];
};

export type Command = {
  name: string;
  description: Span[];
  admin_only: boolean;
  /** Input lines, shown in a fence. */
  usage: string[];
  options: Option[];
  sections: Section[];
};

export type Endpoint = {
  method: string;
  path: string;
  summary: Span[];
  access: Span[];
  /** The body's or query's fields, one line each: name, type, required, meaning. */
  fields: Span[][];
  /**
   * One call and the answer it gets, each as the lines of a fence. `null` for an
   * endpoint whose answer is a redirect rather than a body.
   */
  example: { request: string[]; response: string[] } | null;
  notes: Span[][];
};

export type Guide = {
  pages: Page[];
  commands: Command[];
  endpoints: Endpoint[];
};

/** The whole document. Throws what `fetch` throws, plus a status for a bad one. */
export async function load(): Promise<Guide> {
  const response = await fetch("/api/docs", {
    headers: { Accept: "application/json" },
  });

  if (!response.ok) {
    throw new Error(`GET /api/docs answered ${response.status}`);
  }

  return (await response.json()) as Guide;
}

/** One option of a subcommand, and how deep it sits. */
export type PlacedOption = { option: Option; depth: number };

/**
 * The options in reading order, each with its depth: a subcommand's own options
 * come after it, one step in. Flattened here rather than nested in the markup,
 * because a list is what the page shows and a recursive component would be a
 * second way to say `depth`.
 */
export function flatten(options: Option[], depth = 0): PlacedOption[] {
  return options.flatMap((option) => [
    { option, depth },
    ...flatten(option.children, depth + 1),
  ]);
}
