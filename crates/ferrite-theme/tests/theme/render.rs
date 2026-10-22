use ferrite_core::events::{post_key, term_key, user_key};
use ferrite_theme::{RenderContext, RequestInfo, ThemeError};

use crate::common::{ctx, item, listing, render, render_str, theme};

#[test]
fn title_is_escaped_body_is_not() {
    let mut c = ctx();
    c.item = Some(item("post", "<script>alert(1)</script>"));
    let out = render_str("<h1>{{ post.title }}</h1>{{ post.body }}", c).unwrap();
    assert_eq!(
        out.html,
        "<h1>&lt;script&gt;alert(1)&lt;&#x2f;script&gt;</h1><p>Body <em>html</em></p>"
    );
}

#[test]
fn xml_is_escaped_txt_is_not() {
    let t = theme(&[
        ("templates/feed.xml", "<t>{{ site.title }}</t>"),
        ("templates/robots.txt", "{{ site.title }}"),
    ]);
    assert_eq!(
        render(&t, "feed.xml", ctx()).unwrap().html,
        "<t>My &lt;Site&gt;</t>"
    );
    assert_eq!(render(&t, "robots.txt", ctx()).unwrap().html, "My <Site>");
}

#[test]
fn post_and_page_globals() {
    let src = "{{ post.title }}|{{ page.title }}|{{ page is defined }}";
    let mut c = ctx();
    c.item = Some(item("post", "P"));
    assert_eq!(render_str(src, c).unwrap().html, "P||False");
    let mut c = ctx();
    c.item = Some(item("page", "About"));
    assert_eq!(render_str(src, c).unwrap().html, "About|About|True");
    assert_eq!(
        render_str("{{ post is defined }}{{ posts is defined }}", ctx())
            .unwrap()
            .html,
        "FalseFalse"
    );
}

#[test]
fn item_fields() {
    let mut c = ctx();
    let it = item("post", "T");
    let id = it.item.id;
    c.item = Some(it);
    let out = render_str(
        "{{ post.id }}|{{ post.kind }}|{{ post.slug }}|{{ post.url }}|{{ post.excerpt }}|\
         {{ post.date }}|{{ post.author.display_name }}|{{ post.author.login }}|\
         {{ post.terms[0].name }}|{{ post.terms[0].url }}|{{ post.featured.alt }}|\
         {{ post.fields.color }}|{{ post.parent_id }}",
        c,
    )
    .unwrap();
    assert_eq!(
        out.html,
        format!(
            "{id}|post|hello|&#x2f;hello&#x2f;|An excerpt|2024-03-05T10:20:30+00:00|Ana|ana|\
             News|&#x2f;category&#x2f;news&#x2f;|Alt|blue|None"
        )
    );
}

#[test]
fn listing_and_pagination() {
    let mut c = ctx();
    c.listing = Some(listing(vec![item("post", "A"), item("post", "B")], 2, 5));
    let out = render_str(
        "{% for p in posts.items %}{{ p.title }},{% endfor %}\
         {{ posts.page }}/{{ posts.pages }} prev={{ posts.prev_page }} next={{ posts.next_page }} total={{ posts.total }}",
        c,
    )
    .unwrap();
    assert_eq!(out.html, "A,B,2/3 prev=1 next=3 total=5");
    let mut c = ctx();
    c.listing = Some(listing(vec![], 1, 0));
    let out = render_str(
        "{{ posts.pages }} {{ posts.prev_page }} {{ posts.next_page }}",
        c,
    );
    assert_eq!(out.unwrap().html, "1 None None");
}

#[test]
fn request_info() {
    let c = RenderContext {
        request: RequestInfo {
            path: "/search/".into(),
            query_s: Some("<b>q</b>".into()),
            page: 0,
        },
        status: 404,
        preview: true,
        ..ctx()
    };
    let out = render_str(
        "{{ request.path }} {{ request.s }} {{ request.page }} {{ request.status }} {{ request.preview }}",
        c,
    )
    .unwrap();
    assert_eq!(
        out.html,
        "&#x2f;search&#x2f; &lt;b&gt;q&lt;&#x2f;b&gt; 1 404 True"
    );
}

#[test]
fn site_and_settings() {
    let t = theme(&[
        (
            "theme.toml",
            "[theme]\nname = \"t\"\nversion = \"0.1.0\"\n[settings]\naccent = { type = \"color\", default = \"#abc\" }\nwide = { type = \"bool\" }\n",
        ),
        (
            "templates/t.html",
            "{{ site.url }} {{ site.locale }} {{ site.description }} {{ site.settings.accent }} {{ site.settings.wide }}",
        ),
    ]);
    let mut c = ctx();
    c.site.settings = t
        .manifest()
        .settings_values(serde_json::json!({"wide": true}).as_object().unwrap());
    let out = render(&t, "t.html", c).unwrap();
    assert_eq!(out.html, "https:&#x2f;&#x2f;example.com en desc #abc True");
}

#[test]
fn include_extends_and_import() {
    let t = theme(&[
        (
            "templates/base.html",
            "<title>{% block title %}{% endblock %}</title>{% include \"partials/foot.html\" %}",
        ),
        (
            "templates/single.html",
            "{% extends \"base.html\" %}{% block title %}{{ post.title }}{% endblock %}",
        ),
        (
            "templates/partials/foot.html",
            "<footer>{{ site.title }}</footer>",
        ),
        (
            "templates/partials/macros.html",
            "{% macro brand() %}[{{ site.title }}|{{ asset('a.css') }}]{% endmacro %}",
        ),
        (
            "templates/m.html",
            "{% import \"partials/macros.html\" as m %}{{ m.brand() }}",
        ),
    ]);
    let mut c = ctx();
    c.item = Some(item("post", "Hi"));
    assert_eq!(
        render(&t, "single.html", c).unwrap().html,
        "<title>Hi</title><footer>My &lt;Site&gt;</footer>"
    );
    let out = render(&t, "m.html", ctx()).unwrap().html;
    assert_eq!(
        out,
        format!(
            "[My &lt;Site&gt;|&#x2f;_theme&#x2f;{}&#x2f;a.css]",
            t.hash()
        )
    );
}

#[test]
fn includes_cannot_leave_the_theme() {
    for src in [
        "{% include \"../../etc/passwd\" %}",
        "{% include \"/etc/passwd\" %}",
        "{% include \"../theme.toml\" %}",
        "{% include \"theme.toml\" %}",
        "{% include \"templates/index.html\" %}",
        "{% extends \"/etc/hosts\" %}",
        "{% import \"../../Cargo.toml\" as x %}",
    ] {
        let err = render_str(src, ctx()).unwrap_err();
        match &err {
            ThemeError::Render(m) => assert!(m.contains("template not found"), "{src}: {m}"),
            other => panic!("{src}: {other}"),
        }
    }
}

#[test]
fn missing_template_is_not_found() {
    let t = theme(&[]);
    assert!(matches!(
        render(&t, "nope.html", ctx()),
        Err(ThemeError::TemplateNotFound(_))
    ));
}

#[test]
fn runtime_errors_report_file_and_line() {
    let t = theme(&[
        (
            "templates/t.html",
            "a\nb\n{% include \"partials/p.html\" %}",
        ),
        ("templates/partials/p.html", "1\n2\n3\n{{ \"x\" | date }}"),
        ("templates/u.html", "\n{{ nope() }}"),
    ]);
    let err = render(&t, "t.html", ctx()).unwrap_err();
    let ThemeError::Render(m) = err else {
        panic!("{err}")
    };
    assert!(
        m.starts_with("templates/partials/p.html:4: invalid operation: date"),
        "{m}"
    );
    let err = render(&t, "u.html", ctx()).unwrap_err();
    let ThemeError::Render(m) = err else {
        panic!("{err}")
    };
    assert!(m.starts_with("templates/u.html:2: unknown function"), "{m}");
}

#[test]
fn builtins_left_and_removed() {
    assert_eq!(
        render_str("{{ range(3) | list }}{{ dict(a=1).a }}", ctx())
            .unwrap()
            .html,
        "[0, 1, 2]1"
    );
    let err = render_str("{{ debug() }}", ctx()).unwrap_err();
    assert!(err.to_string().contains("unknown function"), "{err}");
    let err = render_str("{{ 'a' | urlencode }}", ctx()).unwrap_err();
    assert!(err.to_string().contains("unknown filter"), "{err}");
}

#[test]
fn deps_cover_everything_shown() {
    let mut c = ctx();
    let single = item("post", "S");
    let listed = item("post", "L");
    let expect = [
        post_key(single.item.id),
        user_key(single.author.as_ref().unwrap().id),
        term_key(single.terms[0].id),
        post_key(single.featured.as_ref().unwrap().id),
        post_key(listed.item.id),
        term_key(listed.terms[0].id),
        "theme".to_owned(),
        "option:site.title".to_owned(),
        "option:site.url".to_owned(),
        "option:site.description".to_owned(),
        "option:site.locale".to_owned(),
    ];
    c.item = Some(single);
    c.listing = Some(listing(vec![listed], 1, 1));
    let out = render_str("x", c).unwrap();
    for k in &expect {
        assert!(out.deps.contains(k), "missing {k}: {:?}", out.deps);
    }
    assert_eq!(out.deps.iter().count(), expect.len() + 2, "{:?}", out.deps);
}
