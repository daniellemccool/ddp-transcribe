use once_cell::sync::Lazy;
use regex::Regex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Canonical {
    /// URL parsed cleanly to a 19-digit numeric video_id (forms 1 and 2).
    VideoId(String),

    /// Short link (forms 3 and 4): `vm.tiktok.com/...` or `tiktok.com/t/...`.
    /// Cannot extract video_id without following the redirect. Plan C resolves
    /// these; Plan A logs and skips them.
    NeedsResolution(String),

    /// Not a TikTok URL or unparseable.
    Invalid(String),
}

// Form 1: https://www.tiktokv.com/share/video/{19-digit-id}/[?...]
// Form 2: https://www.tiktok.com/@username/video/{19-digit-id}[?...]
// A malformed *literal* regex is a compile-time programmer error, not a runtime
// condition, so expect() on the construction is the idiomatic choice.
#[allow(clippy::expect_used)]
static CANONICAL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"^https?://(?:www\.)?(?:tiktokv|tiktok)\.com/(?:share/video|@[^/]+/video)/(\d{19})(?:/|\?|$)",
    )
    .expect("canonical regex compiles")
});

// Forms 3 and 4: short links that 302 to a canonical form.
// As above: a malformed literal regex is a compile-time programmer error.
#[allow(clippy::expect_used)]
static SHORT_LINK_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"^https?://(?:vm\.tiktok\.com|vt\.tiktok\.com|(?:www\.)?tiktok\.com/t)/[A-Za-z0-9]+/?$",
    )
    .expect("short-link regex compiles")
});

// expect: capture group 1 is structurally present in CANONICAL_RE, so a successful
// match guarantees it captured.
#[allow(clippy::expect_used)]
pub fn canonicalize_url(url: &str) -> Canonical {
    if let Some(captures) = CANONICAL_RE.captures(url) {
        let id = captures.get(1).expect("group 1 captured").as_str();
        return Canonical::VideoId(id.to_string());
    }
    if SHORT_LINK_RE.is_match(url) {
        return Canonical::NeedsResolution(url.to_string());
    }
    Canonical::Invalid(url.to_string())
}
