use crate::Message;

pub(crate) fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn page(title: &str, content: &str) -> String {
    format!(
        r#"<!doctype html><html lang="ja"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>{} · LES</title>
<style>
:root{{font-family:system-ui,sans-serif;color:#202c3d;background:#f4f6fa}}*{{box-sizing:border-box}}body{{margin:0}}header{{padding:22px 5%;background:#14283e;color:white;display:flex;align-items:center;gap:24px}}header strong{{font-size:24px}}header span{{color:#becfe0;font-size:14px}}main{{max-width:1120px;margin:32px auto;padding:0 24px}}a{{color:#165f9c;text-decoration:none}}a:hover{{text-decoration:underline}}nav{{display:flex;gap:10px;margin-bottom:24px}}nav a,button,.pager a{{border:1px solid #ccd6e0;border-radius:7px;background:white;padding:10px 18px}}nav a[aria-current=page]{{background:#165f9c;color:white;border-color:#165f9c}}h1{{font-size:26px}}.muted{{color:#617187;font-size:14px}}form{{display:flex;gap:12px;align-items:end;margin:24px 0;flex-wrap:wrap}}label{{display:grid;gap:8px;font-size:14px}}input{{padding:10px;border:1px solid #b9c6d3;border-radius:6px;width:min(380px,80vw);font:inherit}}button{{cursor:pointer;font:inherit;color:#165f9c}}.card{{background:white;border:1px solid #dde4ec;border-radius:10px;padding:24px;margin:16px 0;overflow-wrap:anywhere}}table{{border-collapse:collapse;width:100%;text-align:left}}th,td{{padding:16px 12px;border-bottom:1px solid #e5eaf0;vertical-align:top}}th{{font-size:13px;color:#617187}}td{{font-size:14px;overflow-wrap:anywhere}}.table{{overflow-x:auto;padding:4px 12px}}.subject{{font-weight:600;min-width:160px}}.pager{{display:flex;gap:16px;align-items:center;margin:24px 0}}dl{{display:grid;grid-template-columns:90px 1fr;gap:14px;font-size:14px}}dt{{color:#617187}}dd{{margin:0;white-space:pre-wrap}}pre{{white-space:pre-wrap;overflow-wrap:anywhere;font:14px/1.7 ui-monospace,monospace}}iframe{{width:100%;height:480px;border:1px solid #dde4ec;background:white}}summary{{cursor:pointer;color:#165f9c}}@media(max-width:600px){{main{{padding:0 12px}}.card{{padding:16px}}header span{{display:none}}dl{{grid-template-columns:60px 1fr}}}}
</style><header><strong>LES</strong><span>Local Email Simulator</span></header><main>{content}</main></html>"#,
        escape(title)
    )
}
fn link(received: bool, address: &str, offset: u32) -> String {
    let query = serde_urlencoded::to_string([
        ("folder", if received { "inbox" } else { "sent" }),
        ("address", address),
        ("offset", &offset.to_string()),
    ])
    .unwrap();
    format!("/_les/mail?{}", escape(&query))
}
pub(crate) fn mailbox(received: bool, address: &str, offset: u32, messages: &[Message]) -> String {
    let title = if received {
        "受信メール"
    } else {
        "送信メール"
    };
    let mut content = format!(
        r#"<nav><a href="/_les/mail?folder=inbox" {}>受信メール</a><a href="/_les/mail?folder=sent" {}>送信メール</a></nav><h1>{title}</h1><p class="muted">LES が受け付けたメールを{}から確認できます。外部への配送・外部からの SMTP 受信は行いません。</p><form method="get" action="/_les/mail"><input type="hidden" name="folder" value="{}"><label>{}のメールアドレス（完全一致）<input name="address" type="email" placeholder="すべてのアドレス" value="{}"></label><button type="submit">絞り込む / 更新</button><a href="{}">条件をクリア</a></form>"#,
        if received { "aria-current=page" } else { "" },
        if received { "" } else { "aria-current=page" },
        if received {
            "宛先側（To・Cc・Bcc）"
        } else {
            "送信元側"
        },
        if received { "inbox" } else { "sent" },
        if received { "宛先" } else { "送信元" },
        escape(address),
        link(received, "", 0)
    );
    if messages.is_empty() {
        content.push_str("<div class=card>該当するメールはありません。</div>");
    } else {
        content.push_str("<div class=\"card table\"><table><thead><tr><th>件名</th><th>送信元</th><th>宛先（To / Cc / Bcc）</th><th>受付日時（UTC）</th></tr></thead><tbody>");
        for m in messages.iter().take(20) {
            let r = &m.request;
            content.push_str(&format!("<tr><td class=subject><a href=\"/_les/mail/{}\">{}</a></td><td>{}</td><td>{}</td><td>{}</td></tr>", escape(&m.id), escape(if r.subject.is_empty() { "（件名なし）" } else { &r.subject }), escape(&r.from), escape(&r.destination.to_addresses.iter().chain(&r.destination.cc_addresses).chain(&r.destination.bcc_addresses).cloned().collect::<Vec<_>>().join(", ")), timestamp(m.created_at_ms)));
        }
        content.push_str("</tbody></table></div>");
    }
    content.push_str("<div class=pager>");
    if offset > 0 {
        content.push_str(&format!(
            "<a href=\"{}\">← 前へ</a>",
            link(received, address, offset.saturating_sub(20))
        ));
    }
    content.push_str(&format!(
        "<span class=muted>{}ページ</span>",
        offset / 20 + 1
    ));
    if messages.len() > 20 {
        content.push_str(&format!(
            "<a href=\"{}\">次へ →</a>",
            link(received, address, offset.saturating_add(20))
        ));
    }
    content.push_str("</div>");
    page(title, &content)
}
// Gregorian civil date conversion; timestamps are displayed in UTC without host locale dependencies.
fn timestamp(ms: i64) -> String {
    let seconds = ms.div_euclid(1000);
    let days = seconds.div_euclid(86400) + 719468;
    let era = days.div_euclid(146097);
    let doe = days - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = year + i64::from(month <= 2);
    let time = seconds.rem_euclid(86400);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        time / 3600,
        time / 60 % 60,
        time % 60
    )
}
pub(crate) fn detail(m: &Message) -> String {
    let r = &m.request;
    let mut content = String::from(
        "<nav><a href=\"/_les/mail?folder=inbox\">受信メール</a><a href=\"/_les/mail?folder=sent\">送信メール</a></nav>",
    );
    content.push_str(&format!(
        "<h1>{}</h1><div class=card><dl>",
        escape(if r.subject.is_empty() {
            "（件名なし）"
        } else {
            &r.subject
        })
    ));
    for (label, value) in [
        ("From", r.from.clone()),
        ("To", r.destination.to_addresses.join(", ")),
        ("Cc", r.destination.cc_addresses.join(", ")),
        ("Bcc", r.destination.bcc_addresses.join(", ")),
        ("受付 UTC", timestamp(m.created_at_ms)),
        ("ID", m.id.clone()),
    ] {
        content.push_str(&format!("<dt>{label}</dt><dd>{}</dd>", escape(&value)));
    }
    content.push_str("</dl></div><section class=card><h2>テキスト本文</h2>");
    content.push_str(&format!(
        "<pre>{}</pre></section>",
        escape(r.text.as_deref().unwrap_or("テキスト本文はありません。"))
    ));
    if let Some(html) = &r.html {
        content.push_str(&format!("<section class=card><h2>HTML プレビュー</h2><p class=muted>スクロールして本文全体を確認できます。リンクやスクリプト、一部の装飾は除去して表示します。</p><iframe title=\"HTML メール本文\" sandbox tabindex=\"0\" referrerpolicy=\"no-referrer\" src=\"/_les/mail/{}/preview\"></iframe><details><summary>HTML ソースを表示</summary><pre>{}</pre></details></section>", escape(&m.id), escape(html)));
    }
    page(&r.subject, &content)
}
pub(crate) fn not_found() -> String {
    page(
        "メールが見つかりません",
        "<h1>メールが見つかりません</h1><a href=\"/_les/mail\">一覧へ戻る</a>",
    )
}

/// Sanitize navigation targets rather than disabling interaction with the whole frame.
/// The original HTML remains available in the source view.
pub(crate) fn preview(html: &str) -> String {
    let body = ammonia::Builder::default()
        .rm_tags(&["a", "area"])
        .clean(html)
        .to_string();
    format!("<!doctype html><html lang=\"ja\"><meta charset=\"utf-8\"><body>{body}</body></html>")
}
