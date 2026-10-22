//! Review Focus 4: a hostile theme is stopped and the next render works.

use std::time::{Duration, Instant};

use ferrite_theme::{Theme, ThemeError};

use crate::common::{ctx, render, theme};

const BUDGET: Duration = Duration::from_secs(2);

fn hostile() -> Theme {
    theme(&[
        (
            "templates/loop.html",
            "{% for i in range(100000) %}{% for j in range(100000) %}{{ j }}{% endfor %}{% endfor %}",
        ),
        (
            "templates/silent-loop.html",
            "{% for i in range(100000) %}{% for j in range(100000) %}{% endfor %}{% endfor %}",
        ),
        (
            "templates/huge-range.html",
            "{% for i in range(10**9) %}{{ i }}{% endfor %}",
        ),
        (
            "templates/recurse.html",
            "{% macro r(n) %}{{ r(n + 1) }}{% endmacro %}{{ r(0) }}",
        ),
        (
            "templates/self-include.html",
            "{% include \"self-include.html\" %}",
        ),
        (
            "templates/big.html",
            "{% for i in range(11) %}{{ 'x' * 1000000 }}{% endfor %}",
        ),
        ("templates/repeat.html", "{{ 'x' * 1000000000 }}"),
        ("templates/ok.html", "fine {{ site.title }}"),
    ])
}

fn assert_limit(t: &Theme, name: &str) {
    let start = Instant::now();
    let err = render(t, name, ctx()).unwrap_err();
    let took = start.elapsed();
    assert!(matches!(err, ThemeError::Limit(_)), "{name}: {err}");
    assert!(took < BUDGET, "{name} took {took:?}");
    // The engine is unaffected: the next render succeeds.
    assert_eq!(
        render(t, "ok.html", ctx()).unwrap().html,
        "fine My &lt;Site&gt;"
    );
}

#[test]
fn infinite_loop_runs_out_of_fuel() {
    let t = hostile();
    assert_limit(&t, "loop.html");
    assert_limit(&t, "silent-loop.html");
    let err = render(&t, "loop.html", ctx()).unwrap_err();
    assert!(err.to_string().contains("fuel"), "{err}");
}

#[test]
fn huge_range_is_a_limit() {
    assert_limit(&hostile(), "huge-range.html");
}

#[test]
fn unbounded_recursion_is_a_limit() {
    let t = hostile();
    assert_limit(&t, "recurse.html");
    assert_limit(&t, "self-include.html");
}

#[test]
fn ten_megabytes_of_output_is_a_limit() {
    let t = hostile();
    assert_limit(&t, "big.html");
    let err = render(&t, "big.html", ctx()).unwrap_err();
    assert!(
        err.to_string().contains("output exceeds 8388608 bytes"),
        "{err}"
    );
    assert_limit(&t, "repeat.html");
}
