//! Local contact index — search / list / show / clear over
//! `<data_local_dir>/mail-cli/contacts.jsonl` (populated by `message pull`).

use serde_json::json;

use crate::cli::{
    ContactCommand, ContactListArgs, ContactSearchArgs, ContactSearchField, ContactSort, GlobalArgs,
};
use crate::contacts;
use crate::error::Result;
use crate::output::{OutputFormat, emit};

pub async fn run(cmd: ContactCommand, _global: &GlobalArgs, fmt: OutputFormat) -> Result<()> {
    match cmd {
        ContactCommand::Search(args) => search(args, fmt).await,
        ContactCommand::List(args) => list(args, fmt).await,
        ContactCommand::Show { email } => show(&email, fmt).await,
        ContactCommand::Clear => clear(fmt).await,
        ContactCommand::Path => path(fmt).await,
    }
}

async fn search(args: ContactSearchArgs, fmt: OutputFormat) -> Result<()> {
    let path = contacts::default_store_path()?;
    let all = contacts::load_merged(&path)?;

    // Each term must match somewhere (in the chosen field). Case-insensitive substring.
    let terms: Vec<String> = args
        .query
        .iter()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
        .collect();

    let matched: Vec<&contacts::Contact> = all
        .iter()
        .filter(|c| {
            terms.iter().all(|t| match args.field {
                ContactSearchField::Any => contacts::matches(c, t),
                ContactSearchField::Email => c.email.contains(t),
                ContactSearchField::Name => c
                    .names
                    .iter()
                    .any(|n| n.to_lowercase().contains(t)),
            })
        })
        .collect();

    // Sort by relevance-ish: recent + frequent first.
    let mut sorted = matched.clone();
    sorted.sort_by(|a, b| {
        b.last_seen
            .cmp(&a.last_seen)
            .then_with(|| b.count.cmp(&a.count))
    });
    sorted.truncate(args.limit as usize);

    emit(
        &json!({
            "query": args.query,
            "field": match args.field {
                ContactSearchField::Any => "any",
                ContactSearchField::Email => "email",
                ContactSearchField::Name => "name",
            },
            "total_matches": matched.len(),
            "returned": sorted.len(),
            "results": sorted,
        }),
        fmt,
    )
}

async fn list(args: ContactListArgs, fmt: OutputFormat) -> Result<()> {
    let path = contacts::default_store_path()?;
    let mut all = contacts::load_merged(&path)?;
    match args.sort {
        ContactSort::LastSeen => all.sort_by(|a, b| b.last_seen.cmp(&a.last_seen)),
        ContactSort::Count => all.sort_by(|a, b| b.count.cmp(&a.count)),
        ContactSort::Email => all.sort_by(|a, b| a.email.cmp(&b.email)),
    }
    let total = all.len();
    all.truncate(args.limit as usize);
    emit(
        &json!({
            "total": total,
            "returned": all.len(),
            "sort": match args.sort {
                ContactSort::LastSeen => "last_seen",
                ContactSort::Count => "count",
                ContactSort::Email => "email",
            },
            "contacts": all,
        }),
        fmt,
    )
}

async fn show(email: &str, fmt: OutputFormat) -> Result<()> {
    let path = contacts::default_store_path()?;
    let all = contacts::load_merged(&path)?;
    let hit = contacts::find_by_email(&all, email);
    emit(
        &json!({
            "email": email,
            "contact": hit,
        }),
        fmt,
    )
}

async fn clear(fmt: OutputFormat) -> Result<()> {
    let path = contacts::default_store_path()?;
    let freed = contacts::clear(&path)?;
    emit(
        &json!({
            "status": "ok",
            "path": path,
            "freed_bytes": freed,
        }),
        fmt,
    )
}

async fn path(fmt: OutputFormat) -> Result<()> {
    let path = contacts::default_store_path()?;
    let exists = path.exists();
    let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    emit(
        &json!({
            "path": path,
            "exists": exists,
            "size_bytes": size,
        }),
        fmt,
    )
}
