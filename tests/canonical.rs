#![allow(clippy::unwrap_used, clippy::expect_used)]

use ddp_transcribe::canonical::{canonicalize_url, Canonical};

#[test]
fn canonicalizes_form_1_tiktokv_share_video() {
    let result = canonicalize_url("https://www.tiktokv.com/share/video/7234567890123456789/");
    match result {
        Canonical::VideoId(id) => assert_eq!(id, "7234567890123456789"),
        other => panic!("expected VideoId, got {other:?}"),
    }
}

#[test]
fn canonicalizes_form_2_tiktok_user_video() {
    let result = canonicalize_url("https://www.tiktok.com/@coolcreator/video/7234567890123456789");
    match result {
        Canonical::VideoId(id) => assert_eq!(id, "7234567890123456789"),
        other => panic!("expected VideoId, got {other:?}"),
    }
}

#[test]
fn canonicalizes_form_1_with_query_string() {
    let result = canonicalize_url(
        "https://www.tiktokv.com/share/video/7234567890123456789/?utm_source=share",
    );
    match result {
        Canonical::VideoId(id) => assert_eq!(id, "7234567890123456789"),
        other => panic!("expected VideoId, got {other:?}"),
    }
}

#[test]
fn marks_short_link_form_3_as_needs_resolution() {
    let result = canonicalize_url("https://vm.tiktok.com/ZMabcdef/");
    match result {
        Canonical::NeedsResolution(url) => {
            assert_eq!(url, "https://vm.tiktok.com/ZMabcdef/");
        }
        other => panic!("expected NeedsResolution, got {other:?}"),
    }
}

#[test]
fn marks_short_link_form_4_as_needs_resolution() {
    let result = canonicalize_url("https://www.tiktok.com/t/ZTabcdef/");
    match result {
        Canonical::NeedsResolution(url) => {
            assert_eq!(url, "https://www.tiktok.com/t/ZTabcdef/");
        }
        other => panic!("expected NeedsResolution, got {other:?}"),
    }
}

// Coverage-fill test (per ADR 0003): the regex already handles vt.tiktok.com
// as a recognized short-link host, but the plan's test list omitted this form.
// Added so a regression that drops vt from the alternation would be caught.
#[test]
fn marks_short_link_vt_tiktok_as_needs_resolution() {
    let result = canonicalize_url("https://vt.tiktok.com/ZSabcdef/");
    match result {
        Canonical::NeedsResolution(url) => {
            assert_eq!(url, "https://vt.tiktok.com/ZSabcdef/");
        }
        other => panic!("expected NeedsResolution, got {other:?}"),
    }
}

#[test]
fn rejects_non_tiktok_url() {
    match canonicalize_url("https://example.com/video/123") {
        Canonical::Invalid(_) => {}
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[test]
fn rejects_malformed_url() {
    match canonicalize_url("not a url at all") {
        Canonical::Invalid(_) => {}
        other => panic!("expected Invalid, got {other:?}"),
    }
}
