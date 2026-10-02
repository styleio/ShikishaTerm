//! Request handling shared by the settings and remote servers.

/// Refuse a known oversized body before reading it; bound unknown sizes too.
/// Each route supplies its own limit. Invalid UTF-8 is an I/O error.
pub(crate) fn read_body(
    req: &mut tiny_http::Request,
    max: usize,
) -> std::io::Result<Option<String>> {
    use std::io::Read as _;
    if req.body_length().is_some_and(|n| n > max) {
        return Ok(None);
    }
    let mut body = String::new();
    req.as_reader()
        .take((max as u64).saturating_add(1))
        .read_to_string(&mut body)?;
    Ok((body.len() <= max).then_some(body))
}
