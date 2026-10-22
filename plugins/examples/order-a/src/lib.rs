//! Test fixture for hook ordering: appends `<meta name="order" content="order-a">`.
use ferrite_sdk::{Ctx, Head, Result};

fn head(mut head: Head, _ctx: &Ctx) -> Result<Head> {
    head.meta("order", "order-a");
    Ok(head)
}

ferrite_sdk::export_plugin! {
    filters: { "render.head" => head },
}
