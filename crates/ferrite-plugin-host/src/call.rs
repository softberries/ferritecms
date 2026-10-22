//! One bounded plugin call: fresh store, limits, instantiate, invoke, classify.

use std::fmt;
use std::future::Future;
use std::time::Duration;

use wasmtime::{Store, Trap};

use crate::bindings::v0_1::Plugin;
use crate::bindings::v0_1::ferrite::plugin::types::PluginError as WitPluginError;
use crate::failure::FailureKind;
use crate::output::sanitize_guest_text;
use crate::registry::LoadedPlugin;
use crate::state::{HostState, MAX_RESOURCES};
use crate::types::{ActionEvent, FilterValue, RequestInfo, RouteRequest, RouteResponse};

/// Fuel consumed between cooperative yields to the async runtime. Design 05 §6 says
/// 10_000; measured, that makes a CPU-bound call ~3.5x slower (50 M fuel: ~110 ms vs
/// ~38 ms at 100_000, debug build), so the spike uses 100_000 (≈ tens of µs of guest work).
const FUEL_YIELD_INTERVAL: u64 = 100_000;

/// Why a plugin call produced no usable result.
#[derive(Debug)]
pub(crate) enum CallFailure {
    /// Guest trapped: panic/unreachable, out of fuel, epoch deadline, memory fault.
    Trap(Trap),
    /// The guest trapped after the host refused a memory or table grow.
    Memory,
    /// Hard deadline (`deadline × 1.5`) elapsed, e.g. inside a slow host import.
    Timeout,
    /// The instance could not be created. Text is capped and escaped.
    Instantiate(String),
    /// The guest returned `plugin-error`. Text is capped and escaped.
    GuestError(String),
    /// The call failed without a trap (host-side error during the call). Text is capped
    /// and escaped: wasmtime's error chain can carry guest name-section frame names.
    Invalid(String),
}

impl fmt::Display for CallFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Trap(t) => write!(f, "trap: {t}"),
            Self::Memory => f.write_str("memory limit exceeded"),
            Self::Timeout => f.write_str("hard deadline exceeded"),
            Self::Instantiate(e) => write!(f, "instantiation failed: {e}"),
            Self::GuestError(e) => write!(f, "plugin error: {e}"),
            Self::Invalid(e) => write!(f, "call failed: {e}"),
        }
    }
}

impl CallFailure {
    pub(crate) fn kind(&self) -> FailureKind {
        match self {
            Self::Trap(Trap::OutOfFuel) => FailureKind::OutOfFuel,
            Self::Trap(Trap::Interrupt) | Self::Timeout => FailureKind::Deadline,
            Self::Trap(_) | Self::Invalid(_) => FailureKind::Trap,
            Self::Memory => FailureKind::Memory,
            Self::Instantiate(_) => FailureKind::Instantiate,
            Self::GuestError(_) => FailureKind::GuestError,
        }
    }
}

fn guest_error(e: WitPluginError) -> CallFailure {
    let (prefix, message) = match &e {
        WitPluginError::InvalidInput(m) => ("invalid input: ", m),
        WitPluginError::Unavailable(m) => ("unavailable: ", m),
        WitPluginError::Other(m) => ("", m),
    };
    CallFailure::GuestError(format!("{prefix}{}", sanitize_guest_text(message)))
}

/// Classify an error from the guest call; `store` tells whether a grow was refused.
fn call_error(store: &Store<HostState>, e: wasmtime::Error) -> CallFailure {
    match e.downcast_ref::<Trap>() {
        Some(_) if store.data().limits.grow_denied() => CallFailure::Memory,
        Some(t) => CallFailure::Trap(*t),
        None => CallFailure::Invalid(sanitize_guest_text(&format!("{e:#}"))),
    }
}

fn instantiate_error(e: wasmtime::Error) -> CallFailure {
    match e.downcast_ref::<Trap>() {
        Some(t) => CallFailure::Trap(*t),
        None => CallFailure::Instantiate(sanitize_guest_text(&format!("{e:#}"))),
    }
}

/// `ceil(deadline / tick)`, at least 1.
fn deadline_ticks(deadline: Duration, tick: Duration) -> u64 {
    let ticks = deadline.as_nanos().div_ceil(tick.as_nanos().max(1)).max(1);
    u64::try_from(ticks).unwrap_or(u64::MAX)
}

/// Fresh store with memory/instance limits, fuel and an epoch deadline; then instantiate.
async fn instantiate(
    loaded: &LoadedPlugin,
    deadline: Duration,
) -> Result<(Store<HostState>, Plugin), CallFailure> {
    let state = HostState::new(
        loaded.name.clone(),
        loaded.grants.clone(),
        loaded.services.clone(),
        loaded.memory_bytes,
    );
    let mut store = Store::new(loaded.pre.engine(), state);
    store.limiter(|s| &mut s.limits);
    store.set_max_component_handles(MAX_RESOURCES);
    store.set_fuel(loaded.fuel).map_err(instantiate_error)?;
    store
        .fuel_async_yield_interval(Some(FUEL_YIELD_INTERVAL))
        .map_err(instantiate_error)?;
    store.epoch_deadline_trap();
    store.set_epoch_deadline(deadline_ticks(deadline, loaded.epoch_tick));
    let plugin = loaded
        .pre
        .instantiate_async(&mut store)
        .await
        .map_err(instantiate_error)?;
    Ok((store, plugin))
}

/// Run `call` under the hard deadline (`deadline × 1.5`), the backstop for time spent
/// in host imports, which epochs do not interrupt.
async fn bounded<T>(
    deadline: Duration,
    call: impl Future<Output = Result<T, CallFailure>>,
) -> Result<T, CallFailure> {
    let hard = deadline.saturating_mul(3) / 2;
    tokio::time::timeout(hard, call)
        .await
        .unwrap_or(Err(CallFailure::Timeout))
}

pub(crate) async fn call_filter(
    loaded: &LoadedPlugin,
    hook: &str,
    value: &FilterValue,
    req: &RequestInfo,
    deadline: Duration,
) -> Result<FilterValue, CallFailure> {
    let value = value.clone().into();
    let req = req.clone().into();
    bounded(deadline, async {
        let (mut store, plugin) = instantiate(loaded, deadline).await?;
        plugin
            .ferrite_plugin_guest()
            .call_apply_filter(&mut store, hook, &value, &req)
            .await
            .map_err(|e| call_error(&store, e))?
            .map(Into::into)
            .map_err(guest_error)
    })
    .await
}

pub(crate) async fn call_action(
    loaded: &LoadedPlugin,
    hook: &str,
    event: ActionEvent,
    deadline: Duration,
) -> Result<(), CallFailure> {
    let event = event.into();
    bounded(deadline, async {
        let (mut store, plugin) = instantiate(loaded, deadline).await?;
        plugin
            .ferrite_plugin_guest()
            .call_on_action(&mut store, hook, &event)
            .await
            .map_err(|e| call_error(&store, e))?
            .map_err(guest_error)
    })
    .await
}

pub(crate) async fn call_route(
    loaded: &LoadedPlugin,
    req: RouteRequest,
    deadline: Duration,
) -> Result<RouteResponse, CallFailure> {
    let req = req.into();
    bounded(deadline, async {
        let (mut store, plugin) = instantiate(loaded, deadline).await?;
        plugin
            .ferrite_plugin_guest()
            .call_handle_request(&mut store, &req)
            .await
            .map_err(|e| call_error(&store, e))?
            .map(Into::into)
            .map_err(guest_error)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_are_classified() {
        assert_eq!(
            CallFailure::Trap(Trap::OutOfFuel).kind(),
            FailureKind::OutOfFuel
        );
        assert_eq!(
            CallFailure::Trap(Trap::Interrupt).kind(),
            FailureKind::Deadline
        );
        assert_eq!(CallFailure::Timeout.kind(), FailureKind::Deadline);
        assert_eq!(
            CallFailure::Trap(Trap::UnreachableCodeReached).kind(),
            FailureKind::Trap
        );
        assert_eq!(CallFailure::Memory.kind(), FailureKind::Memory);
    }

    #[test]
    fn guest_error_text_is_capped_and_escaped() {
        let huge = "x\n\u{1b}".repeat(512 * 1024);
        for e in [
            WitPluginError::Other(huge.clone()),
            WitPluginError::InvalidInput(huge.clone()),
            WitPluginError::Unavailable(huge),
        ] {
            let shown = guest_error(e).to_string();
            // 1 KiB kept, each newline/ESC escaped to at most 6 chars, plus a prefix.
            assert!(shown.len() < 8 * 1024, "{} bytes", shown.len());
            assert!(!shown.contains(['\n', '\u{1b}']), "unescaped control char");
            assert!(shown.ends_with("…[truncated]"));
        }
    }

    #[test]
    fn ticks_round_up() {
        let ms = Duration::from_millis;
        assert_eq!(deadline_ticks(ms(50), ms(5)), 10);
        assert_eq!(deadline_ticks(ms(51), ms(5)), 11);
        assert_eq!(deadline_ticks(ms(1), ms(5)), 1);
        assert_eq!(deadline_ticks(Duration::ZERO, ms(5)), 1);
    }
}
