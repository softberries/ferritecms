use std::sync::{Arc, atomic::Ordering};

use ferrite_core::events::{archive_key, menu_key, post_key};
use ferrite_domain::ContentId;
use ferrite_theme::{ImageRef, MAX_BRIDGE_CALLS, RenderOutput, Renderer};

use crate::common::{FakeBridge, ctx, item, keys, menu_tree, render_str, theme};

fn render_with(
    src: &str,
    c: ferrite_theme::RenderContext,
    bridge: Arc<FakeBridge>,
) -> RenderOutput {
    let t = theme(&[("templates/t.html", src)]);
    Renderer::new().render(&t, "t.html", c, bridge).unwrap()
}

#[test]
fn head_is_unescaped_asset_is_hashed() {
    let mut c = ctx();
    c.head_html = "<meta name=\"x\" content=\"y\">".into();
    let t = theme(&[(
        "templates/t.html",
        "{{ head() }}|{{ asset('css/a b.css') }}",
    )]);
    let out = Renderer::new()
        .render(&t, "t.html", c, Arc::new(ferrite_theme::NoBridge))
        .unwrap();
    assert_eq!(
        out.html,
        format!(
            "<meta name=\"x\" content=\"y\">|&#x2f;_theme&#x2f;{}&#x2f;css&#x2f;a%20b.css",
            t.hash()
        )
    );
    assert!(out.deps.contains("theme"));
}

#[test]
fn filters_in_templates() {
    let mut c = ctx();
    c.item = Some(item("post", "Hello, Wörld!"));
    let out = render_str(
        "{{ post.date | date('%d %b %Y') }}|{{ post.body | excerpt(1) }}|{{ post.title | slugify }}|\
         <script>var d = {{ {'t': post.title, 'x': '</script>'} | json }};</script>",
        c,
    )
    .unwrap();
    assert_eq!(
        out.html,
        "05 Mar 2024|Body…|hello-world|<script>var d = {\"t\":\"Hello, Wörld!\",\"x\":\"\\u003c/script\\u003e\"};</script>"
    );
}

#[test]
fn date_rejects_bad_format_without_panicking() {
    let mut c = ctx();
    c.item = Some(item("post", "x"));
    for fmt in ["%Q", "%", "%-", "%:"] {
        let err = render_str(&format!("{{{{ post.date | date('{fmt}') }}}}"), c.clone());
        assert!(err.is_err(), "{fmt}");
    }
}

#[test]
fn menu_renders_tree_records_deps_and_is_memoised() {
    let target = ContentId::new();
    // A draft target the bridge removed from the tree but still reports.
    let hidden = ContentId::new();
    let bridge = Arc::new(FakeBridge {
        menu: Some(menu_tree()),
        deps: keys(&[post_key(target), post_key(hidden)]),
        ..FakeBridge::default()
    });
    let out = render_with(
        "{% for n in menu('primary').items %}<a href=\"{{ n.url }}\">{{ n.label }}</a>\
         {% for c in n.children %}[{{ c.label }}]{% endfor %}{% endfor %}\
         {{ menu('primary').items | length }}{{ menu('primary').items[0].target is defined }}",
        ctx(),
        bridge.clone(),
    );
    assert_eq!(
        out.html,
        "<a href=\"&#x2f;\">Home &amp; more</a>[About]1False"
    );
    assert_eq!(bridge.calls.load(Ordering::SeqCst), 1);
    assert!(out.deps.contains(&menu_key("primary")));
    assert!(out.deps.contains(&post_key(target)));
    assert!(out.deps.contains(&post_key(hidden)));
}

#[test]
fn bridge_reported_deps_reach_the_output() {
    // related(<uuid>) with an empty result: the bridge's archive and the referenced
    // (non-public) source item are still recorded.
    let draft = ContentId::new();
    let bridge = Arc::new(FakeBridge {
        deps: keys(&[archive_key("event"), post_key(draft)]),
        ..FakeBridge::default()
    });
    let out = render_with(
        &format!("{{{{ related('{draft}') | length }}}}"),
        ctx(),
        bridge,
    );
    assert_eq!(out.html, "0");
    assert!(out.deps.contains(&archive_key("event")));
    assert!(out.deps.contains(&post_key(draft)));
    // query() merges the bridge's keys too (e.g. the term it filtered on).
    let term = ferrite_domain::TermId::new();
    let bridge = Arc::new(FakeBridge {
        deps: keys(&[ferrite_core::events::term_key(term)]),
        ..FakeBridge::default()
    });
    let out = render_with(
        "{{ query(type='post', term='category/x') | length }}",
        ctx(),
        bridge,
    );
    assert!(out.deps.contains(&ferrite_core::events::term_key(term)));
}

#[test]
fn missing_menu_still_records_its_key() {
    let bridge = Arc::new(FakeBridge::default());
    let out = render_with("{{ menu('footer') is none }}", ctx(), bridge);
    assert_eq!(out.html, "True");
    assert!(out.deps.contains(&menu_key("footer")));
}

#[test]
fn related_passes_item_and_clamps_n() {
    let rel = item("post", "Rel");
    let rel_id = rel.item.id;
    let bridge = Arc::new(FakeBridge {
        related: vec![rel],
        ..FakeBridge::default()
    });
    let mut c = ctx();
    let it = item("post", "Main");
    let id = it.item.id;
    c.item = Some(it);
    let out = render_with(
        "{% for r in related(post) %}{{ r.title }}{% endfor %}{{ related(post, 500) | length }}{{ related('junk') | length }}",
        c,
        bridge.clone(),
    );
    assert_eq!(out.html, "Rel10");
    assert_eq!(
        *bridge.related_args.lock().unwrap(),
        vec![(id, 3), (id, 10)]
    );
    assert!(out.deps.contains(&post_key(rel_id)));
    assert!(out.deps.contains(&archive_key("post")));
}

#[test]
fn query_takes_kwargs_and_clamps_limit() {
    let bridge = Arc::new(FakeBridge::default());
    let out = render_with(
        "{% for p in query(type='event', term='category/news', limit=500) %}{{ p.title }}{% endfor %}\
         {{ query(type='post') | length }}",
        ctx(),
        bridge.clone(),
    );
    assert_eq!(out.html, "Queried1");
    assert_eq!(
        *bridge.queries.lock().unwrap(),
        vec![
            ("event".into(), Some("category/news".into()), 50),
            ("post".into(), None, 10)
        ]
    );
    assert!(out.deps.contains(&archive_key("event")));
    assert!(out.deps.contains(&archive_key("post")));
    let t = theme(&[("templates/t.html", "{{ query(kind='post') }}")]);
    let err = Renderer::new()
        .render(&t, "t.html", ctx(), Arc::new(FakeBridge::default()))
        .unwrap_err();
    assert!(err.to_string().contains("missing argument"), "{err}");
}

#[test]
fn image_uses_preloaded_media_then_the_bridge() {
    let bridge = Arc::new(FakeBridge {
        image: Some(ImageRef {
            url: "/_media/x/b.png".into(),
            width: Some(10),
            height: None,
            srcset: String::new(),
        }),
        ..FakeBridge::default()
    });
    let mut c = ctx();
    let it = item("post", "x");
    let media = it.featured.as_ref().unwrap().id;
    c.item = Some(it);
    let other = ContentId::new();
    let out = render_with(
        &format!(
            "{{% set i = image(post.featured, 'card') %}}{{{{ i.url }}}} {{{{ i.width }}}}x{{{{ i.height }}}} [{{{{ i.srcset }}}}]|\
             {{{{ image(post.featured, 'nope').url }}}}|{{{{ image('{other}', 'card').url }}}}|{{{{ image('bad') is none }}}}"
        ),
        c,
        bridge.clone(),
    );
    assert_eq!(
        out.html,
        "&#x2f;_media&#x2f;h2&#x2f;a-card.jpg 640x360 [&#x2f;_media&#x2f;h2&#x2f;a-card.jpg 640w, &#x2f;_media&#x2f;h&#x2f;a.jpg 1600w]|\
         &#x2f;_media&#x2f;h&#x2f;a.jpg|&#x2f;_media&#x2f;x&#x2f;b.png|True"
    );
    assert_eq!(bridge.calls.load(Ordering::SeqCst), 1);
    assert!(out.deps.contains(&post_key(media)));
    assert!(out.deps.contains(&post_key(other)));
}

#[test]
fn bridge_calls_are_capped_per_render() {
    let bridge = Arc::new(FakeBridge::default());
    let out = render_with(
        "{% for i in range(30) %}{{ query(type='post') | length }}{% endfor %}",
        ctx(),
        bridge.clone(),
    );
    let max = MAX_BRIDGE_CALLS as usize;
    assert_eq!(
        out.html,
        format!("{}{}", "1".repeat(max), "0".repeat(30 - max))
    );
    assert_eq!(bridge.calls.load(Ordering::SeqCst), MAX_BRIDGE_CALLS);
    // The budget is per render.
    let out = render_with("{{ query(type='post') | length }}", ctx(), bridge.clone());
    assert_eq!(out.html, "1");
}
