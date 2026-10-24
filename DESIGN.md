# LES 設計

## lqs から採用する方針

[lqs の設計](https://github.com/ieee0824/lqs/blob/main/DESIGN.md)を参考に、同期コアを HTTP から独立させ、SQLite を状態の唯一の保存先にします。インメモリ DB とファイル DB は同じスキーマを使います。コアへの時刻は呼び出し元がミリ秒で渡し、HTTP 層だけが Unix 時刻を読みます。

`src/lib.rs` は入力検証・送信元の管理・メール保存、`src/server.rs` は Query/XML と REST/JSON の変換、`src/main.rs` は環境変数・DB 接続・サーバー起動を担当します。HTTP からも Rust からも同じ送信検証を通ります。

## 保存と受付

`identities(name)` に即時検証済みのメールアドレス/ドメイン、`messages(sequence, id, created_at_ms, payload)` に宛先・件名・本文を保存します。ID は UUID、閲覧順は SQLite の挿入順です。送信は全検証後の単一 INSERT で確定し、失敗したメールは保存しません。HTTP では Mutex により1接続を直列使用します。WAL と5秒の busy timeout を設定します。高負荷向けの接続プールや非同期 DB ワーカーは現段階では導入しません。

検証済み判定と INSERT は別 SQL です。別プロセスが同じ DB を操作する場合、判定直後の identity 削除と競合する可能性があります。送信の重複排除は行わず、再試行は別メールとして保存します。

状態は「検証 → 受付 → 保存」のみです。保存成功は実配送や受信成功を意味しません。保存一覧は LES 独自の管理 API とし、AWS API とパスを分離します。閲覧画面はサーバー側で HTML を生成し、件名・宛先・本文などはエスケープします。HTML プレビューは ammonia でリンク・スクリプト・フォームなどを除去し、別 URL と操作可能な sandbox iframe に隔離します。元の HTML は保存したままソース表示に使用します。CSP によりスクリプト・外部リソース・フォーム送信を禁止します。閲覧ページには no-store を指定します。

## 互換性の境界

[SES v1 SendEmail](https://docs.aws.amazon.com/ses/latest/APIReference/API_SendEmail.html)と[SES v2 SendEmail](https://docs.aws.amazon.com/ses/latest/APIReference-V2/API_SendEmail.html)を基に、通常メール送信の最小サブセットを実装します。v1 は XML 名前空間と ResponseMetadata、v2 は JSON とリクエスト ID ヘッダーを返します。未検証送信元は MessageRejected とし、永続化エラーの内部情報は HTTP に出しません。

AWS の全仕様を模倣せず、初版の制限を README に明記します。送信に影響する未知のフィールドを黙って無視しないことで、ローカルでは成功したが本番と意味が違うケースを減らします。

## 検証と今後の拡張

テストでは未検証送信元・不正宛先・宛先件数・本文欠落で保存されないこと、ドメイン検証、DB 再オープン、ページング、v1/v2 の共通状態と応答形式を確認します。

拡張は Raw MIME の解析/保存、テンプレート、決定的な配信イベント、クォータ制御の順で別機能として追加できます。bounce / complaint を導入する際は宛先単位のイベントをメールと同じトランザクションで保存し、時刻を注入して sleep なしでテストできる構成にします。SMTP や SNS 連携は独立したアダプターとして扱います。スキーマ変更時には明示的なマイグレーションを追加します。

## メール閲覧

`GET /` は `/_les/mail` にリダイレクトし、既存の `POST /` SES Query API は維持します。受信（inbox）と送信（sent）は同じ保存メールをそれぞれ宛先・送信元で検索するビューです。SMTP 受信や配送状態を追加するものではありません。

`Les::mailbox` は JSON 保存済みの送信元または To/Cc/Bcc に SQL で完全一致フィルターを適用し、挿入順の降順で取得します。宛先の重複による行の重複は EXISTS で防ぎます。20件と次ページ判定用の1件を取得し、本文は ID による詳細画面でも確認できます。スキーマ変更は不要です。offset ページングのため、新規メールの挿入があるとページ境界が移動する場合があります。

SES v2 の送信元作成は `Les::create_identity` を使用し、単一 INSERT の競合結果から重複を判定して `AlreadyExistsException` に変換します。v1 と Rust の `verify_identity` は重複を許容します。
