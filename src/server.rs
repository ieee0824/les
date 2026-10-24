//! SES Query/XML and SES v2 REST/JSON adapters.
use crate::{Destination, Error, Les, SendRequest};
use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};

type Shared = Arc<Mutex<Les>>;
pub fn router(les: Les) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route(
            "/",
            get(|| async { axum::response::Redirect::to("/_les/mail") }).post(query),
        )
        .route("/v2/email/outbound-emails", post(send_v2))
        .route("/v2/email/identities", post(create_identity))
        .route(
            "/v2/email/identities/{identity}",
            get(get_identity).delete(delete_identity),
        )
        .route("/_les/messages", get(messages))
        .route("/_les/mail", get(mailbox))
        .route("/_les/mail/{id}", get(mail_detail))
        .route("/_les/mail/{id}/preview", get(mail_preview))
        .layer(DefaultBodyLimit::max(16 * 1024 * 1024))
        .with_state(Arc::new(Mutex::new(les)))
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn response(status: StatusCode, body: String, is_xml: bool, id: &str) -> Response {
    (
        status,
        [
            (
                "content-type",
                if is_xml {
                    "text/xml; charset=utf-8"
                } else {
                    "application/json"
                },
            ),
            ("x-amzn-requestid", id),
        ],
        body,
    )
        .into_response()
}
fn failure(e: Error, is_xml: bool, id: &str) -> Response {
    let (status, code, message) = match e {
        Error::Invalid(s) => (
            StatusCode::BAD_REQUEST,
            if is_xml {
                "InvalidParameterValue"
            } else {
                "BadRequestException"
            },
            s,
        ),
        Error::Rejected(s) => (StatusCode::BAD_REQUEST, "MessageRejected", s),
        Error::AlreadyExists(s) => (StatusCode::BAD_REQUEST, "AlreadyExistsException", s),
        Error::Storage(e) => {
            eprintln!("storage error: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "InternalFailure",
                "Internal storage error".into(),
            )
        }
    };
    let body = if is_xml {
        format!(
            "<ErrorResponse xmlns=\"http://ses.amazonaws.com/doc/2010-12-01/\"><Error><Type>{}</Type><Code>{code}</Code><Message>{}</Message></Error><RequestId>{id}</RequestId></ErrorResponse>",
            if status.is_server_error() {
                "Receiver"
            } else {
                "Sender"
            },
            xml(&message)
        )
    } else {
        json!({"__type":code,"message":message}).to_string()
    };
    let mut result = response(status, body, is_xml, id);
    if !is_xml {
        result
            .headers_mut()
            .insert("x-amzn-errortype", code.parse().unwrap());
    }
    result
}
fn invalid(s: impl Into<String>) -> Error {
    Error::Invalid(s.into())
}
fn run<T>(state: &Shared, f: impl FnOnce(&Les) -> Result<T, Error>) -> Result<T, Error> {
    let les = state
        .lock()
        .map_err(|_| Error::Storage(rusqlite::Error::InvalidQuery))?;
    f(&les)
}
async fn query(State(state): State<Shared>, body: Bytes) -> Response {
    let id = uuid::Uuid::new_v4().to_string();
    let result = (|| {
        let p: BTreeMap<String, String> =
            serde_urlencoded::from_bytes(&body).map_err(|_| invalid("Invalid Query body"))?;
        let required = |key: &str| {
            p.get(key)
                .cloned()
                .ok_or_else(|| invalid(format!("Missing {key}")))
        };
        let action = required("Action")?;
        let result = run(&state, |les| match action.as_str() {
            "VerifyEmailIdentity" => {
                let name = required("EmailAddress")?;
                if !crate::valid_address(&name) {
                    return Err(invalid("Invalid email address"));
                }
                les.verify_identity(&name)?;
                Ok(String::new())
            }
            "VerifyDomainIdentity" => {
                let name = required("Domain")?;
                if !crate::valid_domain(&name) {
                    return Err(invalid("Invalid domain"));
                }
                les.verify_identity(&name)?;
                Ok("<VerificationToken>local-verification</VerificationToken>".into())
            }
            "DeleteIdentity" => {
                les.delete_identity(&required("Identity")?)?;
                Ok(String::new())
            }
            "ListIdentities" => {
                if p.contains_key("NextToken") || p.contains_key("MaxItems") {
                    return Err(invalid("Pagination is not supported for ListIdentities"));
                }
                let kind = p.get("IdentityType").map(String::as_str);
                if !matches!(kind, None | Some("EmailAddress" | "Domain")) {
                    return Err(invalid("Invalid IdentityType"));
                }
                let names = les
                    .identities()?
                    .into_iter()
                    .filter(|s| match kind {
                        Some("EmailAddress") => s.contains('@'),
                        Some("Domain") => !s.contains('@'),
                        _ => true,
                    })
                    .map(|s| format!("<member>{}</member>", xml(&s)))
                    .collect::<String>();
                Ok(format!("<Identities>{names}</Identities>"))
            }
            "SendEmail" => {
                for key in p.keys() {
                    if !matches!(
                        key.as_str(),
                        "Action"
                            | "Version"
                            | "Source"
                            | "Message.Subject.Data"
                            | "Message.Body.Text.Data"
                            | "Message.Body.Html.Data"
                            | "Message.Subject.Charset"
                            | "Message.Body.Text.Charset"
                            | "Message.Body.Html.Charset"
                    ) && !key.starts_with("Destination.")
                    {
                        return Err(invalid(format!("Unsupported parameter: {key}")));
                    }
                    if key.ends_with(".Charset") && !p[key].eq_ignore_ascii_case("UTF-8") {
                        return Err(invalid("Only UTF-8 is supported"));
                    }
                }
                let addresses = |kind: &str| -> Result<Vec<String>, Error> {
                    let prefix = format!("Destination.{kind}.member.");
                    let mut entries = Vec::new();
                    for (k, v) in &p {
                        if let Some(n) = k.strip_prefix(&prefix) {
                            let n = n
                                .parse::<usize>()
                                .map_err(|_| invalid("Invalid recipient index"))?;
                            if n == 0 {
                                return Err(invalid("Recipient indices start at 1"));
                            }
                            entries.push((n, v.clone()));
                        }
                    }
                    entries.sort_by_key(|(n, _)| *n);
                    Ok(entries.into_iter().map(|(_, v)| v).collect())
                };
                for key in p.keys().filter(|k| k.starts_with("Destination.")) {
                    if !["ToAddresses", "CcAddresses", "BccAddresses"]
                        .iter()
                        .any(|kind| key.starts_with(&format!("Destination.{kind}.member.")))
                    {
                        return Err(invalid("Unsupported destination parameter"));
                    }
                }
                let req = SendRequest {
                    from: required("Source")?,
                    destination: Destination {
                        to_addresses: addresses("ToAddresses")?,
                        cc_addresses: addresses("CcAddresses")?,
                        bcc_addresses: addresses("BccAddresses")?,
                    },
                    subject: required("Message.Subject.Data")?,
                    text: p.get("Message.Body.Text.Data").cloned(),
                    html: p.get("Message.Body.Html.Data").cloned(),
                };
                Ok(format!("<MessageId>{}</MessageId>", les.send(req, now())?))
            }
            _ => Err(invalid(format!("Unsupported action: {action}"))),
        })?;
        Ok(format!(
            "<{action}Response xmlns=\"http://ses.amazonaws.com/doc/2010-12-01/\"><{action}Result>{result}</{action}Result><ResponseMetadata><RequestId>{id}</RequestId></ResponseMetadata></{action}Response>"
        ))
    })();
    match result {
        Ok(body) => response(StatusCode::OK, body, true, &id),
        Err(e) => failure(e, true, &id),
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
struct Data {
    data: String,
    charset: Option<String>,
}
impl Data {
    fn value(self) -> Result<String, Error> {
        if self
            .charset
            .is_some_and(|s| !s.eq_ignore_ascii_case("UTF-8"))
        {
            return Err(invalid("Only UTF-8 is supported"));
        }
        Ok(self.data)
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
struct Body {
    text: Option<Data>,
    html: Option<Data>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
struct Simple {
    subject: Data,
    body: Body,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
struct Content {
    simple: Simple,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
struct Send {
    from_email_address: String,
    destination: Destination,
    content: Content,
}
fn json_result(result: Result<serde_json::Value, Error>) -> Response {
    let id = uuid::Uuid::new_v4().to_string();
    match result {
        Ok(v) => response(StatusCode::OK, v.to_string(), false, &id),
        Err(e) => failure(e, false, &id),
    }
}
async fn send_v2(State(state): State<Shared>, body: Bytes) -> Response {
    json_result((|| {
        let send: Send = serde_json::from_slice(&body).map_err(|e| invalid(e.to_string()))?;
        let simple = send.content.simple;
        let req = SendRequest {
            from: send.from_email_address,
            destination: send.destination,
            subject: simple.subject.value()?,
            text: simple.body.text.map(Data::value).transpose()?,
            html: simple.body.html.map(Data::value).transpose()?,
        };
        run(&state, |les| {
            Ok(json!({"MessageId": les.send(req, now())?}))
        })
    })())
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
struct Identity {
    email_identity: String,
}
async fn create_identity(State(state): State<Shared>, body: Bytes) -> Response {
    json_result((|| {
        let identity: Identity =
            serde_json::from_slice(&body).map_err(|e| invalid(e.to_string()))?;
        run(&state, |les| {
            les.create_identity(&identity.email_identity)?;
            Ok(identity_json(&identity.email_identity))
        })
    })())
}
fn identity_json(name: &str) -> serde_json::Value {
    json!({"IdentityType":if name.contains('@') { "EMAIL_ADDRESS" } else { "DOMAIN" }, "VerifiedForSendingStatus":true,"VerificationStatus":"SUCCESS"})
}
async fn get_identity(State(state): State<Shared>, Path(name): Path<String>) -> Response {
    match run(&state, |les| Ok(les.identities()?.contains(&name))) {
        Ok(true) => json_result(Ok(identity_json(&name))),
        Ok(false) => {
            let id = uuid::Uuid::new_v4().to_string();
            let mut r = response(
                StatusCode::NOT_FOUND,
                json!({"message":"Identity not found","__type":"NotFoundException"}).to_string(),
                false,
                &id,
            );
            r.headers_mut()
                .insert("x-amzn-errortype", "NotFoundException".parse().unwrap());
            r
        }
        Err(e) => json_result(Err(e)),
    }
}
async fn delete_identity(State(state): State<Shared>, Path(name): Path<String>) -> Response {
    json_result(run(&state, |les| {
        les.delete_identity(&name)?;
        Ok(json!({}))
    }))
}
#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    offset: u32,
    limit: Option<u32>,
}
async fn messages(State(state): State<Shared>, Query(page): Query<Page>) -> Response {
    json_result(run(&state, |les| {
        Ok(json!({"messages":les.messages(page.offset, page.limit.unwrap_or(100))?}))
    }))
}

#[derive(Deserialize)]
struct MailboxPage {
    folder: Option<String>,
    #[serde(default)]
    address: String,
    #[serde(default)]
    offset: u32,
}
fn html_response(status: StatusCode, html: String, preview: bool) -> Response {
    let policy = if preview {
        "sandbox; default-src 'none'; style-src 'unsafe-inline'; img-src data:; base-uri 'none'; form-action 'none'; frame-ancestors 'self'"
    } else {
        "default-src 'none'; style-src 'unsafe-inline'; frame-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'"
    };
    (
        status,
        [
            ("content-type", "text/html; charset=utf-8"),
            ("content-security-policy", policy),
            ("x-content-type-options", "nosniff"),
            ("cache-control", "no-store"),
            ("referrer-policy", "no-referrer"),
        ],
        html,
    )
        .into_response()
}
async fn mailbox(State(state): State<Shared>, Query(page): Query<MailboxPage>) -> Response {
    let received = match page.folder.as_deref() {
        None | Some("inbox") => true,
        Some("sent") => false,
        _ => return json_result(Err(invalid("folder must be inbox or sent"))),
    };
    match run(&state, |les| {
        les.mailbox(received, &page.address, page.offset, 21)
    }) {
        Ok(messages) => html_response(
            StatusCode::OK,
            crate::ui::mailbox(received, &page.address, page.offset, &messages),
            false,
        ),
        Err(e) => json_result(Err(e)),
    }
}
async fn mail_detail(State(state): State<Shared>, Path(id): Path<String>) -> Response {
    match run(&state, |les| les.message(&id)) {
        Ok(Some(m)) => html_response(StatusCode::OK, crate::ui::detail(&m), false),
        Ok(None) => html_response(StatusCode::NOT_FOUND, crate::ui::not_found(), false),
        Err(e) => json_result(Err(e)),
    }
}
async fn mail_preview(State(state): State<Shared>, Path(id): Path<String>) -> Response {
    match run(&state, |les| les.message(&id)) {
        Ok(Some(m)) => html_response(
            StatusCode::OK,
            crate::ui::preview(m.request.html.as_deref().unwrap_or_default()),
            true,
        ),
        Ok(None) => html_response(StatusCode::NOT_FOUND, crate::ui::not_found(), true),
        Err(e) => json_result(Err(e)),
    }
}
