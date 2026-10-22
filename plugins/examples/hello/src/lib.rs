use ferrite_sdk::{Ctx, Head, Level, Result};

fn head(mut head: Head, _ctx: &Ctx) -> Result<Head> {
    ferrite_sdk::log(Level::Debug, "hello: render.head");
    head.meta("generator", "ferrite-hello");
    Ok(head)
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
}
