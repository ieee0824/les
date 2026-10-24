# LES

ローカル開発・テスト用の Amazon SES シミュレーターです。
[lqs](https://github.com/ieee0824/lqs) と同様、Rust ライブラリ・SQLite 永続化・AWS 互換 HTTP アダプターで構成します。メールは SQLite に保存し、外部には配送しません。

## 起動

Rust（edition 2024 対応）が必要です。

```sh
cargo run --locked
```

| 環境変数 | 既定値 | 用途 |
| --- | --- | --- |
| `LES_BIND_ADDR` | `127.0.0.1:9325` | HTTP 待受アドレス |
| `LES_DATABASE_PATH` | `les.sqlite` | SQLite ファイル |

```sh
curl --fail http://127.0.0.1:9325/health

# 検証は即時成功。検証メールや DNS 設定は不要です。
curl --fail http://127.0.0.1:9325/ \
  --data-urlencode 'Action=VerifyEmailIdentity' \
  --data-urlencode 'EmailAddress=sender@example.com'

curl --fail http://127.0.0.1:9325/ \
  --data-urlencode 'Action=SendEmail' \
  --data-urlencode 'Source=sender@example.com' \
  --data-urlencode 'Destination.ToAddresses.member.1=recipient@example.net' \
  --data-urlencode 'Message.Subject.Data=テストメール' \
  --data-urlencode 'Message.Body.Text.Data=Hello from LES'

# LES 独自の確認 API。limit は最大100、offset は0始まり。
curl --fail 'http://127.0.0.1:9325/_les/messages?offset=0&limit=100'
```

## メールを見る

起動後、ブラウザで **http://127.0.0.1:9325/** を開きます。

- **受信メール**: LES が受け付けたメールを宛先側から表示します。To・Cc・Bcc のメールアドレスで絞り込めます。
- **送信メール**: 送信元のメールアドレスで絞り込めます。
- 件名をクリックすると、From / To / Cc / Bcc、受付日時（UTC）、MessageId、テキスト本文、HTML プレビューとソースを確認できます。
- 新しいメールから20件ずつ表示します。「絞り込む / 更新」で最新の状態を取得します。アドレスは完全一致です。

受信一覧は保存済みメールの宛先側のビューです。外部メールサーバーからの SMTP 受信は行いません。送信済みメールを複製せず、既存の SQLite データをそのまま表示します。HTML プレビューはリンク・スクリプト・フォームを除去したうえで sandbox と CSP で表示し、マウスとキーボードでスクロールできます。一部の装飾は除去されるため、元の HTML はソース表示でも確認できます。管理画面は Bcc を含むデータを表示するローカル開発用です。

## AWS CLI / SDK

エンドポイントを `http://127.0.0.1:9325` に変更し、リージョンとダミー認証情報を設定します。SigV4 は検証しません。

```sh
export AWS_ACCESS_KEY_ID=test
export AWS_SECRET_ACCESS_KEY=test
export AWS_DEFAULT_REGION=us-east-1

aws --endpoint-url http://127.0.0.1:9325 ses verify-email-identity \
  --email-address sender@example.com

aws --endpoint-url http://127.0.0.1:9325 sesv2 send-email \
  --from-email-address sender@example.com \
  --destination '{"ToAddresses":["recipient@example.net"]}' \
  --content '{"Simple":{"Subject":{"Data":"Hello"},"Body":{"Text":{"Data":"Local SES"}}}}'
```

## 対応範囲

| プロトコル | API |
| --- | --- |
| SES v1 Query / XML（POST `/`） | `VerifyEmailIdentity`, `VerifyDomainIdentity`, `ListIdentities`, `DeleteIdentity`, `SendEmail` |
| SES v2 REST / JSON | `CreateEmailIdentity`, `GetEmailIdentity`, `DeleteEmailIdentity`, `SendEmail`（Simple） |
| LES 独自 | `GET /health`, `GET /_les/messages`, 閲覧画面 `GET /_les/mail` |

`CreateEmailIdentity` は既存の送信元を登録すると HTTP 400 / `AlreadyExistsException` を返します。v1 の検証登録は繰り返し実行できます。

送信元は登録済みアドレスまたは登録済みドメインとの完全一致が必要です。宛先は未検証でも使用できます。To / Cc / Bcc 合計1〜50件、ASCII の裸のメールアドレス、Text または Html 本文を受け付けます。送信成功時の `MessageId` で保存内容を照合できます。再起動しても送信元とメールは残ります。

## Rust ライブラリ

```rust
use les::{Destination, Les, SendRequest};

let les = Les::in_memory()?; // 永続化: Les::open("les.sqlite")?
les.verify_identity("example.com")?;
let id = les.send(SendRequest {
    from: "sender@example.com".into(),
    destination: Destination {
        to_addresses: vec!["recipient@example.net".into()],
        ..Default::default()
    },
    subject: "hello".into(),
    text: Some("body".into()),
    html: None,
}, 1_000)?;
assert_eq!(les.messages(0, 100)?[0].id, id);
# Ok::<(), les::Error>(())
```

## 制約

初版は通常メールの受付・検証・保存を対象とします。Raw MIME、添付、テンプレート、SMTP、Reply-To、タグ、設定セット、送信クォータ、sandbox の宛先検証、bounce / complaint、SNS 通知は未対応です。未対応の送信フィールドはエラーになります。`ListIdentities` のページングは未対応です。

UTF-8 のみ対応し、表示名付きアドレス・国際化アドレスは受け付けません。サイズ上限は全 API 共通で JSON 保存形式の10 MiB、HTTP 入力は16 MiBです。AWS の MIME サイズ計算・v2 のサイズ上限とは異なります。HTTP のルーティングやボディサイズ超過など、フレームワーク由来のエラーは AWS 形式ではありません。

ローカルの信頼できる環境専用です。認証・TLS・暗号化・アカウント/リージョン分離はありません。確認 API は Bcc を含む全保存データを返します。SQLite は平文保存で、自動削除は行いません。

## 開発

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
# 別ターミナルで LES を起動した状態で実行（AWS CLI が必要）
bash integration/aws-cli.sh
```

設計・拡張方針は [DESIGN.md](DESIGN.md) を参照してください。
