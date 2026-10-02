//! The site's pages, as prose.
//!
//! Each one is a page of `/document/*` — see [`super::Page`] — and each is
//! written for someone who has never seen this service: what it is, how to start,
//! what an application is. The command list and the endpoint list are not here;
//! they are [`super::commands`] and [`super::api`], and a page says which of them
//! belongs under it with [`super::Listing`].
//!
//! The pages these replace said the right thing about a service that had no
//! applications, no contracts and no `/issue`. What is kept is the shape — what
//! the service is, what a currency is worth, how the pool grows — and the pool's
//! arithmetic is the implementation's rather than the documents': `vc-core`'s
//! `reset_pool_amount` counts the supply in accounts, floors the day's allowance
//! at 5 and the ceiling at 35, and rounds rather than ceils.

use super::{Listing, Page, api, lead, list, section, text};

/// Every page, in the order the navigation shows them.
pub fn all() -> &'static [Page] {
    &[START, COMMANDS, APPLICATIONS, api::PAGE, FAQ]
}

/// はじめに: what this is, and the shortest way to a balance somebody else can see.
const START: Page = Page {
    slug: "start",
    title: message!("docs.pages.all.001"),
    summary: message!("docs.pages.all.002"),
    listing: Listing::None,
    sections: &[
        lead(&[text(message!("docs.pages.all.003"))]),
        section(
            message!("docs.pages.all.004"),
            &[list(&[
                message!("docs.pages.all.005"),
                message!("docs.pages.all.006"),
                message!("docs.pages.all.007"),
                message!("docs.pages.all.008"),
                message!("docs.pages.all.009"),
            ])],
        ),
        section(
            message!("docs.pages.all.010"),
            &[
                list(&[
                    message!("docs.pages.all.011"),
                    message!("docs.pages.all.012"),
                    message!("docs.pages.all.013"),
                ]),
                text(message!("docs.pages.all.014")),
            ],
        ),
        section(
            message!("docs.pages.all.015"),
            &[
                text(message!("docs.pages.all.016")),
                text(message!("docs.pages.all.017")),
                text(message!("docs.pages.all.018")),
                text(message!("docs.pages.all.019")),
                text(message!("docs.pages.all.020")),
            ],
        ),
        section(
            message!("docs.pages.all.021"),
            &[
                text(message!("docs.pages.all.022")),
                text(message!("docs.pages.all.023")),
            ],
        ),
    ],
};

/// コマンド: how to read the list, then the list itself.
const COMMANDS: Page = Page {
    slug: "commands",
    title: message!("docs.pages.all.024"),
    summary: message!("docs.pages.all.025"),
    listing: Listing::Commands,
    sections: &[
        section(
            message!("docs.pages.all.026"),
            &[
                text(message!("docs.pages.all.027")),
                text(message!("docs.pages.all.028")),
            ],
        ),
        section(
            message!("docs.pages.all.029"),
            &[text(message!("docs.pages.all.030"))],
        ),
        section(
            message!("docs.pages.all.031"),
            &[text(message!("docs.pages.all.032"))],
        ),
    ],
};

/// アプリケーション連携: what an application is, and the four steps to using one.
const APPLICATIONS: Page = Page {
    slug: "applications",
    title: message!("docs.pages.all.033"),
    summary: message!("docs.pages.all.034"),
    listing: Listing::None,
    sections: &[
        lead(&[text(message!("docs.pages.all.035"))]),
        section(
            message!("docs.pages.all.036"),
            &[
                text(message!("docs.pages.all.037")),
                list(&[
                    message!("docs.pages.all.038"),
                    message!("docs.pages.all.039"),
                    message!("docs.pages.all.040"),
                    message!("docs.pages.all.041"),
                    message!("docs.pages.all.042"),
                    message!("docs.pages.all.043"),
                    message!("docs.pages.all.044"),
                    message!("docs.pages.all.045"),
                ]),
                text(message!("docs.pages.all.046")),
                text(message!("docs.pages.all.047")),
            ],
        ),
        section(
            message!("docs.pages.all.048"),
            &[
                text(message!("docs.pages.all.049")),
                text(message!("docs.pages.all.050")),
                text(message!("docs.pages.all.051")),
                text(message!("docs.pages.all.052")),
            ],
        ),
        section(
            message!("docs.pages.all.053"),
            &[
                text(message!("docs.pages.all.054")),
                text(message!("docs.pages.all.055")),
                text(message!("docs.pages.all.056")),
                text(message!("docs.pages.all.057")),
            ],
        ),
        section(
            message!("docs.pages.all.058"),
            &[
                text(message!("docs.pages.all.059")),
                text(message!("docs.pages.all.060")),
                text(message!("docs.pages.all.061")),
            ],
        ),
        section(
            message!("docs.pages.all.062"),
            &[
                text(message!("docs.pages.all.063")),
                list(&[
                    message!("docs.pages.all.064"),
                    message!("docs.pages.all.065"),
                    message!("docs.pages.all.066"),
                    message!("docs.pages.all.067"),
                ]),
                text(message!("docs.pages.all.068")),
                text(message!("docs.pages.all.069")),
            ],
        ),
        section(
            message!("docs.pages.all.070"),
            &[
                text(message!("docs.pages.all.071")),
                text(message!("docs.pages.all.072")),
            ],
        ),
    ],
};

/// よくある質問: the sentences people arrive with, answered where they are asked.
const FAQ: Page = Page {
    slug: "faq",
    title: message!("docs.pages.all.073"),
    summary: message!("docs.pages.all.074"),
    listing: Listing::None,
    sections: &[
        section(
            message!("docs.pages.all.075"),
            &[list(&[
                message!("docs.pages.all.076"),
                message!("docs.pages.all.077"),
                message!("docs.pages.all.078"),
                message!("docs.pages.all.079"),
            ])],
        ),
        section(
            message!("docs.pages.all.080"),
            &[list(&[
                message!("docs.pages.all.081"),
                message!("docs.pages.all.082"),
                message!("docs.pages.all.083"),
                message!("docs.pages.all.084"),
            ])],
        ),
        section(
            message!("docs.pages.all.085"),
            &[list(&[
                message!("docs.pages.all.086"),
                message!("docs.pages.all.087"),
                message!("docs.pages.all.088"),
                message!("docs.pages.all.089"),
            ])],
        ),
        section(
            message!("docs.pages.all.090"),
            &[list(&[
                message!("docs.pages.all.091"),
                message!("docs.pages.all.092"),
                message!("docs.pages.all.093"),
                message!("docs.pages.all.094"),
                message!("docs.pages.all.095"),
            ])],
        ),
        section(
            message!("docs.pages.all.096"),
            &[
                text(message!("docs.pages.all.097")),
                text(message!("docs.pages.all.098")),
            ],
        ),
    ],
};
