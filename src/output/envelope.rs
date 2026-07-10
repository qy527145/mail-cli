use serde::{Deserialize, Serialize};

/// Envelope list response, compatible with Himalaya's `--json` output shape.
#[derive(Debug, Serialize, Deserialize)]
pub struct EnvelopeList {
    pub envelopes: Vec<Envelope>,
    pub pagination: Pagination,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Envelope {
    pub id: String,
    #[serde(rename = "message-id")]
    pub message_id: String,
    pub flags: Vec<Flag>,
    pub subject: String,
    pub from: Vec<Address>,
    pub to: Vec<Address>,
    /// ISO-8601 datetime, if the message carried a parseable Date header.
    pub date: Option<String>,
    pub size: u64,
    #[serde(rename = "has-attachment")]
    pub has_attachment: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Flag {
    pub raw: String,
    pub iana: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Address {
    pub name: Option<String>,
    pub email: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Pagination {
    pub page: u32,
    pub page_size: u32,
    pub total_estimate: Option<u64>,
}
