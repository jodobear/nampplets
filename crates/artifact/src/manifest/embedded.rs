//! Capability domains declared inside the verified entry document.
//!
//! Signed `requires` tags stay authoritative. Napplets published without them
//! still declare their domains in the `napplet-requires` meta element of
//! `/index.html`, and those bytes are pinned by the signed path digest and the
//! signed aggregate, so they carry exactly the authority of the tags. Reading
//! them lets such builds reach permission review instead of launching with an
//! empty capability inventory.
//!
//! The scan is deliberately narrow: only `<meta>` elements in the document head
//! are considered, raw-text and comment regions are skipped so a string literal
//! inside a bundled script cannot forge a declaration, and only names already
//! inside the pinned compatibility inventory survive.

use super::KNOWN_REQUIREMENTS;

const REQUIRES_META_NAME: &str = "napplet-requires";
/// Generous enough for the largest declaration the trusted shell will read
/// back out of the same head, so no legal element truncates the scan.
const MAXIMUM_ELEMENT_BYTES: usize = 256 * 1024;

/// Extracts the capability domains declared by `<meta name="napplet-requires">`
/// in the head of one verified entry document, in declaration order.
///
/// Unknown or malformed names are ignored rather than refused: this path only
/// proposes a permission review, and a bounded subset is always safer than
/// refusing to install a build whose signed tags already verified.
pub fn embedded_requirements(document: &[u8]) -> Vec<&'static str> {
    let mut domains: Vec<&'static str> = Vec::new();
    let mut cursor = 0usize;
    while cursor < document.len() {
        let Some(open) = next_tag(document, cursor) else {
            break;
        };
        if starts_with_ignoring_case(&document[open..], b"<!--") {
            cursor = skip_until(document, open + 4, b"-->").unwrap_or(document.len());
            continue;
        }
        let Some(name_end) = tag_name_end(document, open) else {
            cursor = open + 1;
            continue;
        };
        let name = &document[open + 1..name_end];
        if equals_ignoring_case(name, b"body") || equals_ignoring_case(name, b"/head") {
            break;
        }
        let Some(close) = element_end(document, name_end) else {
            break;
        };
        if equals_ignoring_case(name, b"meta") {
            read_meta(&document[name_end..close], &mut domains);
        }
        cursor = close + 1;
        if is_raw_text(name) {
            let mut terminator = Vec::with_capacity(name.len() + 3);
            terminator.extend_from_slice(b"</");
            terminator.extend_from_slice(name);
            cursor = skip_until(document, cursor, &terminator).unwrap_or(document.len());
        }
    }
    domains
}

fn read_meta(attributes: &[u8], domains: &mut Vec<&'static str>) {
    let mut is_requires = false;
    let mut content: Option<&[u8]> = None;
    let mut cursor = 0usize;
    while let Some((name, value, next)) = next_attribute(attributes, cursor) {
        if equals_ignoring_case(name, b"name") {
            is_requires = equals_ignoring_case(value, REQUIRES_META_NAME.as_bytes());
        } else if equals_ignoring_case(name, b"content") {
            content = Some(value);
        }
        cursor = next;
    }
    let (true, Some(content)) = (is_requires, content) else {
        return;
    };
    for field in content.split(|byte| *byte == b',') {
        if domains.len() == KNOWN_REQUIREMENTS.len() {
            return;
        }
        let field = trim_ascii(field);
        let Some(known) = KNOWN_REQUIREMENTS
            .iter()
            .find(|known| equals_ignoring_case(field, known.as_bytes()))
        else {
            continue;
        };
        if !domains.contains(known) {
            domains.push(known);
        }
    }
}

/// Reads one `name="value"` pair, returning the offset just past it.
fn next_attribute(attributes: &[u8], from: usize) -> Option<(&[u8], &[u8], usize)> {
    let mut cursor = from;
    while cursor < attributes.len()
        && (attributes[cursor].is_ascii_whitespace() || attributes[cursor] == b'/')
    {
        cursor += 1;
    }
    let start = cursor;
    while cursor < attributes.len()
        && !attributes[cursor].is_ascii_whitespace()
        && attributes[cursor] != b'='
        && attributes[cursor] != b'/'
    {
        cursor += 1;
    }
    if cursor == start {
        return None;
    }
    let name = &attributes[start..cursor];
    while cursor < attributes.len() && attributes[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    if cursor >= attributes.len() || attributes[cursor] != b'=' {
        return Some((name, &[], cursor));
    }
    cursor += 1;
    while cursor < attributes.len() && attributes[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    if cursor >= attributes.len() {
        return Some((name, &[], cursor));
    }
    let quote = attributes[cursor];
    if quote == b'"' || quote == b'\'' {
        cursor += 1;
        let start = cursor;
        while cursor < attributes.len() && attributes[cursor] != quote {
            cursor += 1;
        }
        let value = &attributes[start..cursor];
        return Some((name, value, (cursor + 1).min(attributes.len())));
    }
    let start = cursor;
    while cursor < attributes.len() && !attributes[cursor].is_ascii_whitespace() {
        cursor += 1;
    }
    Some((name, &attributes[start..cursor], cursor))
}

fn next_tag(document: &[u8], from: usize) -> Option<usize> {
    document[from..]
        .iter()
        .position(|byte| *byte == b'<')
        .map(|offset| from + offset)
}

fn tag_name_end(document: &[u8], open: usize) -> Option<usize> {
    let mut cursor = open + 1;
    if cursor < document.len() && document[cursor] == b'/' {
        cursor += 1;
    }
    let start = cursor;
    while cursor < document.len()
        && (document[cursor].is_ascii_alphanumeric() || document[cursor] == b'-')
    {
        cursor += 1;
    }
    (cursor > start).then_some(cursor)
}

/// Finds the `>` that closes one element, ignoring `>` inside quoted values.
fn element_end(document: &[u8], from: usize) -> Option<usize> {
    let limit = from
        .saturating_add(MAXIMUM_ELEMENT_BYTES)
        .min(document.len());
    let mut cursor = from;
    let mut quote: Option<u8> = None;
    while cursor < limit {
        let byte = document[cursor];
        match quote {
            Some(open) if byte == open => quote = None,
            Some(_) => {}
            None if byte == b'"' || byte == b'\'' => quote = Some(byte),
            None if byte == b'>' => return Some(cursor),
            None => {}
        }
        cursor += 1;
    }
    None
}

fn skip_until(document: &[u8], from: usize, terminator: &[u8]) -> Option<usize> {
    let mut cursor = from;
    while cursor + terminator.len() <= document.len() {
        if equals_ignoring_case(&document[cursor..cursor + terminator.len()], terminator) {
            return Some(cursor + terminator.len());
        }
        cursor += 1;
    }
    None
}

fn is_raw_text(name: &[u8]) -> bool {
    equals_ignoring_case(name, b"script")
        || equals_ignoring_case(name, b"style")
        || equals_ignoring_case(name, b"textarea")
        || equals_ignoring_case(name, b"title")
}

fn starts_with_ignoring_case(value: &[u8], prefix: &[u8]) -> bool {
    value.len() >= prefix.len() && equals_ignoring_case(&value[..prefix.len()], prefix)
}

fn equals_ignoring_case(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
}

fn trim_ascii(value: &[u8]) -> &[u8] {
    let mut start = 0usize;
    let mut end = value.len();
    while start < end && value[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && value[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    &value[start..end]
}
