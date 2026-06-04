use toolkit_core::{classify_url, UrlMatch};

#[test]
fn creator_home_desktop() {
    let m = classify_url("https://www.douyin.com/user/MS4wLjABAAAAabcdEFG_-123?foo=bar");
    assert_eq!(
        m,
        UrlMatch::CreatorHome {
            sec_uid: "MS4wLjABAAAAabcdEFG_-123".into()
        }
    );
}

#[test]
fn creator_home_share() {
    let m = classify_url("https://m.douyin.com/share/user/MS4wLjABAAAAxyz");
    assert_eq!(
        m,
        UrlMatch::CreatorHome {
            sec_uid: "MS4wLjABAAAAxyz".into()
        }
    );
}

#[test]
fn creator_home_short_link() {
    let m = classify_url("https://v.douyin.com/abc123/");
    assert_eq!(
        m,
        UrlMatch::CreatorHomeShort {
            short_code: "abc123".into()
        }
    );
}

#[test]
fn work_page() {
    let m = classify_url("https://www.douyin.com/video/7123456789012345678");
    assert_eq!(
        m,
        UrlMatch::Work {
            aweme_id: "7123456789012345678".into()
        }
    );
}

#[test]
fn search_page() {
    let m = classify_url("https://www.douyin.com/search/熊猫?type=user");
    assert_eq!(m, UrlMatch::Search);
}

#[test]
fn unknown_url() {
    assert_eq!(classify_url("https://example.com/foo"), UrlMatch::None);
    assert_eq!(
        classify_url("https://www.douyin.com/discover"),
        UrlMatch::None
    );
}
