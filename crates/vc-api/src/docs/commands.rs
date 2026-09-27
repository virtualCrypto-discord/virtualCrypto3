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
            usage: &["/help", "/help command:<コマンド名>"],
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
                "/application show client_id:<アプリケーション>",
            ],
            sections: APPLICATION,
        },
        Command {
            name: "issue",
            usage: &["/issue user:<発行先>", "/issue user:<発行先> amount:<枚数>"],
            sections: ISSUE,
        },
        Command {
            name: "pat",
            usage: &["/pat create name:<名前>", "/pat list"],
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
            usage: &["/pay unit:<通貨の単位> user:<送信先> amount:<枚数>"],
            sections: PAY,
        },
        Command {
            name: "info",
            usage: &["/info", "/info name:<通貨名>", "/info unit:<単位>"],
            sections: INFO,
        },
        Command {
            name: "create",
            usage: &["/create name:<通貨名> unit:<単位> amount:<最初の発行枚数>"],
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
                "/claim make user:<請求先> unit:<通貨の単位> amount:<枚数>",
                "/claim show id:<請求番号>",
                "/claim approve id:<請求番号>",
                "/claim deny id:<請求番号>",
                "/claim cancel id:<請求番号>",
            ],
            sections: CLAIM,
        },
        Command {
            name: "mute",
            usage: &[
                "/mute currency unit:<通貨の単位>",
                "/mute user user:<相手>",
                "/mute list",
            ],
            sections: MUTE,
        },
        Command {
            name: "unmute",
            usage: &[
                "/unmute currency unit:<通貨の単位>",
                "/unmute user user:<相手>",
            ],
            sections: UNMUTE,
        },
        Command {
            name: "history",
            usage: &[
                "/history pay",
                "/history pay unit:<通貨の単位> user:<相手>",
                "/history issue",
                "/history issue user:<相手>",
            ],
            sections: HISTORY,
        },
    ]
}

const HELP: &[Section] = &[
    section(
        "使い方",
        &[
            text(
                "引数を付けずに実行すると、すべてのコマンドの一覧を表示します。\
                 一覧のメニューからコマンドを選ぶと、そのコマンドの使い方を表示します。",
            ),
            text("`/help command:<コマンド名>` を使うと、最初から1つのコマンドの詳細を開けます。"),
        ],
    ),
    section(
        "同じ内容をWebで読む",
        &[list(&[
            "コマンドの使い方: [コマンド]({site}/document/commands)",
            "サービスのはじめかた: [はじめに]({site}/document/start)",
        ])],
    ),
];

const INVITE: &[Section] = &[section(
    "使い方",
    &[
        text("Botをサーバーに追加するための招待URLと、サポートサーバーの招待URLを表示します。"),
        list(&[
            "Botの招待: [招待URL]({invite})",
            "サポートサーバー: [招待URL]({support})",
        ]),
    ],
)];

const APPLICATION: &[Section] = &[
    section(
        "サブコマンド",
        &[
            list(&[
                "`register` 新しいアプリケーションを登録します。",
                "`list` 自分が持つアプリケーションの一覧を表示します。",
                "`show` アプリケーションの詳細を表示します。設定の変更とBotの接続はこの画面から行います。",
            ]),
            text(
                "登録したときに発行される `client_id` と `client_secret` は、\
                 `show` の画面に表示されます。",
            ),
            text(
                "Botを接続するには、先にそのBotのプロフィール（説明）へ、\
                 `show` の画面に出るトークン（`{site}/applications/verification?q=<client_id>`）を\
                 追記してください。そのあと `show` の画面の一覧でBotを選ぶと接続できます。\
                 接続できるサーバーは、そのときコマンドを実行しているサーバーです。",
            ),
        ],
    ),
    section(
        "詳しく",
        &[text(
            "登録からBotの接続、発行の許可までの流れは、\
             [アプリケーション連携]({site}/document/applications)をご覧ください。",
        )],
    ),
];

const ISSUE: &[Section] = &[
    section(
        "使い方",
        &[
            text(
                "サーバーの発行枠から、指定したユーザーに通貨を発行します。\
                 実行には管理者権限が必要で、サーバーの中でだけ実行できます。",
            ),
            text("`amount` を省略すると、そのときの発行枠のすべてが発行されます。"),
        ],
    ),
    section(
        "発行枠",
        &[
            text(
                "発行枠は、その通貨の総発行量をもとに増えます。\
                 1日に1回、総発行量の0.5%が加算され、上限は総発行量の3.5%です。\
                 加算は最小5、上限は最小35で、端数は丸められます。",
            ),
            text(
                "ここでいう総発行量は、これまでに発行した額の合計です。\
                 まだ発行していない発行枠そのものは含みません。\
                 契約でロックされている通貨は、契約の口座にあるので含まれます。",
            ),
            text("いまの発行枠は `/info` で確認できます。"),
        ],
    ),
    section(
        "結果",
        &[text(
            "発行できたときは、発行した枚数と残りの発行枠が表示されます。",
        )],
    ),
];

const GRANT: &[Section] = &[
    section(
        "Usage",
        &[
            text("Review requests to access your account or issue currency in your server."),
            list(&[
                "`approve code:` Review the application, target, permissions and currencies, then confirm with the Approve button. Nothing is granted by entering the code alone.",
                "`user` List and revoke applications with access to your account. Available in servers and DMs.",
                "`server` List and revoke applications allowed to issue in this server. Requires administrator permission in that server.",
            ]),
        ],
    ),
    section(
        "Who may approve",
        &[text(
            "Personal requests can only be approved by the requested user. Server requests require an administrator in the requested server. Every button rechecks these permissions.",
        )],
    ),
];

const CONTRACT: &[Section] = &[
    section(
        "使い方",
        &[
            text(
                "あなたが対象になっている契約の一覧を表示します。\
                 契約は、アプリケーションがあなたの通貨を決めた量だけロックして使うための取り決めです。",
            ),
            text(
                "承認すると、その分の通貨があなたの残高から契約に移り、アプリケーションが使えるようになります。\
                 承認・拒否は画面のボタンで行います。",
            ),
            text(
                "承認したあとでも、期限のない契約はいつでも取り消せます。\
                 期限のある契約は、その期間が終わると取り消せます。",
            ),
            text("このコマンドはサーバーの中でもDMでも実行できます。"),
            text(
                "`/mute` で指定した通貨と相手の契約は、一覧に表示されません。\
                 件数にも含まれません。",
            ),
        ],
    ),
    section(
        "ロックされた通貨",
        &[text(
            "ロックされた通貨が消えることはありません。\
             アプリケーションが使わなかった分は、契約が終わるとあなたに戻ります。",
        )],
    ),
];

const PAT: &[Section] = &[
    section(
        "使い方",
        &[
            text(
                "個人アクセストークン（PAT）を持つツールは、あなたとしてAPIを操作できます。\
                 アプリケーションの登録と接続、残高の参照、送金、請求、契約の承認などができます。\
                 発行時に権限を絞ることはできません。",
            ),
            list(&[
                "`/pat create name:<名前>` トークンを1つ作ります。値は実行した本人だけに、この返信で一度だけ表示されます。",
                "`/pat list` 名前を10件ずつ表示します。対象の名前の横にある「失効」ボタンで失効させます。",
            ]),
        ],
    ),
    section(
        "失効",
        &[text(
            "有効期限はありません。`/pat list` の「失効」ボタンを押すと、直ちにAPIで使えなくなります。\
             そのトークンを使うツールも動かなくなります。トークンの値は再表示できません。",
        )],
    ),
    section(
        "名前",
        &[text(
            "名前は1〜32文字で、同じ名前は1つだけです。1アカウントに25個まで作れます。\
             使うツールや用途が分かる名前にしてください。",
        )],
    ),
    section(
        "気をつけること",
        &[text(
            "トークンはパスワードと同じです。他人に見せないでください。\
             見せてしまったときは `/pat list` で対象の「失効」ボタンを押してください。",
        )],
    ),
];

const PAY: &[Section] = &[
    section(
        "使い方",
        &[text("自分の残高から、指定したユーザーに通貨を送ります。")],
    ),
    section("例", &[code(&["/pay unit:v user:@すみどら amount:100"])]),
    section(
        "結果の表示",
        &[
            text(
                "送金に成功すると、送信者・受取人・金額・単位をこのチャンネルに公開します。\
                 残高不足などのエラーは本人だけに表示します。",
            ),
            text(
                "「送金しませんでした」と表示された場合、残高は変わりません。\
                 結果を確認できない場合やDiscordに応答が届かなかった場合は、\
                 再送せず、`/history` で送金履歴を確認してください。",
            ),
        ],
    ),
    section(
        "エラー",
        &[list(&[
            "`通貨が存在しません` 指定した単位の通貨がありません。",
            "`不正な金額です` 送る枚数が0以下です。1以上を指定してください。",
            "`通貨が不足しています` 自分の残高が足りません。`/bal` で確認できます。",
        ])],
    ),
];

const INFO: &[Section] = &[
    section(
        "使い方",
        &[
            text(
                "通貨の情報を表示します。\
                 引数を付けずにサーバーで実行すると、そのサーバーの通貨を表示します。\
                 DMで実行するときは `name` か `unit` のどちらかが必要です。",
            ),
            text("両方を指定した場合は `name` が優先されます。"),
        ],
    ),
    section(
        "表示される内容",
        &[list(&[
            "**通貨名** 通貨の名前です。",
            "**サーバー名** その通貨を作ったサーバーです。",
            "**単位** 送金や請求で指定する単位です。",
            "**総発行量** これまでに発行した額の合計です。発行枠の計算のもとになります。",
            "**発行枠** 管理者が `/issue` で発行できる、残りの量です。",
            "**あなたの所持量** あなたの残高です。",
            "**削除可能** 作成から72時間以内かどうかです。",
        ])],
    ),
    section(
        "発行枠の増え方",
        &[text(
            "発行枠は1日に1回、総発行量の0.5%増えます。\
             加算は最小5、上限は総発行量の3.5%（最小35）で、端数は丸められます。",
        )],
    ),
];

const CREATE: &[Section] = &[
    section(
        "使い方",
        &[
            text(
                "サーバーに新しい通貨を作ります。実行には管理者権限が必要です。\
                 作った枚数は、そのままあなたの残高になります。",
            ),
            text("1つのサーバーに作れる通貨は1つだけです。"),
        ],
    ),
    section(
        "引数",
        &[list(&[
            "`name` 通貨の名前です。2〜16文字の英数字です。",
            "`unit` 通貨の単位です。1〜10文字の英小文字です。",
            "`amount` 最初に発行する枚数です。1以上4294967295以下です。",
        ])],
    ),
    section(
        "作成後のこと",
        &[
            text(
                "作った枚数の0.5%（最小5）が、最初の発行枠として入ります。\
                 以後は1日1回、総発行量に応じて増えます。",
            ),
            text("作成から72時間以内なら `/delete` で削除できます。"),
            text("`/info` で情報を確認できます。"),
        ],
    ),
    section(
        "エラー",
        &[list(&[
            "`このギルドではすでに通貨が作成されています` 1つのサーバーに作れる通貨は1つです。",
            "`という名前の通貨は存在しています` 名前はサービス全体で一意です。",
            "`という単位の通貨は存在しています` 単位はサービス全体で一意です。",
            "`実行には管理者権限が必要です` サーバーの管理者権限が必要です。",
            "`DMでは実行できません` サーバーの中で実行してください。",
        ])],
    ),
];

const DELETE: &[Section] = &[
    section(
        "使い方",
        &[
            text(
                "サーバーの通貨を削除します。実行には管理者権限が必要で、\
                 作成から72時間以内だけ行えます。",
            ),
            text(
                "実行すると確認のフォームが開きます。\
                 `delete 単位` の形で入力すると、削除されます。",
            ),
        ],
    ),
    section(
        "注意",
        &[text(
            "削除すると、その通貨の残高・請求・契約もすべて消えます。元には戻せません。",
        )],
    ),
    section(
        "エラー",
        &[list(&[
            "`このサーバーに通貨が存在しません` 通貨がまだありません。",
            "`作成から72時間以上経過しているため削除できません` 削除できる期間を過ぎています。",
            "`実行には管理者権限が必要です` 確認フォームの送信時にも管理者権限が必要です。",
        ])],
    ),
];

const BAL: &[Section] = &[section(
    "使い方",
    &[
        text("自分が持っている通貨と枚数の一覧を表示します。この表示は実行者だけに見えます。"),
        text("通貨ごとの詳しい情報は `/info`、他の人への送金は `/pay` です。"),
    ],
)];

const CLAIM: &[Section] = &[
    section(
        "請求とは",
        &[text(
            "`/claim make` で、相手に支払いを求める請求を作ります。\
             相手が `/claim approve` すると、相手からあなたへ通貨が支払われます。",
        )],
    ),
    section(
        "サブコマンド",
        &[
            list(&[
                "`list` 自分に関係する請求の一覧を表示します。",
                "`received` 自分が受け取った請求（自分が支払う側）を表示します。",
                "`sent` 自分が送った請求（自分が受け取る側）を表示します。",
                "`make` 請求を作ります。",
                "`show` 1件の請求を表示します。",
                "`approve` 請求を承諾し、支払います。",
                "`deny` 請求を拒否します。",
                "`cancel` 自分が送った請求を取り消します。",
            ]),
            text("`id` は `/claim list` で確認できます。"),
        ],
    ),
    section(
        "一覧のしぼり込み",
        &[
            text(
                "`pending` `approved` `denied` `canceled` のいずれかを指定すると、\
                 指定した状態の請求だけを表示します。\
                 何も指定しないときは、未決定（`pending`）の請求だけを表示します。",
            ),
            text("`user` を指定すると、そのユーザーが関わる請求だけを表示します。"),
            text(
                "`/mute` で指定した通貨と相手の請求は、一覧に表示されません。\
                 件数にも含まれません。",
            ),
        ],
    ),
    section(
        "状態",
        &[list(&[
            "⌛未決定 相手の返事を待っています。",
            "✅支払い済み 支払われました。",
            "❌拒否 相手が拒否しました。",
            "🗑️キャンセル 請求した側が取り消しました。",
        ])],
    ),
    section(
        "実行できる人",
        &[text(
            "`approve` と `deny` は支払う側（請求先）、`cancel` は請求した側が実行できます。\
             一覧では、未決定の請求の行にその3つのボタンが出ます。\
             ボタンは、その請求を実行できる立場のときだけ押せます。",
        )],
    ),
    section(
        "請求の表示",
        &[text(
            "`/claim show` は1件の請求を表示します。\
             自分が関係していない請求は表示できません。\
             支払う側には、自分の残高と、支払ったあとの残高が表示されます。",
        )],
    ),
];

const MUTE: &[Section] = &[
    section(
        "使い方",
        &[
            text(
                "自分の請求と契約の一覧に表示しないものを指定します。\
                 資金の移動は止まりません。ミュートした通貨でも、請求・支払い・契約はそのまま行えます。",
            ),
            list(&[
                "`/mute currency unit:<通貨の単位>` その通貨の請求と契約を、自分の一覧に表示しなくします。",
                "`/mute user user:<相手>` その人の請求と契約を、自分の一覧に表示しなくします。",
                "`/mute list` ミュートしているものを表示します。1件ずつの解除はこの画面のボタンです。",
            ]),
        ],
    ),
    section(
        "表示されなくなるもの",
        &[text(
            "自分の請求の一覧と契約の一覧だけです。\
             残高は変わりません。相手の画面も変わりません。\
             ミュートは指定した本人の一覧にだけ効きます。",
        )],
    ),
    section(
        "解除",
        &[text(
            "無期限です。解除するまで続きます。\
             `/mute list` の各行の「解除」を押すか、`/unmute` で解除できます。",
        )],
    ),
];

const UNMUTE: &[Section] = &[section(
    "使い方",
    &[
        text("`/mute` で指定したものを解除します。"),
        list(&[
            "`/unmute currency unit:<通貨の単位>` その通貨のミュートを解除します。",
            "`/unmute user user:<相手>` その人のミュートを解除します。",
        ]),
        text("ミュートしているものの一覧は `/mute list` です。"),
    ],
)];

const HISTORY: &[Section] = &[
    section(
        "使い方",
        &[
            text(
                "自分のお金の出入りの履歴と、このサーバーの発行の履歴を表示します。どちらも新しい順です。",
            ),
            list(&[
                "`/history pay` 自分が送った・受け取った・発行で受け取った分の履歴を表示します。",
                "`/history pay unit:<通貨の単位> user:<相手>` 表示する履歴を絞ります。",
                "`/history issue` このサーバーの発行枠から発行された履歴を表示します（管理者）。",
                "`/history issue user:<相手>` その人に発行された履歴だけを表示します。",
            ]),
        ],
    ),
    section(
        "自分の履歴に出るもの",
        &[
            text(
                "自分が関わったお金の出入りです。`/pay` のほかに、請求を承諾したときの支払い、\
                 契約を承認したときのロック（「契約にロック」）、契約が終わって戻ってきた分や\
                 アプリケーションが返した分（「契約から返却」）、そして発行枠から自分に発行された分\
                 （「発行」）が入ります。契約から支払いを受け取った分は「契約から受取」と表示されます。\
                 支払う側はロック時に残高が減っているため、契約からの支払いを再び出金として数えません。",
            ),
            text(
                "相手を指定すると（`user:`）、発行の行は出ません。発行の相手は発行枠で、人ではないからです。",
            ),
        ],
    ),
    section(
        "表示の見方",
        &[
            text(
                "1ページ5件です。種別・金額、相手、取引後残高、日時、取引IDを取引ごとに表示します。\
                 金額の＋は入金、−は出金です。自己送金は残高変動なしと表示します。\
                 日時はDiscordの表示設定に従います。",
            ),
            text(
                "残高はその取引直後の保存値です。過去の記録など保存値がない場合は「未記録」と表示します。\
                 発行履歴では、発行先個人の残高ではなく発行枠の残高を表示します。",
            ),
            text(
                "取引IDのPは送金・契約の記録、Iは発行の記録です。数字が同じでも別の記録です。\
                 契約の記録には契約IDも表示します。請求の承諾による支払いは送金・受取に含まれます。",
            ),
            text(
                "上部で絞り込み条件と件数、下部でページ番号と表示範囲を確認できます。\
                 「最初」「前へ」「次へ」「最後」で移動しても条件は維持されます。\
                 0件の場合は条件を確認し、必要に応じて変更して再実行してください。",
            ),
        ],
    ),
    section(
        "実行できる人",
        &[text(
            "`/history pay` は自分の履歴なので、誰でも実行できます。\
             `/history issue` は発行枠を確認するものなので、`/issue` と同じく管理者権限が必要です。",
        )],
    ),
];
