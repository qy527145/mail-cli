use email::envelope::Envelope as EmailEnvelope;
use email::envelope::address::Address as EmailAddress;
use email::flag::Flag as EmailFlag;

use crate::output::envelope::{Address, Envelope, Flag};

pub fn convert_envelope(e: &EmailEnvelope) -> Envelope {
    Envelope {
        id: e.id.clone(),
        message_id: e.message_id.clone(),
        flags: e.flags.iter().map(convert_flag).collect(),
        subject: e.subject.clone(),
        from: vec![convert_address(&e.from)],
        to: vec![convert_address(&e.to)],
        date: Some(e.date.to_rfc3339()),
        // email-lib's Envelope does not expose message size; fill in later if the backend provides it.
        size: 0,
        has_attachment: e.has_attachment,
    }
}

pub fn convert_address(a: &EmailAddress) -> Address {
    Address {
        name: a.name.clone(),
        email: a.addr.clone(),
    }
}

pub fn convert_flag(f: &EmailFlag) -> Flag {
    let (raw, iana) = match f {
        EmailFlag::Seen => (r"\Seen", Some("seen")),
        EmailFlag::Answered => (r"\Answered", Some("answered")),
        EmailFlag::Flagged => (r"\Flagged", Some("flagged")),
        EmailFlag::Deleted => (r"\Deleted", Some("deleted")),
        EmailFlag::Draft => (r"\Draft", Some("draft")),
        EmailFlag::Custom(s) => (s.as_str(), None),
    };
    Flag {
        raw: raw.to_string(),
        iana: iana.map(str::to_string),
    }
}
