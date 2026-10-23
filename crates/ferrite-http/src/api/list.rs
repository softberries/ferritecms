//! List responses (P12): each resource has its own list DTO,
//! `#[derive(Serialize, ToSchema)] struct XList { items: Vec<XDto>, next_cursor: Option<String> }`,
//! so the OpenAPI document names every item type. There is no generic envelope; this
//! module only converts a store [`Page`] into the two fields.

use ferrite_domain::Page;

/// The `items` (each converted with `f`) and `next_cursor` of `page`.
pub fn items<T, D>(page: Page<T>, f: impl FnMut(T) -> D) -> (Vec<D>, Option<String>) {
    (page.items.into_iter().map(f).collect(), page.next_cursor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_items_and_keeps_the_cursor() {
        let page = Page {
            items: vec![1, 2],
            next_cursor: Some("c".to_owned()),
        };
        assert_eq!(
            items(page, |n| n * 10),
            (vec![10, 20], Some("c".to_owned()))
        );
    }
}
