#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SniffedMime {
    Png,
    Jpeg,
    Gif,
    Webp,
    Avif,
    Svg,
}

impl SniffedMime {
    pub(super) fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
            Self::Avif => "image/avif",
            Self::Svg => "image/svg+xml",
        }
    }
}

pub(super) fn sniff_mime(bytes: &[u8]) -> Option<SniffedMime> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(SniffedMime::Png);
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some(SniffedMime::Jpeg);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some(SniffedMime::Gif);
    }
    if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(SniffedMime::Webp);
    }
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" && matches!(&bytes[8..12], b"avif" | b"avis") {
        return Some(SniffedMime::Avif);
    }
    if looks_like_svg(bytes) {
        return Some(SniffedMime::Svg);
    }
    None
}

fn looks_like_svg(bytes: &[u8]) -> bool {
    let prefix = &bytes[..bytes.len().min(4_096)];
    let Ok(text) = std::str::from_utf8(prefix) else {
        return false;
    };
    let text = text.trim_start_matches('\u{feff}').trim_start();
    let lowercase = text.to_ascii_lowercase();
    lowercase.starts_with("<svg")
        || (lowercase.starts_with("<?xml")
            && lowercase
                .find("<svg")
                .is_some_and(|position| position < 1_024))
}
