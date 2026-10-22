//! Hostile: answers a `head` filter with a `text` value.
use ferrite_sdk::{
    ActionEvent, FilterValue, Guest, PluginError, RequestInfo, RouteRequest, RouteResponse,
};

struct WrongVariant;

impl Guest for WrongVariant {
    fn apply_filter(
        _hook: String,
        _value: FilterValue,
        _req: RequestInfo,
    ) -> Result<FilterValue, PluginError> {
        Ok(FilterValue::Text("not a head".into()))
    }

    fn on_action(_hook: String, _event: ActionEvent) -> Result<(), PluginError> {
        Ok(())
    }

    fn handle_request(_req: RouteRequest) -> Result<RouteResponse, PluginError> {
        Err(PluginError::Other("no routes".into()))
    }
}

ferrite_sdk::bindings::export!(WrongVariant);
