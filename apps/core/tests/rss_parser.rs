use mediaflow_core::automation::rss::parser::{FeedFormat, FeedParseError, FeedParser};

const RSS: &[u8] = include_bytes!("fixtures/rss/rss20.xml");
const ATOM: &[u8] = include_bytes!("fixtures/rss/atom.xml");
const DOCTYPE: &[u8] = include_bytes!("fixtures/rss/doctype.xml");

#[test]
fn parses_rss_and_atom_into_bounded_secret_event_drafts() {
    let rss = FeedParser::default().parse(RSS).unwrap();
    assert_eq!(rss.format, FeedFormat::Rss20);
    assert_eq!(rss.items.len(), 2);
    assert_eq!(rss.ignored_item_count, 1);
    assert_eq!(rss.items[0].title.as_deref(), Some("Movie.One.2026"));
    assert_eq!(
        rss.items[0].source().expose(),
        b"magnet:?xt=urn:btih:0123456789abcdef0123456789abcdef01234567"
    );
    assert_ne!(rss.items[0].dedup_key, rss.items[1].dedup_key);
    assert!(!format!("{:?}", rss.items[1]).contains("token=private"));

    let atom = FeedParser::default().parse(ATOM).unwrap();
    assert_eq!(atom.format, FeedFormat::Atom);
    assert_eq!(atom.items.len(), 1);
    assert_eq!(atom.ignored_item_count, 1);
    assert_eq!(atom.items[0].title.as_deref(), Some("Series.S01E01"));
    assert_eq!(
        atom.items[0].source().expose(),
        b"https://downloads.example.test/episode-1.torrent?token=private"
    );
}

#[test]
fn source_fingerprint_is_stable_without_guid_and_guid_controls_identity_when_present() {
    let first = FeedParser::default().parse(RSS).unwrap();
    let second = FeedParser::default().parse(RSS).unwrap();
    assert_eq!(first.items[0].dedup_key, second.items[0].dedup_key);
    assert_eq!(first.items[1].dedup_key, second.items[1].dedup_key);

    let changed_source_same_guid = RSS
        .windows(b"01234567".len())
        .position(|window| window == b"01234567")
        .map(|offset| {
            let mut bytes = RSS.to_vec();
            bytes[offset..offset + 8].copy_from_slice(b"89abcdef");
            bytes
        })
        .unwrap();
    let changed = FeedParser::default()
        .parse(&changed_source_same_guid)
        .unwrap();
    assert_eq!(first.items[0].dedup_key, changed.items[0].dedup_key);
}

#[test]
fn rejects_active_xml_and_all_document_complexity_limits() {
    assert_eq!(
        FeedParser::default().parse(DOCTYPE),
        Err(FeedParseError::UnsafeXml)
    );
    assert_eq!(
        FeedParser::default().parse(&vec![b'x'; 2 * 1024 * 1024 + 1]),
        Err(FeedParseError::TooLarge)
    );

    let deep = format!(
        "<rss version=\"2.0\"><channel><item>{}<link>https://example.test/a.torrent</link>{}</item></channel></rss>",
        "<x>".repeat(31),
        "</x>".repeat(31)
    );
    assert_eq!(
        FeedParser::default().parse(deep.as_bytes()),
        Err(FeedParseError::TooDeep)
    );

    let many = format!(
        "<rss version=\"2.0\"><channel>{}</channel></rss>",
        "<item><guid>x</guid><link>https://example.test/a.torrent</link></item>".repeat(501)
    );
    assert_eq!(
        FeedParser::default().parse(many.as_bytes()),
        Err(FeedParseError::TooManyItems)
    );
}
