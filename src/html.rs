/// Convert HTML to plain text with a fixed line width (default 80).
///
/// Returns `(text, remote_resource_count)` where `remote_resource_count` is a
/// rough count of `http(s)://` references in the source, useful to log how
/// many remote images/links would have been fetched by a naive HTML renderer.
pub fn html_to_text(html: &str, width: usize) -> (String, u32) {
    let remote = count_remote_urls(html);
    let text = html2text::from_read(html.as_bytes(), width);
    (text, remote)
}

fn count_remote_urls(html: &str) -> u32 {
    (html.matches("http://").count() + html.matches("https://").count()) as u32
}
