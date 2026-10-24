//! Local SES core. No network delivery; callers supply Unix milliseconds.
pub mod server;
mod ui;

use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub enum Error {
    Invalid(String),
    Rejected(String),
    AlreadyExists(String),
    Storage(rusqlite::Error),
}
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::Storage(e)
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(s) | Self::Rejected(s) | Self::AlreadyExists(s) => f.write_str(s),
            Self::Storage(e) => write!(f, "{e}"),
        }
    }
}
impl std::error::Error for Error {}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
pub struct Destination {
    #[serde(default)]
    pub to_addresses: Vec<String>,
    #[serde(default)]
    pub cc_addresses: Vec<String>,
    #[serde(default)]
    pub bcc_addresses: Vec<String>,
}
impl Destination {
    fn addresses(&self) -> impl Iterator<Item = &String> {
        self.to_addresses
            .iter()
            .chain(&self.cc_addresses)
            .chain(&self.bcc_addresses)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendRequest {
    pub from: String,
    pub destination: Destination,
    pub subject: String,
    pub text: Option<String>,
    pub html: Option<String>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub created_at_ms: i64,
    pub request: SendRequest,
}

pub struct Les {
    conn: Connection,
}
impl Les {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self, Error> {
        Self::init(Connection::open(path)?)
    }
    pub fn in_memory() -> Result<Self, Error> {
        Self::init(Connection::open_in_memory()?)
    }
    fn init(conn: Connection) -> Result<Self, Error> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA journal_mode=WAL;
            CREATE TABLE IF NOT EXISTS identities (name TEXT PRIMARY KEY);
            CREATE TABLE IF NOT EXISTS messages (sequence INTEGER PRIMARY KEY, id TEXT UNIQUE NOT NULL, created_at_ms INTEGER NOT NULL, payload TEXT NOT NULL);")?;
        Ok(Self { conn })
    }
    /// Verification succeeds immediately; no verification mail or DNS lookup is performed.
    pub fn verify_identity(&self, name: &str) -> Result<(), Error> {
        match self.create_identity(name) {
            Err(Error::AlreadyExists(_)) => Ok(()),
            result => result,
        }
    }
    /// Creates an immediately verified identity, rejecting duplicates atomically.
    pub fn create_identity(&self, name: &str) -> Result<(), Error> {
        if !(valid_address(name) || valid_domain(name)) {
            return Err(Error::Invalid("Invalid email address or domain".into()));
        }
        let inserted = self.conn.execute(
            "INSERT INTO identities(name) VALUES (?1) ON CONFLICT(name) DO NOTHING",
            [name],
        )?;
        if inserted == 0 {
            return Err(Error::AlreadyExists("Email identity already exists".into()));
        }
        Ok(())
    }
    pub fn delete_identity(&self, name: &str) -> Result<(), Error> {
        self.conn
            .execute("DELETE FROM identities WHERE name=?1", [name])?;
        Ok(())
    }
    pub fn identities(&self) -> Result<Vec<String>, Error> {
        Ok(self
            .conn
            .prepare("SELECT name FROM identities ORDER BY name")?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?)
    }
    pub fn send(&self, req: SendRequest, now_ms: i64) -> Result<String, Error> {
        if !valid_address(&req.from) || req.destination.addresses().any(|s| !valid_address(s)) {
            return Err(Error::Invalid(
                "Invalid email address (bare ASCII addresses required)".into(),
            ));
        }
        if !(1..=50).contains(&req.destination.addresses().count()) {
            return Err(Error::Invalid("Expected 1 to 50 recipients".into()));
        }
        if req.text.is_none() && req.html.is_none() {
            return Err(Error::Invalid("Text or Html body required".into()));
        }
        if req.subject.contains(['\r', '\n']) {
            return Err(Error::Invalid("Subject cannot contain newlines".into()));
        }
        let payload = serde_json::to_string(&req).expect("serializable request");
        if payload.len() > 10 * 1024 * 1024 {
            return Err(Error::Invalid(
                "Local message limit is 10 MiB of serialized payload".into(),
            ));
        }
        let domain = req.from.split_once('@').expect("validated address").1;
        let verified: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM identities WHERE name=?1 OR lower(name)=lower(?2))",
            params![req.from, domain],
            |r| r.get(0),
        )?;
        if !verified {
            return Err(Error::Rejected("Email address is not verified".into()));
        }
        let id = uuid::Uuid::new_v4().to_string();
        self.conn.execute(
            "INSERT INTO messages(id,created_at_ms,payload) VALUES (?1,?2,?3)",
            params![id, now_ms, payload],
        )?;
        Ok(id)
    }
    /// Stable insertion ordering; bounded pages for local inspection.
    pub fn messages(&self, offset: u32, limit: u32) -> Result<Vec<Message>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id,created_at_ms,payload FROM messages ORDER BY sequence LIMIT ?1 OFFSET ?2",
        )?;
        let rows = stmt.query_map(params![limit.min(100), offset], |r| {
            let payload: String = r.get(2)?;
            let request = serde_json::from_str(&payload).map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;
            Ok(Message {
                id: r.get(0)?,
                created_at_ms: r.get(1)?,
                request,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Newest-first mailbox view of captured messages; filters before pagination.
    pub fn mailbox(
        &self,
        received: bool,
        address: &str,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<Message>, Error> {
        let mut stmt = self.conn.prepare(
            "SELECT id, created_at_ms, payload FROM messages
            WHERE ?1 = '' OR (?2 = 0 AND json_extract(payload, '$.from') = ?1)
            OR (?2 = 1 AND EXISTS (
                SELECT 1 FROM json_each(payload, '$.destination') AS groups,
                json_each(groups.value) AS addresses WHERE addresses.value = ?1))
            ORDER BY sequence DESC LIMIT ?3 OFFSET ?4",
        )?;
        let rows = stmt.query_map(
            params![address, received, limit.min(100), offset],
            decode_message,
        )?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn message(&self, id: &str) -> Result<Option<Message>, Error> {
        use rusqlite::OptionalExtension;
        Ok(self
            .conn
            .query_row(
                "SELECT id, created_at_ms, payload FROM messages WHERE id=?1",
                [id],
                decode_message,
            )
            .optional()?)
    }
}
fn decode_message(row: &rusqlite::Row<'_>) -> rusqlite::Result<Message> {
    let payload: String = row.get(2)?;
    let request = serde_json::from_str(&payload).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text, Box::new(e))
    })?;
    Ok(Message {
        id: row.get(0)?,
        created_at_ms: row.get(1)?,
        request,
    })
}
fn valid_domain(s: &str) -> bool {
    s.len() <= 253
        && s.contains('.')
        && s.split('.').all(|p| {
            !p.is_empty()
                && p.len() <= 63
                && !p.starts_with('-')
                && !p.ends_with('-')
                && p.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
}
fn valid_address(s: &str) -> bool {
    s.split_once('@').is_some_and(|(local, domain)| {
        !local.is_empty()
            && local.len() <= 64
            && !local.starts_with('.')
            && !local.ends_with('.')
            && !local.contains("..")
            && local
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b".!#$%&'*+-/=?^_`{|}~".contains(&c))
            && valid_domain(domain)
    })
}
