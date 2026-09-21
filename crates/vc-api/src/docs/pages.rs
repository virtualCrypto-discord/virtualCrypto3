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
    title: "はじめに",
    summary: "VirtualCryptoとは何か、通貨の仕組み、はじめかた。",
    listing: Listing::None,
    sections: &[
        lead(&[text(
            "VirtualCryptoは、Discordのサーバーで独自の通貨を使えるようにするBotです。\
                 ここでいう通貨は暗号通貨ではなく、そのサーバーの中だけで通用する点数です。",
        )]),
        section(
            "できること",
            &[list(&[
                "`/create` サーバーに通貨を作る",
                "`/pay` 通貨を送る、`/claim` 支払いを請求する",
                "`/bal` 自分の残高を見る、`/info` 通貨の情報を見る",
                "`/issue` 発行枠から配る（管理者）",
                "アプリケーションと連携すると、Botや外部サービスが通貨を動かせる（[アプリケーション連携]({site}/document/applications)）",
            ])],
        ),
        section(
            "はじめかた",
            &[
                list(&[
                    "[Botを招待する]({invite})。招待には「アプリケーションのコマンド」の権限が必要です。",
                    "サーバーの管理者が `/create` で通貨を作る。",
                    "`/pay` で配る。`/bal` で自分の残高を確認できます。",
                ]),
                text("コマンドの一覧と使い方は[コマンド]({site}/document/commands)にあります。"),
            ],
        ),
        section(
            "通貨の仕組み",
            &[
                text(
                    "作った枚数は、そのまま作った人の残高になります。\
                     ここから配る分として、最初の発行枠が用意されます。",
                ),
                text(
                    "発行枠は1日に1回、総発行量の0.5%増えます。\
                     加算は最小5、上限は総発行量の3.5%（最小35）で、端数は丸められます。",
                ),
                text(
                    "総発行量とは、これまでに発行した額の合計です。発行した通貨は消えないので、\
                     今みんなの口座にある残高の合計と一致します。\
                     契約でロックされている通貨は契約の口座にあるので含まれ、\
                     まだ発行していない発行枠は含みません。\
                     `/info` で総発行量と今の発行枠を確認できます。",
                ),
                text(
                    "管理者は `/issue` で発行枠から発行できます。\
                     発行できるのはサーバーごとに1つ、通貨の名前と単位はサービス全体で一意です。",
                ),
                text("作成から72時間以内なら、`/delete` で通貨を削除できます。"),
            ],
        ),
        section(
            "困ったとき",
            &[
                text("エラーの意味は[よくある質問]({site}/document/faq)にまとめています。"),
                text("それでも分からないときは[サポートサーバー]({support})へどうぞ。"),
            ],
        ),
    ],
};

/// コマンド: how to read the list, then the list itself.
const COMMANDS: Page = Page {
    slug: "commands",
    title: "コマンド",
    summary: "使えるコマンドの一覧と、それぞれの使い方。",
    listing: Listing::Commands,
    sections: &[
        section(
            "読み方",
            &[
                text(
                    "「使い方」は入力の形です。`< >` の中は、その値に置き換えてください。\
                     たとえば `/pay unit:<通貨の単位> user:<送信先> amount:<枚数>` は、\
                     `/pay unit:v user:@すみどら amount:100` のように入力します。",
                ),
                text(
                    "引数には「必須」と「任意」があります。必須の引数を省略するとエラーになります。\
                     任意の引数を省略したときの動作は、各コマンドの説明をご覧ください。",
                ),
            ],
        ),
        section(
            "管理者のコマンド",
            &[text(
                "`/create` `/delete` `/issue` `/grant` は、実行する人にサーバーの管理者権限が必要です。\
                 それ以外のコマンドは、サーバーのメンバーなら誰でも使えます。",
            )],
        ),
        section(
            "Discordの中で読む",
            &[text(
                "同じ内容はDiscordの `/help` でも読めます。\
                 `/help command:<コマンド名>` で、1つのコマンドを直接開けます。",
            )],
        ),
    ],
};

/// アプリケーション連携: what an application is, and the four steps to using one.
const APPLICATIONS: Page = Page {
    slug: "applications",
    title: "アプリケーション連携",
    summary: "Botや外部サービスからVirtualCryptoの通貨を扱うための手順。",
    listing: Listing::None,
    sections: &[
        lead(&[text(
            "アプリケーションは、Botや外部サービスがVirtualCryptoの通貨を扱うための登録です。\
             登録すると `client_id` と `client_secret` が発行され、APIを呼べるようになります。",
        )]),
        section(
            "1. 登録する",
            &[
                text(
                    "`/application register` で登録します。\
                     登録した時点では、種類 `web`・グラントタイプ `authorization_code`・\
                     レスポンスタイプ `code` で作られます。\
                     細かい設定は `/application show` の画面で変えます。",
                ),
                list(&[
                    "**クライアント名** 一覧や契約に表示される名前です。",
                    "**リダイレクト URI** 認可コードを受け取るURLです。1行に1つ書きます。",
                    "**webhook URL** 通知を受け取るURLです。",
                    "**グラントタイプ** 認可コードの交換と、リフレッシュトークンでの更新のどちらを許すかです。",
                    "**レスポンスタイプ** 認可コードを返すかどうかです。",
                    "**通知イベント** 受け取る通知を選びます。",
                    "**ロゴ URI** 同意画面などに表示される画像です。",
                    "**サポートサーバーの招待 slug** 利用者からの質問を受け付ける場所です。",
                ]),
                text("`client_secret` は `show` の画面に表示されます。他人に見せないでください。"),
            ],
        ),
        section(
            "2. Botを接続する",
            &[
                text(
                    "アプリケーションがDiscordのBotなら、`/application show` の画面でBotを選ぶと接続できます。\
                     接続はサーバーの中で行います。",
                ),
                text(
                    "接続できるのは、Discord上のそのBotのプロフィール（説明）に、\
                     `show` の画面に出るトークン（`{site}/applications/verification?q=<client_id>`）が\
                     書かれている場合だけです。\
                     これは、他人のBotにあなたのアプリケーションを騙られないための確認です。",
                ),
            ],
        ),
        section(
            "3. 発行を許可する",
            &[
                text(
                    "サーバーの発行枠から通貨を発行したいアプリケーションは、そのサーバーに申請します。\
                     申請はアプリケーションのAPIから行われ、サーバーに申請コードが届きます。",
                ),
                text(
                    "サーバーの管理者は、アプリケーションが表示する申請コードを\
                     `/grant approve code:<申請コード>` に入れて承認します。",
                ),
                text(
                    "`/grant list` は、そのサーバーで発行を許可しているアプリケーションの\
                     一覧です。各アプリケーションの「取り消す」で許可を外せます。",
                ),
                text("承認できるのは、申請されたスコープだけです。拒否という決定はありません。"),
            ],
        ),
        section(
            "4. 契約で通貨をロックする",
            &[
                text(
                    "アプリケーションが利用者の通貨を操作するには、契約を作ります。\
                     契約は、対象になる利用者と、それぞれがロックする量を決めます。",
                ),
                text(
                    "利用者は `/contract list` で内容を確認し、承認します。\
                     承認した分の通貨は、利用者の残高から契約に移ります。\
                     アプリケーションは、ロックされた範囲でだけ支払えます。",
                ),
                text(
                    "使われなかった分は、契約が終わると利用者に戻ります。\
                     ロックは移動であって、通貨が消えることはありません。",
                ),
            ],
        ),
        section(
            "スコープ",
            &[
                text("アプリケーションができることは、トークンに付いたスコープで決まります。"),
                list(&[
                    "`vc.pay` 通貨を送る。",
                    "`vc.claim` 請求を作る・承諾する。",
                    "`vc.issue` サーバーの発行枠から発行する（サーバーの許可が必要）。",
                    "`vc.contract` 契約でロックされた通貨を操作する。",
                ]),
                text(
                    "利用者に同意を求めるときは、同意画面にそのアプリケーションが求めるスコープが表示されます。\
                     同意した内容は、そのアプリケーションのトークンにだけ入ります。",
                ),
            ],
        ),
        section(
            "通知",
            &[
                text(
                    "アプリケーションは、契約の決定や発行の許可をWebhookで受け取れます。\
                     受け取るイベントは `show` の画面で選べます。",
                ),
                text("本文の形と署名の検証方法は、[API]({site}/document/api)をご覧ください。"),
            ],
        ),
    ],
};

/// よくある質問: the sentences people arrive with, answered where they are asked.
const FAQ: Page = Page {
    slug: "faq",
    title: "よくある質問",
    summary: "エラーの意味と、よくあるつまずき。",
    listing: Listing::None,
    sections: &[
        section(
            "通貨まわり",
            &[list(&[
                "**`/create` で「すでに作成されています」と言われる** 1つのサーバーに作れる通貨は1つです。既にある通貨を `/info` で確認してください。",
                "**名前や単位が使えないと言われる** 名前と単位は、ほかのサーバーも含めてサービス全体で一意です。別のものを指定してください。",
                "**「DMでは実行できません」と言われる** 通貨はサーバーのものなので、`/create` `/delete` `/issue` `/grant` はサーバーの中で実行します。",
                "**「実行には管理者権限が必要です」と言われる** サーバーの管理者に依頼してください。",
            ])],
        ),
        section(
            "残高と発行枠",
            &[list(&[
                "**発行枠が増えない** 発行枠は1日に1回、総発行量の0.5%（最小5）増えます。だれも持っていない通貨は増えません。",
                "**発行枠が思ったより少ない** 上限は総発行量の3.5%です。`/info` で今の発行枠を確認できます。",
                "**総発行量には何が入るか** これまでに発行した額の合計です。契約でロックされた分は契約の口座にあるので含まれ、まだ発行していない発行枠は含みません。",
                "**残高が足りないと言われる** `/bal` で残高を確認してください。",
            ])],
        ),
        section(
            "請求",
            &[list(&[
                "**請求の一覧に何も出ない** 何も指定しない一覧は、未決定の請求だけを表示します。`approved` などを指定すると、その状態だけを表示します。",
                "**請求を承諾できない** 承諾できるのは請求先（支払う側）だけです。残高が足りない場合も承諾できません。",
                "**請求を取り消したい** 自分が送った請求は `/claim cancel` で取り消せます。",
                "**請求が支払われない** 請求は相手が承諾するまで動きません。承諾されないまま残ることもあります。",
            ])],
        ),
        section(
            "契約とアプリケーション",
            &[list(&[
                "**契約が届かない** 契約に名前を挙げられている人が `/contract list` で確認します。一覧に無い契約は、あなたが対象ではありません。",
                "**承認したお金が戻らない** 期限のない契約は「取り消す」でいつでも戻せます。期限のある契約は、その期間が終わると戻せます。",
                "**Botを接続できない** そのBotのプロフィール（説明）に、`/application show` の画面に出るトークンが書かれている必要があります。",
                "**アプリケーションの登録をやめたい** 通貨を扱う許可は、サーバーの管理者が `/grant list` の「取り消す」で外せます。管理者に依頼してください。",
            ])],
        ),
        section(
            "困ったとき",
            &[
                text("解決しないときは[サポートサーバー]({support})で聞いてください。"),
                text("障害の状況は公式サイト({site})で告知することがあります。"),
            ],
        ),
    ],
};
