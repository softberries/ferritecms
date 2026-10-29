#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ferrite_import_wp::ImportError;
use ferrite_import_wp::model::{Item, Record};
use ferrite_import_wp::php::{self, PhpError, PhpValue};
use ferrite_import_wp::wxr::{self, ParseStats};

fn corpus(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus")
        .join(name)
}

fn read_all(bytes: &[u8]) -> Result<(Vec<Record>, ParseStats), ImportError> {
    let mut records = Vec::new();
    let stats = wxr::parse(bytes, |r| {
        records.push(r);
        Ok(())
    })?;
    Ok((records, stats))
}

fn read_file(name: &str) -> (Vec<Record>, ParseStats) {
    read_all(&std::fs::read(corpus(name)).unwrap()).unwrap()
}

fn items(records: &[Record]) -> Vec<&Item> {
    records
        .iter()
        .filter_map(|r| match r {
            Record::Item(i) => Some(&**i),
            _ => None,
        })
        .collect()
}

/// A minimal WXR document around `items`.
fn wxr_doc(items: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:excerpt="http://wordpress.org/export/1.2/excerpt/" xmlns:content="http://purl.org/rss/1.0/modules/content/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:wp="http://wordpress.org/export/1.2/">
<channel>
<title>T</title>
<wp:wxr_version>1.2</wp:wxr_version>
<wp:base_blog_url>http://example.test/</wp:base_blog_url>
{items}
</channel>
</rss>
"#
    )
}

#[test]
fn site_a_counts() {
    let (records, stats) = read_file("site-a.xml");
    assert!(matches!(records.first(), Some(Record::Channel(_))));
    let Some(Record::Channel(channel)) = records.first() else {
        unreachable!()
    };
    assert_eq!(channel.title, "Corpus Site");
    assert_eq!(channel.base_blog_url, "http://localhost:8099/");
    assert_eq!(channel.wxr_version, "1.2");
    assert_eq!(channel.language, "en-US");

    let items = items(&records);
    assert_eq!(items.len(), 35);
    assert_eq!(stats.items, 35);
    let mut by_type = BTreeMap::new();
    for i in &items {
        *by_type.entry(i.post_type.as_str()).or_insert(0) += 1;
    }
    let expected = BTreeMap::from([
        ("post", 8),
        ("page", 4),
        ("attachment", 4),
        ("nav_menu_item", 5),
        ("book", 2),
        ("acf-field-group", 1),
        ("acf-field", 11),
    ]);
    assert_eq!(by_type, expected);

    let logins: Vec<_> = records
        .iter()
        .filter_map(|r| match r {
            Record::Author(a) => Some(a.login.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(logins.len(), 4);
    for l in ["admin", "erin", "arlo", "cass"] {
        assert!(logins.contains(&l), "{l} missing from {logins:?}");
    }
    assert_eq!(stats.authors, 4);

    // Terms from <wp:category>, <wp:tag> and <wp:term>, with their parents.
    let local = records
        .iter()
        .find_map(|r| match r {
            Record::Term(t) if t.slug == "local" => Some(t),
            _ => None,
        })
        .unwrap();
    assert_eq!(local.taxonomy, "category");
    assert_eq!(local.parent_slug.as_deref(), Some("news"));
    assert_eq!(local.name, "Local News");
    let guides = records
        .iter()
        .find_map(|r| match r {
            Record::Term(t) if t.slug == "guides" => Some(t),
            _ => None,
        })
        .unwrap();
    assert_eq!(guides.parent_slug, None);
    assert_eq!(guides.description, "How-to articles");
    assert!(records.iter().any(
        |r| matches!(r, Record::Term(t) if t.taxonomy == "post_tag" && t.slug == "self-hosting")
    ));
    assert!(records.iter().any(|r| matches!(r, Record::Term(t) if t.taxonomy == "genre" && t.slug == "high-fantasy" && t.parent_slug.as_deref() == Some("fantasy"))));

    let hello = items.iter().find(|i| i.wp_id == 1).unwrap();
    assert_eq!(hello.title, "Hello world!");
    assert_eq!(hello.creator_login, "admin");
    assert_eq!(hello.status, "publish");
    assert_eq!(hello.post_name, "hello-world");
    assert_eq!(hello.date_gmt, "2026-10-28 10:47:31");
    assert_eq!(hello.guid, "http://localhost:8099/?p=1");
    assert!(
        hello
            .content
            .starts_with("<!-- wp:paragraph -->\n<p>Welcome")
    );
    assert_eq!(hello.terms.len(), 1);
    assert_eq!(hello.terms[0].taxonomy, "category");
    assert_eq!(hello.terms[0].slug, "uncategorized");
    assert_eq!(hello.terms[0].name, "Uncategorized");
    assert_eq!(hello.comments.len(), 1);
    assert!(
        hello.comments[0]
            .content
            .contains("<a href=\"https://gravatar.com/\">")
    );
    assert_eq!(hello.comments[0].approved, "1");

    let hero = items.iter().find(|i| i.title == "Hero").unwrap();
    assert_eq!(
        hero.attachment_url.as_deref(),
        Some("http://localhost:8099/wp-content/uploads/2026/10/hero.png")
    );
    assert!(hero.meta.contains(&(
        "_wp_attachment_image_alt".into(),
        "A red hero banner".into()
    )));

    let every = items
        .iter()
        .find(|i| i.title == "Every core block")
        .unwrap();
    assert!(every.meta.contains(&(
        "_yoast_wpseo_meta-robots-noindex".to_string(),
        "1".to_string()
    )));
    // site-a.php inserts four comments, but WordPress's exporter leaves out spam, so the file
    // holds three: two approved (threaded) and one pending.
    let approved: Vec<_> = every.comments.iter().map(|c| c.approved.as_str()).collect();
    assert_eq!(approved, ["1", "1", "0"]);
    assert_eq!(every.comments[1].parent_wp_id, every.comments[0].wp_id);
    assert_ne!(every.comments[1].user_id, 0);
}

#[test]
fn comment_meta_stays_out_of_item_meta() {
    let doc = wxr_doc(
        "<item><title>a</title><wp:post_id>7</wp:post_id><wp:post_type>post</wp:post_type>\
         <wp:postmeta><wp:meta_key>k</wp:meta_key><wp:meta_value><![CDATA[v]]></wp:meta_value></wp:postmeta>\
         <wp:comment><wp:comment_id>3</wp:comment_id><wp:comment_content><![CDATA[hi]]></wp:comment_content>\
         <wp:comment_parent>2</wp:comment_parent><wp:comment_user_id>5</wp:comment_user_id>\
         <wp:commentmeta><wp:meta_key>akismet_result</wp:meta_key><wp:meta_value>false</wp:meta_value></wp:commentmeta>\
         </wp:comment></item>",
    );
    let (records, stats) = read_all(doc.as_bytes()).unwrap();
    let item = items(&records)[0];
    assert_eq!(item.meta, vec![("k".to_string(), "v".to_string())]);
    assert_eq!(item.comments.len(), 1);
    assert_eq!(item.comments[0].wp_id, 3);
    assert_eq!(item.comments[0].content, "hi");
    assert_eq!(item.comments[0].parent_wp_id, 2);
    assert_eq!(item.comments[0].user_id, 5);
    assert_eq!(stats.comments, 1);
}

#[test]
fn site_b_counts() {
    let (records, stats) = read_file("site-b.xml");
    assert_eq!(items(&records).len(), 7);
    assert_eq!(stats.items, 7);
}

#[test]
fn records_round_trip_as_json() {
    let (records, _) = read_file("site-a.xml");
    let json = serde_json::to_string(&records).unwrap();
    let back: Vec<Record> = serde_json::from_str(&json).unwrap();
    assert_eq!(back, records);
}

#[test]
fn cdata_split_is_joined() {
    let doc = wxr_doc(
        "<item><title>a</title><wp:post_id>7</wp:post_id><wp:post_type>post</wp:post_type>\
         <content:encoded><![CDATA[x]]]]><![CDATA[>y &amp; z]]></content:encoded></item>",
    );
    let (records, _) = read_all(doc.as_bytes()).unwrap();
    let items = items(&records);
    assert_eq!(items[0].content, "x]]>y &amp; z");
}

#[test]
fn entities_and_char_refs_are_decoded_outside_cdata() {
    let doc = wxr_doc(
        "<item><title>Fish &amp; Chips &#8211; &#x2019;s &lt;b&gt;</title>\
         <wp:post_id>7</wp:post_id><wp:post_type>post</wp:post_type></item>",
    );
    let (records, _) = read_all(doc.as_bytes()).unwrap();
    assert_eq!(
        items(&records)[0].title,
        "Fish & Chips \u{2013} \u{2019}s <b>"
    );
}

#[test]
fn sink_error_stops_the_parse() {
    let bytes = std::fs::read(corpus("site-a.xml")).unwrap();
    let mut seen = 0;
    let err = wxr::parse(bytes.as_slice(), |r| {
        if let Record::Item(_) = r {
            seen += 1;
            if seen == 3 {
                return Err(ImportError::Missing {
                    what: "test stop",
                    offset: 0,
                });
            }
        }
        Ok(())
    })
    .unwrap_err();
    assert!(matches!(
        err,
        ImportError::Missing {
            what: "test stop",
            ..
        }
    ));
    assert_eq!(seen, 3);
}

#[test]
fn doctype_refused() {
    let doc = r#"<?xml version="1.0"?>
<!DOCTYPE rss [<!ENTITY xxe SYSTEM "file:///etc/passwd">]>
<rss><channel><title>&xxe;</title></channel></rss>"#;
    assert!(matches!(
        read_all(doc.as_bytes()),
        Err(ImportError::Doctype)
    ));
}

#[test]
fn undeclared_entity_is_an_error() {
    let doc = wxr_doc(
        "<item><title>&nbsp;</title><wp:post_id>7</wp:post_id><wp:post_type>post</wp:post_type></item>",
    );
    assert!(matches!(
        read_all(doc.as_bytes()),
        Err(ImportError::Xml { .. })
    ));
}

#[test]
fn invalid_utf8_reports_offset() {
    let doc = wxr_doc(
        "<item><title>ok</title><wp:post_id>7</wp:post_id><wp:post_type>post</wp:post_type>\
         <content:encoded><![CDATA[abc@@def]]></content:encoded></item>",
    );
    let mut bytes = doc.into_bytes();
    let at = bytes.windows(2).position(|w| w == b"@@").unwrap();
    bytes[at] = 0xff;
    match read_all(&bytes) {
        Err(ImportError::Utf8 { offset }) => assert_eq!(offset, at as u64),
        other => panic!("expected Utf8, got {other:?}"),
    }

    // Outside CDATA too.
    let doc = wxr_doc(
        "<item><title>ab@@</title><wp:post_id>7</wp:post_id><wp:post_type>post</wp:post_type></item>",
    );
    let mut bytes = doc.into_bytes();
    let at = bytes.windows(2).position(|w| w == b"@@").unwrap();
    bytes[at] = 0xc3;
    bytes[at + 1] = b'(';
    match read_all(&bytes) {
        Err(ImportError::Utf8 { offset }) => assert_eq!(offset, at as u64),
        other => panic!("expected Utf8, got {other:?}"),
    }
}

#[test]
fn truncated_file_errors() {
    let bytes = std::fs::read(corpus("site-a.xml")).unwrap();
    for cut in [bytes.len() / 3, bytes.len() / 2, bytes.len() - 20] {
        let res = read_all(&bytes[..cut]);
        assert!(res.is_err(), "cut at {cut} parsed: {:?}", res.map(|r| r.1));
    }
}

#[test]
fn nesting_is_capped_at_256() {
    let nest = |n: usize| wxr_doc(&format!("{}{}", "<x>".repeat(n), "</x>".repeat(n)));
    // rss and channel are two levels already.
    assert!(read_all(nest(254).as_bytes()).is_ok());
    match read_all(nest(255).as_bytes()) {
        Err(ImportError::Xml { msg, .. }) => assert!(msg.contains("256"), "{msg}"),
        other => panic!("expected Xml, got {other:?}"),
    }
}

#[test]
fn item_without_post_id_is_missing() {
    let doc = wxr_doc("<item><title>a</title><wp:post_type>post</wp:post_type></item>");
    assert!(matches!(
        read_all(doc.as_bytes()),
        Err(ImportError::Missing {
            what: "wp:post_id",
            ..
        })
    ));
}

#[test]
fn not_wxr_is_missing_channel() {
    assert!(matches!(
        read_all(b"<?xml version=\"1.0\"?><feed></feed>"),
        Err(ImportError::Missing {
            what: "channel",
            ..
        })
    ));
}

/// The WordPress theme-test files (GPL-2.0, not checked in, ruling P15) parse when present.
#[test]
fn wp_theme_test_data_parses() {
    let Some(dir) = std::env::var_os("FERRITE_WP_CORPUS") else {
        println!("SKIP: FERRITE_WP_CORPUS not set");
        return;
    };
    for name in ["themeunittestdata.wordpress.xml", "64-block-test-data.xml"] {
        let bytes = std::fs::read(Path::new(&dir).join(name)).unwrap();
        let (_, stats) = read_all(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(stats.items > 0, "{name}: no items");
    }
}

// PHP unserialize.

#[test]
fn php_site_a_repeater_parses() {
    let v = php::unserialize(
        r#"a:2:{i:0;a:1:{s:4:"text";s:9:"Quote one";}i:1;a:1:{s:4:"text";s:9:"Quote two";}}"#,
    )
    .unwrap();
    let row = |t: &str| {
        PhpValue::Array(vec![(
            PhpValue::Str("text".into()),
            PhpValue::Str(t.into()),
        )])
    };
    assert_eq!(
        v,
        PhpValue::Array(vec![
            (PhpValue::Int(0), row("Quote one")),
            (PhpValue::Int(1), row("Quote two")),
        ])
    );
}

#[test]
fn php_scalars() {
    assert_eq!(php::unserialize("N;").unwrap(), PhpValue::Null);
    assert_eq!(php::unserialize("b:1;").unwrap(), PhpValue::Bool(true));
    assert_eq!(php::unserialize("b:0;").unwrap(), PhpValue::Bool(false));
    assert_eq!(php::unserialize("i:-42;").unwrap(), PhpValue::Int(-42));
    assert_eq!(php::unserialize("d:0.5;").unwrap(), PhpValue::Float(0.5));
    assert_eq!(
        php::unserialize("d:1.0E+25;").unwrap(),
        PhpValue::Float(1.0e25)
    );
    assert_eq!(
        php::unserialize("d:-INF;").unwrap(),
        PhpValue::Float(f64::NEG_INFINITY)
    );
    assert_eq!(
        php::unserialize(r#"s:0:"";"#).unwrap(),
        PhpValue::Str(String::new())
    );
    // Quotes inside the string are taken by length, not escaped.
    assert_eq!(
        php::unserialize(r#"s:6:"a";b"c";"#).unwrap(),
        PhpValue::Str(r#"a";b"c"#.into())
    );
}

#[test]
fn php_site_attachment_metadata_parses() {
    let xml = std::fs::read_to_string(corpus("site-a.xml")).unwrap();
    let start = xml.find("<![CDATA[a:6:{s:5:\"width\"").unwrap() + "<![CDATA[".len();
    let end = start + xml[start..].find("]]>").unwrap();
    let PhpValue::Array(pairs) = php::unserialize(&xml[start..end]).unwrap() else {
        panic!("not an array")
    };
    assert_eq!(
        pairs[0],
        (PhpValue::Str("width".into()), PhpValue::Int(1600))
    );
    assert_eq!(pairs.len(), 6);
}

#[test]
fn php_string_length_counts_bytes() {
    assert_eq!(
        php::unserialize(r#"s:5:"café";"#).unwrap(),
        PhpValue::Str("café".into())
    );
    // Counting characters (4) is wrong and must not parse.
    assert!(php::unserialize(r#"s:4:"café";"#).is_err());
    // A length past the input.
    assert!(php::unserialize(r#"s:99:"short";"#).is_err());
    assert!(php::unserialize(r#"s:18446744073709551615:"x";"#).is_err());
}

#[test]
fn php_object_is_refused() {
    assert!(matches!(
        php::unserialize(r#"O:8:"stdClass":1:{s:1:"a";i:1;}"#),
        Err(PhpError::Object { .. })
    ));
    assert!(matches!(
        php::unserialize(r#"a:1:{i:0;O:8:"stdClass":0:{}}"#),
        Err(PhpError::Object { .. })
    ));
    assert!(matches!(
        php::unserialize(r#"C:3:"Foo":2:{xx}"#),
        Err(PhpError::Object { .. })
    ));
}

#[test]
fn php_depth_is_bounded() {
    let nest = |n: usize| format!("{}N;{}", "a:1:{i:0;".repeat(n), "}".repeat(n));
    assert!(php::unserialize(&nest(32)).is_ok());
    assert!(matches!(
        php::unserialize(&nest(33)),
        Err(PhpError::TooDeep { .. })
    ));
}

#[test]
fn php_malformed_input_errors() {
    for bad in [
        "",
        "x",
        "i:;",
        "i:1",
        "b:2;",
        "s:1:\"ab\";",
        "a:2:{i:0;N;}",
        "a:1:{i:0;N;",
        "a:1:{d:0.5;N;}",
        "a:99999999999:{}",
        "N;trailing",
        "r:1;",
    ] {
        assert!(php::unserialize(bad).is_err(), "{bad:?} parsed");
    }
}
