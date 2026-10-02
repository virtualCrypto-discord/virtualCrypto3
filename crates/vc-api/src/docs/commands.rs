//! What each command is for, in the words a person reads.
//!
//! One entry per command, in the order [`crate::discord_commands`] registers
//! them. What is here is the part Discord's own description has no room for: how
//! the command is typed, what it does to a balance, and the sentences its
//! refusals leave a person with.
//!
//! **Nothing here repeats a name, a description or an option.** Those are the
//! registration payload's, and [`super::showings`] joins the two. Everything
//! below was written from the handlers (`crate::command`) and the reads they
//! call (`vc_core`), not from the old site's documents, whose `/give` and
//! `[unit] [user]` never matched this service.

use super::{Command, Section, code, list, section, text};

/// Every command's prose, in registration order.
pub fn all() -> &'static [Command] {
    &[
        Command {
            name: "help",
            usage: &["/help", message!("docs.commands.all.001")],
            sections: HELP,
        },
        Command {
            name: "invite",
            usage: &["/invite"],
            sections: INVITE,
        },
        Command {
            name: "application",
            usage: &[
                "/application register",
                "/application list",
                message!("docs.commands.all.002"),
            ],
            sections: APPLICATION,
        },
        Command {
            name: "issue",
            usage: &[
                message!("docs.commands.all.003"),
                message!("docs.commands.all.004"),
            ],
            sections: ISSUE,
        },
        Command {
            name: "pat",
            usage: &[message!("docs.commands.all.005"), "/pat list"],
            sections: PAT,
        },
        Command {
            name: "grant",
            usage: &["/grant user", "/grant server", "/grant approve code:<code>"],
            sections: GRANT,
        },
        Command {
            name: "contract",
            usage: &["/contract list"],
            sections: CONTRACT,
        },
        Command {
            name: "pay",
            usage: &[message!("docs.commands.all.006")],
            sections: PAY,
        },
        Command {
            name: "info",
            usage: &[
                "/info",
                message!("docs.commands.all.007"),
                message!("docs.commands.all.008"),
            ],
            sections: INFO,
        },
        Command {
            name: "create",
            usage: &[message!("docs.commands.all.009")],
            sections: CREATE,
        },
        Command {
            name: "delete",
            usage: &["/delete"],
            sections: DELETE,
        },
        Command {
            name: "bal",
            usage: &["/bal"],
            sections: BAL,
        },
        Command {
            name: "claim",
            usage: &[
                "/claim list",
                "/claim received",
                "/claim sent",
                message!("docs.commands.all.010"),
                message!("docs.commands.all.011"),
                message!("docs.commands.all.012"),
                message!("docs.commands.all.013"),
                message!("docs.commands.all.014"),
            ],
            sections: CLAIM,
        },
        Command {
            name: "mute",
            usage: &[
                message!("docs.commands.all.015"),
                message!("docs.commands.all.016"),
                "/mute list",
            ],
            sections: MUTE,
        },
        Command {
            name: "history",
            usage: &[
                "/history pay",
                message!("docs.commands.all.017"),
                "/history issue",
                message!("docs.commands.all.018"),
            ],
            sections: HISTORY,
        },
    ]
}

const HELP: &[Section] = &[
    section(
        message!("docs.commands.all.019"),
        &[
            text(message!("docs.commands.all.020")),
            text(message!("docs.commands.all.021")),
        ],
    ),
    section(
        message!("docs.commands.all.022"),
        &[list(&[
            message!("docs.commands.all.023"),
            message!("docs.commands.all.024"),
        ])],
    ),
];

const INVITE: &[Section] = &[section(
    message!("docs.commands.all.025"),
    &[
        text(message!("docs.commands.all.026")),
        list(&[
            message!("docs.commands.all.027"),
            message!("docs.commands.all.028"),
        ]),
    ],
)];

const APPLICATION: &[Section] = &[
    section(
        message!("docs.commands.all.029"),
        &[
            list(&[
                message!("docs.commands.all.030"),
                message!("docs.commands.all.031"),
                message!("docs.commands.all.032"),
            ]),
            text(message!("docs.commands.all.033")),
            text(message!("docs.commands.all.034")),
        ],
    ),
    section(
        message!("docs.commands.all.035"),
        &[text(message!("docs.commands.all.036"))],
    ),
];

const ISSUE: &[Section] = &[
    section(
        message!("docs.commands.all.037"),
        &[
            text(message!("docs.commands.all.038")),
            text(message!("docs.commands.all.039")),
        ],
    ),
    section(
        message!("docs.commands.all.040"),
        &[
            text(message!("docs.commands.all.041")),
            text(message!("docs.commands.all.042")),
            text(message!("docs.commands.all.043")),
        ],
    ),
    section(
        message!("docs.commands.all.044"),
        &[text(message!("docs.commands.all.045"))],
    ),
];

const GRANT: &[Section] = &[
    section(
        message!("docs.commands.all.046"),
        &[
            text(message!("docs.commands.all.047")),
            list(&[
                message!("docs.commands.all.048"),
                message!("docs.commands.all.049"),
                message!("docs.commands.all.050"),
            ]),
        ],
    ),
    section(
        message!("docs.commands.all.051"),
        &[text(message!("docs.commands.all.052"))],
    ),
];

const CONTRACT: &[Section] = &[
    section(
        message!("docs.commands.all.053"),
        &[
            text(message!("docs.commands.all.054")),
            text(message!("docs.commands.all.055")),
            text(message!("docs.commands.all.056")),
            text(message!("docs.commands.all.057")),
            text(message!("docs.commands.all.058")),
        ],
    ),
    section(
        message!("docs.commands.all.059"),
        &[text(message!("docs.commands.all.060"))],
    ),
];

const PAT: &[Section] = &[
    section(
        message!("docs.commands.all.061"),
        &[
            text(message!("docs.commands.all.062")),
            list(&[
                message!("docs.commands.all.063"),
                message!("docs.commands.all.064"),
            ]),
        ],
    ),
    section(
        message!("docs.commands.all.065"),
        &[text(message!("docs.commands.all.066"))],
    ),
    section(
        message!("docs.commands.all.067"),
        &[text(message!("docs.commands.all.068"))],
    ),
    section(
        message!("docs.commands.all.069"),
        &[text(message!("docs.commands.all.070"))],
    ),
];

const PAY: &[Section] = &[
    section(
        message!("docs.commands.all.071"),
        &[text(message!("docs.commands.all.072"))],
    ),
    section(
        message!("docs.commands.all.073"),
        &[code(&[message!("docs.commands.all.074")])],
    ),
    section(
        message!("docs.commands.all.075"),
        &[
            text(message!("docs.commands.all.076")),
            text(message!("docs.commands.all.077")),
        ],
    ),
    section(
        message!("docs.commands.all.078"),
        &[list(&[
            message!("docs.commands.all.079"),
            message!("docs.commands.all.080"),
            message!("docs.commands.all.081"),
        ])],
    ),
];

const INFO: &[Section] = &[
    section(
        message!("docs.commands.all.082"),
        &[
            text(message!("docs.commands.all.083")),
            text(message!("docs.commands.all.084")),
        ],
    ),
    section(
        message!("docs.commands.all.085"),
        &[list(&[
            message!("docs.commands.all.086"),
            message!("docs.commands.all.087"),
            message!("docs.commands.all.088"),
            message!("docs.commands.all.089"),
            message!("docs.commands.all.090"),
            message!("docs.commands.all.091"),
            message!("docs.commands.all.092"),
        ])],
    ),
    section(
        message!("docs.commands.all.093"),
        &[text(message!("docs.commands.all.094"))],
    ),
];

const CREATE: &[Section] = &[
    section(
        message!("docs.commands.all.095"),
        &[
            text(message!("docs.commands.all.096")),
            text(message!("docs.commands.all.097")),
        ],
    ),
    section(
        message!("docs.commands.all.098"),
        &[list(&[
            message!("docs.commands.all.099"),
            message!("docs.commands.all.100"),
            message!("docs.commands.all.101"),
        ])],
    ),
    section(
        message!("docs.commands.all.102"),
        &[
            text(message!("docs.commands.all.103")),
            text(message!("docs.commands.all.104")),
            text(message!("docs.commands.all.105")),
        ],
    ),
    section(
        message!("docs.commands.all.106"),
        &[list(&[
            message!("docs.commands.all.107"),
            message!("docs.commands.all.108"),
            message!("docs.commands.all.109"),
            message!("docs.commands.all.110"),
            message!("docs.commands.all.111"),
        ])],
    ),
];

const DELETE: &[Section] = &[
    section(
        message!("docs.commands.all.112"),
        &[
            text(message!("docs.commands.all.113")),
            text(message!("docs.commands.all.114")),
        ],
    ),
    section(
        message!("docs.commands.all.115"),
        &[text(message!("docs.commands.all.116"))],
    ),
    section(
        message!("docs.commands.all.117"),
        &[list(&[
            message!("docs.commands.all.118"),
            message!("docs.commands.all.119"),
            message!("docs.commands.all.120"),
        ])],
    ),
];

const BAL: &[Section] = &[section(
    message!("docs.commands.all.121"),
    &[
        text(message!("docs.commands.all.122")),
        text(message!("docs.commands.all.123")),
        text(message!("docs.commands.all.124")),
    ],
)];

const CLAIM: &[Section] = &[
    section(
        message!("docs.commands.all.125"),
        &[text(message!("docs.commands.all.126"))],
    ),
    section(
        message!("docs.commands.all.127"),
        &[
            list(&[
                message!("docs.commands.all.128"),
                message!("docs.commands.all.129"),
                message!("docs.commands.all.130"),
                message!("docs.commands.all.131"),
                message!("docs.commands.all.132"),
                message!("docs.commands.all.133"),
                message!("docs.commands.all.134"),
                message!("docs.commands.all.135"),
            ]),
            text(message!("docs.commands.all.136")),
        ],
    ),
    section(
        message!("docs.commands.all.137"),
        &[
            text(message!("docs.commands.all.138")),
            text(message!("docs.commands.all.139")),
            text(message!("docs.commands.all.140")),
        ],
    ),
    section(
        message!("docs.commands.all.141"),
        &[list(&[
            message!("docs.commands.all.142"),
            message!("docs.commands.all.143"),
            message!("docs.commands.all.144"),
            message!("docs.commands.all.145"),
        ])],
    ),
    section(
        message!("docs.commands.all.146"),
        &[text(message!("docs.commands.all.147"))],
    ),
    section(
        message!("docs.commands.all.148"),
        &[text(message!("docs.commands.all.149"))],
    ),
];

const MUTE: &[Section] = &[
    section(
        message!("docs.commands.all.150"),
        &[
            text(message!("docs.commands.all.151")),
            list(&[
                message!("docs.commands.all.152"),
                message!("docs.commands.all.153"),
                message!("docs.commands.all.154"),
            ]),
        ],
    ),
    section(
        message!("docs.commands.all.155"),
        &[text(message!("docs.commands.all.156"))],
    ),
    section(
        message!("docs.commands.all.157"),
        &[text(message!("docs.commands.all.158"))],
    ),
];

const HISTORY: &[Section] = &[
    section(
        message!("docs.commands.all.159"),
        &[
            text(message!("docs.commands.all.160")),
            list(&[
                message!("docs.commands.all.161"),
                message!("docs.commands.all.162"),
                message!("docs.commands.all.163"),
                message!("docs.commands.all.164"),
            ]),
        ],
    ),
    section(
        message!("docs.commands.all.165"),
        &[
            text(message!("docs.commands.all.166")),
            text(message!("docs.commands.all.167")),
            text(message!("docs.commands.all.168")),
        ],
    ),
    section(
        message!("docs.commands.all.169"),
        &[
            text(message!("docs.commands.all.170")),
            text(message!("docs.commands.all.171")),
            text(message!("docs.commands.all.172")),
            text(message!("docs.commands.all.173")),
        ],
    ),
    section(
        message!("docs.commands.all.174"),
        &[text(message!("docs.commands.all.175"))],
    ),
];
