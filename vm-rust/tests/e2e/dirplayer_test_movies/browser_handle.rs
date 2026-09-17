#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures::JsFuture;

#[cfg(target_arch = "wasm32")]
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[cfg(target_arch = "wasm32")]
use vm_rust::{test_dispatch_host_events, BrowserPlayerHandle};

#[cfg(target_arch = "wasm32")]
use vm_rust::player::host_events::HostEvent;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(module = "dirplayer-js-api")]
extern "C" {
    #[wasm_bindgen(js_name = "__testRegisterBrowserHandleCallback")]
    fn register_browser_handle_callback(
        owner_key: &str,
        callback: &js_sys::Function,
        rebind_handle: JsValue,
        rebind_on_channel: bool,
    );
    #[wasm_bindgen(js_name = "__testThrowNextBrowserHandleCallback")]
    fn throw_next_browser_handle_callback(owner_key: &str);
    #[wasm_bindgen(js_name = "__testUnregisterBrowserHandleCallback")]
    fn unregister_browser_handle_callback(owner_key: &str);
}

#[cfg(target_arch = "wasm32")]
fn with_handle<R>(
    slot: &Rc<RefCell<Option<BrowserPlayerHandle>>>,
    f: impl FnOnce(&BrowserPlayerHandle) -> Result<R, JsValue>,
) -> Result<R, JsValue> {
    let handle = slot
        .try_borrow()
        .map_err(|_| JsValue::from_str("browser handle slot was already borrowed"))?;
    f(handle
        .as_ref()
        .ok_or_else(|| JsValue::from_str("browser handle was disposed"))?)
}

#[cfg(target_arch = "wasm32")]
fn with_handle_mut<R>(
    slot: &Rc<RefCell<Option<BrowserPlayerHandle>>>,
    f: impl FnOnce(&mut BrowserPlayerHandle) -> Result<R, JsValue>,
) -> Result<R, JsValue> {
    let mut handle = slot
        .try_borrow_mut()
        .map_err(|_| JsValue::from_str("browser handle slot was already borrowed"))?;
    f(handle
        .as_mut()
        .ok_or_else(|| JsValue::from_str("browser handle was disposed"))?)
}

#[cfg(target_arch = "wasm32")]
fn stage_size(handle: &BrowserPlayerHandle) -> Result<(u32, u32), JsValue> {
    let values = handle.stage_size()?;
    let width = values
        .get(0)
        .as_f64()
        .ok_or_else(|| JsValue::from_str("stage width was not numeric"))? as u32;
    let height = values
        .get(1)
        .as_f64()
        .ok_or_else(|| JsValue::from_str("stage height was not numeric"))? as u32;
    Ok((width, height))
}

#[cfg(target_arch = "wasm32")]
fn require(condition: bool, message: &str) -> Result<(), JsValue> {
    condition
        .then_some(())
        .ok_or_else(|| JsValue::from_str(message))
}

#[cfg(target_arch = "wasm32")]
async fn wait_for_host_event_turn() -> Result<(), JsValue> {
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        web_sys::window()
            .expect("browser window is required")
            .set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0)
            .expect("host event turn timer should be installable");
    });
    JsFuture::from(promise).await.map(|_| ())
}

#[cfg(target_arch = "wasm32")]
async fn wait_for_host_event_recorded(
    events: &Rc<RefCell<Vec<String>>>,
    expected: &str,
) -> Result<(), JsValue> {
    for _ in 0..8 {
        if events.borrow().iter().any(|event| event == expected) {
            return Ok(());
        }
        wait_for_host_event_turn().await?;
    }
    Err(JsValue::from_str(&format!(
        "timed out waiting for host event {expected}"
    )))
}

#[cfg(target_arch = "wasm32")]
async fn wait_for_callback_event_since(
    events: &Rc<RefCell<Vec<String>>>,
    start: usize,
    expected: &str,
) -> Result<(), JsValue> {
    for _ in 0..8 {
        if events.borrow()[start..]
            .iter()
            .any(|event| event == expected)
        {
            return Ok(());
        }
        wait_for_host_event_turn().await?;
    }
    Err(JsValue::from_str(&format!(
        "timed out waiting for callback event {expected} after offset {start}"
    )))
}

#[cfg(target_arch = "wasm32")]
async fn wait_for_js_host_event(events: &js_sys::Array, expected: &str) -> Result<(), JsValue> {
    for _ in 0..8 {
        if (0..events.length()).any(|index| {
            events
                .get(index)
                .as_string()
                .is_some_and(|event| event == expected)
        }) {
            return Ok(());
        }
        wait_for_host_event_turn().await?;
    }
    Err(JsValue::from_str(&format!(
        "timed out waiting for host event {expected}"
    )))
}

#[cfg(target_arch = "wasm32")]
fn state_number(state: &JsValue, name: &str) -> Result<Option<f64>, JsValue> {
    Ok(js_sys::Reflect::get(state, &JsValue::from_str(name))?.as_f64())
}

#[cfg(target_arch = "wasm32")]
fn state_bool(state: &JsValue, name: &str) -> Result<bool, JsValue> {
    js_sys::Reflect::get(state, &JsValue::from_str(name))?
        .as_bool()
        .ok_or_else(|| JsValue::from_str("browser handle state was not boolean"))
}

#[cfg(target_arch = "wasm32")]
fn preview_member(handle: &BrowserPlayerHandle) -> Result<(i32, i32), JsValue> {
    let state = handle.preview_state()?;
    let member = js_sys::Reflect::get(&state, &JsValue::from_str("member"))?;
    let member = js_sys::Array::from(&member);
    let cast_lib = member
        .get(0)
        .as_f64()
        .ok_or_else(|| JsValue::from_str("preview member cast lib was not numeric"))?;
    let cast_member = member
        .get(1)
        .as_f64()
        .ok_or_else(|| JsValue::from_str("preview member number was not numeric"))?;
    Ok((cast_lib as i32, cast_member as i32))
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub async fn test_browser_player_handle_isolation() -> Result<(), JsValue> {
    let first = Rc::new(RefCell::new(Some(BrowserPlayerHandle::new()?)));
    let second = Rc::new(RefCell::new(Some(BrowserPlayerHandle::new()?)));

    let first_events = Rc::new(RefCell::new(Vec::<String>::new()));
    let second_events = Rc::new(RefCell::new(Vec::<String>::new()));
    let first_reentered = Rc::new(Cell::new(false));
    let second_reentered = Rc::new(Cell::new(false));
    let first_callback_error = Rc::new(RefCell::new(None::<String>));
    let second_callback_error = Rc::new(RefCell::new(None::<String>));
    let first_host_events = Rc::new(RefCell::new(Vec::<String>::new()));
    let second_host_events = Rc::new(RefCell::new(Vec::<String>::new()));
    let first_host_reentered = Rc::new(Cell::new(false));
    let first_order = Rc::new(RefCell::new(Vec::<String>::new()));

    let first_initial_owner = with_handle(&first, |handle| Ok(handle.owner_identity()))?;
    let second_initial_owner = with_handle(&second, |handle| Ok(handle.owner_identity()))?;
    require(
        first_initial_owner != second_initial_owner,
        "browser handles share an owner",
    )?;
    require(
        with_handle(&first, |handle| Ok(handle.player_id_value()))? == 1,
        "unexpected first player id",
    )?;
    require(
        with_handle(&second, |handle| Ok(handle.player_id_value()))? == 1,
        "unexpected second player id",
    )?;

    let first_expected_owner = Rc::new(RefCell::new(first_initial_owner.clone()));
    let second_expected_owner = Rc::new(RefCell::new(second_initial_owner.clone()));
    let first_host_callback = Closure::wrap(Box::new({
        let events = first_host_events.clone();
        let reentered = first_host_reentered.clone();
        let handle = Rc::downgrade(&first);
        let order = first_order.clone();
        move |event: JsValue, owner_key: JsValue| {
            let kind = js_sys::Reflect::get(&event, &JsValue::from_str("type"))
                .ok()
                .and_then(|value| value.as_string())
                .unwrap_or_default();
            let owner = owner_key.as_string().unwrap_or_default();
            events.borrow_mut().push(format!("{kind}:{owner}"));
            order.borrow_mut().push(format!("host:{kind}"));
            let Some(handle) = handle.upgrade() else {
                return;
            };
            if !matches!(kind.as_str(), "ownerBound" | "ownerRetired" | "flashReset")
                && with_handle(&handle, |handle| handle.preview_state()).is_ok()
            {
                reentered.set(true);
            }
        }
    }) as Box<dyn FnMut(JsValue, JsValue)>);
    let first_host_callback = first_host_callback
        .into_js_value()
        .dyn_into::<js_sys::Function>()?;
    let second_host_callback = Closure::wrap(Box::new({
        let events = second_host_events.clone();
        move |event: JsValue, owner_key: JsValue| {
            let kind = js_sys::Reflect::get(&event, &JsValue::from_str("type"))
                .ok()
                .and_then(|value| value.as_string())
                .unwrap_or_default();
            let owner = owner_key.as_string().unwrap_or_default();
            events.borrow_mut().push(format!("{kind}:{owner}"));
        }
    }) as Box<dyn FnMut(JsValue, JsValue)>);
    let second_host_callback = second_host_callback
        .into_js_value()
        .dyn_into::<js_sys::Function>()?;
    with_handle_mut(&first, |handle| {
        handle.set_host_event_sink(first_host_callback.clone())
    })?;
    with_handle_mut(&second, |handle| {
        handle.set_host_event_sink(second_host_callback.clone())
    })?;
    wait_for_host_event_recorded(
        &first_host_events,
        &format!("ownerBound:{first_initial_owner}"),
    )
    .await?;
    wait_for_host_event_recorded(
        &second_host_events,
        &format!("ownerBound:{second_initial_owner}"),
    )
    .await?;
    require(
        first_host_events
            .borrow()
            .iter()
            .any(|event| event == &format!("ownerBound:{first_initial_owner}")),
        "first direct host sink was not bound",
    )?;
    require(
        second_host_events
            .borrow()
            .iter()
            .any(|event| event == &format!("ownerBound:{second_initial_owner}")),
        "second direct host sink was not bound",
    )?;
    let first_callback = Closure::wrap(Box::new({
        let events = first_events.clone();
        let reentered = first_reentered.clone();
        let callback_error = first_callback_error.clone();
        let expected_owner = first_expected_owner.clone();
        let handle = Rc::downgrade(&first);
        let order = first_order.clone();
        move |kind: JsValue, payload: JsValue, owner_key: JsValue| {
            let Some(owner_key) = owner_key.as_string() else {
                *callback_error.borrow_mut() = Some("first callback omitted owner key".into());
                return;
            };
            if owner_key != *expected_owner.borrow() {
                *callback_error.borrow_mut() =
                    Some("first callback received a foreign owner".into());
                return;
            }
            if !payload.is_object() {
                *callback_error.borrow_mut() =
                    Some("first callback payload was not an object".into());
                return;
            }
            events
                .borrow_mut()
                .push(kind.as_string().unwrap_or_default());
            let kind_string = kind.as_string().unwrap_or_default();
            if let Some(host_kind) = kind_string.strip_prefix("host:") {
                order.borrow_mut().push(format!("host:{host_kind}"));
            } else {
                order.borrow_mut().push(format!("player:{kind_string}"));
            }
            let Some(handle) = handle.upgrade() else {
                return;
            };
            match with_handle(&handle, |handle| handle.preview_state()) {
                Ok(_) => reentered.set(true),
                Err(error) => {
                    *callback_error.borrow_mut() =
                        Some(format!("callback reentry failed: {error:?}"))
                }
            }
        }
    }) as Box<dyn FnMut(JsValue, JsValue, JsValue)>);
    let first_callback = first_callback
        .into_js_value()
        .dyn_into::<js_sys::Function>()?;
    let second_callback = Closure::wrap(Box::new({
        let events = second_events.clone();
        let reentered = second_reentered.clone();
        let callback_error = second_callback_error.clone();
        let expected_owner = second_expected_owner.clone();
        let handle = Rc::downgrade(&second);
        move |kind: JsValue, payload: JsValue, owner_key: JsValue| {
            let Some(owner_key) = owner_key.as_string() else {
                *callback_error.borrow_mut() = Some("second callback omitted owner key".into());
                return;
            };
            if owner_key != *expected_owner.borrow() {
                *callback_error.borrow_mut() =
                    Some("second callback received a foreign owner".into());
                return;
            }
            if !payload.is_object() {
                *callback_error.borrow_mut() =
                    Some("second callback payload was not an object".into());
                return;
            }
            events
                .borrow_mut()
                .push(kind.as_string().unwrap_or_default());
            let Some(handle) = handle.upgrade() else {
                return;
            };
            match with_handle(&handle, |handle| handle.preview_state()) {
                Ok(_) => reentered.set(true),
                Err(error) => {
                    *callback_error.borrow_mut() =
                        Some(format!("callback reentry failed: {error:?}"))
                }
            }
        }
    }) as Box<dyn FnMut(JsValue, JsValue, JsValue)>);
    let second_callback = second_callback
        .into_js_value()
        .dyn_into::<js_sys::Function>()?;
    register_browser_handle_callback(
        &first_initial_owner,
        &first_callback,
        JsValue::UNDEFINED,
        false,
    );
    register_browser_handle_callback(
        &second_initial_owner,
        &second_callback,
        JsValue::UNDEFINED,
        false,
    );

    // Rebind and produce content in the same turn.  Lifecycle delivery is
    // detached, so this proves the public producer waits behind the complete
    // retirement/bind sequence instead of relying on an initial event wait.
    let order_start = first_order.borrow().len();
    with_handle_mut(&first, |handle| {
        handle.set_host_event_sink(first_host_callback.clone())
    })?;
    with_handle(&first, |handle| handle.set_debug_selected_channel(5))?;
    for _ in 0..8 {
        if first_order.borrow().len() >= order_start + 3 {
            break;
        }
        wait_for_host_event_turn().await?;
    }
    let immediate_order = first_order.borrow()[order_start..].to_vec();
    require(
        immediate_order
            == ["host:ownerRetired", "host:ownerBound", "player:channel:5"]
                .into_iter()
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>(),
        "public notification bypassed the owner lifecycle fence",
    )?;

    let second_initial_size = with_handle(&second, stage_size)?;
    with_handle(&first, |handle| handle.set_stage_size(320, 200))?;
    require(
        with_handle(&first, stage_size)? == (320, 200),
        "first stage size was not applied",
    )?;
    require(
        with_handle(&second, stage_size)? == second_initial_size,
        "first stage mutation changed the second player",
    )?;

    with_handle(&second, |handle| handle.set_stage_size(640, 480))?;
    require(
        with_handle(&second, stage_size)? == (640, 480),
        "second stage size was not applied",
    )?;
    let second_owner_before_first_reset =
        with_handle(&second, |handle| Ok(handle.owner_identity()))?;

    let first_events_before = first_events.borrow().len();
    let second_events_before = second_events.borrow().len();
    with_handle(&first, |handle| handle.set_preview_member_ref(1, 2))?;
    with_handle(&first, |handle| handle.set_preview_font_size(11))?;
    // The first player has no movie, so both valid-negative and out-of-range
    // channel selections exercise the empty-score guard without panicking.
    with_handle(&first, |handle| handle.set_debug_selected_channel(7))?;
    with_handle(&first, |handle| handle.set_debug_selected_channel(-1))?;
    with_handle(&first, |handle| handle.subscribe_to_score())?;
    with_handle(&first, |handle| handle.subscribe_to_channel_names())?;
    for _ in 0..8 {
        if first_order.borrow().iter().any(|event| event == "player:channel:7")
        {
            break;
        }
        wait_for_host_event_turn().await?;
    }
    require(
        first_order.borrow().iter().any(|event| event == "player:channel:7"),
        "first channel callback was not observed",
    )?;
    let first_events_after_first = first_events.borrow().len();
    require(
        first_events_after_first > first_events_before,
        "first owner callback saw no first-player events",
    )?;
    let first_events_before_next_boundary = first_events_after_first;
    with_handle(&first, |handle| handle.set_debug_selected_channel(6))?;
    for _ in 0..8 {
        if first_events.borrow().iter().any(|kind| kind == "channel:6") {
            break;
        }
        wait_for_host_event_turn().await?;
    }
    require(
        first_events.borrow().iter().any(|kind| kind == "channel:6"),
        "next-boundary notification was not delivered",
    )?;
    require(
        first_events.borrow().len() > first_events_before_next_boundary,
        "next-boundary notification did not reach the original owner",
    )?;
    require(
        second_events.borrow().len() == second_events_before,
        "first-player events crossed to second owner",
    )?;

    let first_events_before_second = first_events.borrow().len();
    let second_events_before_second = second_events.borrow().len();
    with_handle(&second, |handle| handle.set_preview_member_ref(3, 4))?;
    with_handle(&second, |handle| handle.set_preview_font_size(22))?;
    // The second player is also movie-less in this asset-free fixture; this
    // deliberately covers a large positive out-of-range channel as well.
    with_handle(&second, |handle| handle.set_debug_selected_channel(9))?;
    with_handle(&second, |handle| handle.set_debug_selected_channel(999))?;
    with_handle(&second, |handle| handle.subscribe_to_score())?;
    with_handle(&second, |handle| handle.subscribe_to_channel_names())?;
    wait_for_host_event_recorded(&second_events, "channel:9").await?;
    require(
        first_events.borrow().len() == first_events_before_second,
        "second-player events crossed to first owner",
    )?;
    require(
        second_events.borrow().len() > second_events_before_second,
        "second owner callback saw no second-player events",
    )?;

    throw_next_browser_handle_callback(&second_initial_owner);
    let second_events_before_after_throw = second_events.borrow().len();
    with_handle(&second, |handle| handle.set_debug_selected_channel(12))?;
    wait_for_host_event_turn().await?;
    require(
        !second_events.borrow()[second_events_before_after_throw..]
            .iter()
            .any(|event| event == "channel:12"),
        "throwing notification callback was replayed",
    )?;
    with_handle(&second, |handle| handle.set_debug_selected_channel(13))?;
    for _ in 0..8 {
        if second_events.borrow()[second_events_before_after_throw..]
            .iter()
            .any(|event| event == "channel:13")
        {
            break;
        }
        wait_for_host_event_turn().await?;
    }
    require(
        second_events.borrow()[second_events_before_after_throw..]
            .iter()
            .any(|event| event == "channel:13"),
        "notification drain did not recover after callback error",
    )?;

    require(
        first_callback_error.borrow().is_none(),
        "first owner callback failed",
    )?;
    require(
        second_callback_error.borrow().is_none(),
        "second owner callback failed",
    )?;
    require(
        first_reentered.get(),
        "first callback could not reenter its handle",
    )?;
    require(
        second_reentered.get(),
        "second callback could not reenter its handle",
    )?;
    require(
        first_events.borrow().iter().any(|kind| kind == "channel:7"),
        "first channel payload was not routed",
    )?;
    require(
        second_events
            .borrow()
            .iter()
            .any(|kind| kind == "channel:9"),
        "second channel payload was not routed",
    )?;

    let first_preview = with_handle(&first, |handle| handle.preview_state())?;
    require(
        with_handle(&first, preview_member)? == (1, 2),
        "first preview member was not isolated",
    )?;
    require(
        state_number(&first_preview, "fontSize")? == Some(11.0),
        "first preview font size was not isolated",
    )?;
    require(
        state_number(&first_preview, "selectedChannel")? == Some(6.0),
        "first empty-score selected channel was not isolated",
    )?;
    let first_subscriptions = with_handle(&first, |handle| handle.subscription_state())?;
    require(
        state_bool(&first_subscriptions, "score")?,
        "first score subscription missing",
    )?;
    require(
        state_bool(&first_subscriptions, "channelNames")?,
        "first channel subscription missing",
    )?;
    let second_preview = with_handle(&second, |handle| handle.preview_state())?;
    require(
        with_handle(&second, preview_member)? == (3, 4),
        "second preview member was not isolated",
    )?;
    require(
        state_number(&second_preview, "fontSize")? == Some(22.0),
        "second preview font size was not isolated",
    )?;
    require(
        state_number(&second_preview, "selectedChannel")? == Some(13.0),
        "second out-of-range selected channel was not isolated",
    )?;

    with_handle_mut(&first, |handle| handle.reset())?;
    let first_reset_owner = with_handle(&first, |handle| Ok(handle.owner_identity()))?;
    wait_for_host_event_recorded(
        &first_host_events,
        &format!("ownerBound:{first_reset_owner}"),
    )
    .await?;
    require(
        first_reset_owner != first_initial_owner,
        "first reset did not rotate its owner",
    )?;
    let first_host_events_after_reset = first_host_events.borrow().clone();
    let retired_index = first_host_events_after_reset
        .iter()
        .position(|event| event == &format!("ownerRetired:{first_initial_owner}"))
        .ok_or_else(|| JsValue::from_str("first reset did not retire its old host owner"))?;
    let flash_reset_index = first_host_events_after_reset
        .iter()
        .position(|event| event == &format!("flashReset:{first_initial_owner}"))
        .ok_or_else(|| JsValue::from_str("first reset did not emit FlashReset"))?;
    let rebound_index = first_host_events_after_reset
        .iter()
        .position(|event| event == &format!("ownerBound:{first_reset_owner}"))
        .ok_or_else(|| JsValue::from_str("first reset did not bind its new host owner"))?;
    require(
        retired_index < flash_reset_index && flash_reset_index < rebound_index,
        "first reset host lifecycle order was invalid",
    )?;
    require(
        first_host_reentered.get(),
        "first direct host callback could not re-enter its handle",
    )?;
    require(
        with_handle(&second, |handle| Ok(handle.owner_identity()))?
            == second_owner_before_first_reset,
        "first reset changed the second owner",
    )?;
    require(
        with_handle(&second, stage_size)? == (640, 480),
        "first reset changed the second stage size",
    )?;
    require(
        with_handle(&second, preview_member)? == (3, 4),
        "first reset changed second preview member",
    )?;
    let second_after_reset = with_handle(&second, |handle| handle.preview_state())?;
    require(
        state_number(&second_after_reset, "fontSize")? == Some(22.0),
        "first reset changed second preview font size",
    )?;
    require(
        state_number(&second_after_reset, "selectedChannel")? == Some(13.0),
        "first reset changed second selected channel",
    )?;
    let second_subscriptions = with_handle(&second, |handle| handle.subscription_state())?;
    require(
        state_bool(&second_subscriptions, "score")?,
        "first reset changed second score subscription",
    )?;
    require(
        state_bool(&second_subscriptions, "channelNames")?,
        "first reset changed second channel subscription",
    )?;

    unregister_browser_handle_callback(&first_initial_owner);
    *first_expected_owner.borrow_mut() = first_reset_owner.clone();
    register_browser_handle_callback(
        &first_reset_owner,
        &first_callback,
        JsValue::UNDEFINED,
        false,
    );
    let first_events_before_reset = first_events.borrow().len();
    let second_events_before_first_reset = second_events.borrow().len();
    with_handle(&first, |handle| handle.subscribe_to_score())?;
    with_handle(&first, |handle| handle.subscribe_to_channel_names())?;
    wait_for_callback_event_since(&first_events, first_events_before_reset, "score").await?;
    wait_for_callback_event_since(
        &first_events,
        first_events_before_reset,
        "channelNames",
    )
    .await?;
    require(
        first_callback_error.borrow().is_none(),
        "reset owner callback failed",
    )?;
    require(
        first_events.borrow().len() > first_events_before_reset,
        "reset did not emit a new owner snapshot",
    )?;
    require(
        second_events.borrow().len() == second_events_before_first_reset,
        "reset notifications crossed to second owner",
    )?;

    with_handle_mut(&first, |handle| handle.clear_host_event_sink())?;
    wait_for_host_event_recorded(
        &first_host_events,
        &format!("ownerRetired:{first_reset_owner}"),
    )
    .await?;
    require(
        first_host_events
            .borrow()
            .iter()
            .any(|event| event == &format!("ownerRetired:{first_reset_owner}")),
        "clearing the first host sink did not retire its owner",
    )?;
    unregister_browser_handle_callback(&first_reset_owner);
    drop(first.borrow_mut().take());
    wait_for_host_event_turn().await?;
    require(
        with_handle(&second, |handle| Ok(handle.owner_identity()))?
            == second_owner_before_first_reset,
        "disposing first changed the second owner",
    )?;
    require(
        with_handle(&second, stage_size)? == (640, 480),
        "disposing first changed the second stage size",
    )?;
    require(
        with_handle(&second, preview_member)? == (3, 4),
        "disposing first changed second preview member",
    )?;
    let second_events_before_disposed_first = second_events.borrow().len();
    with_handle(&second, |handle| handle.set_debug_selected_channel(11))?;
    wait_for_host_event_recorded(&second_events, "channel:11").await?;
    require(
        second_events.borrow().len() > second_events_before_disposed_first,
        "second owner stopped receiving callbacks after first disposal",
    )?;

    with_handle_mut(&second, |handle| handle.reset())?;
    require(
        with_handle(&second, |handle| Ok(handle.owner_identity()))?
            != second_owner_before_first_reset,
        "second reset did not rotate its owner",
    )?;
    with_handle(&second, |handle| handle.set_stage_size(800, 600))?;
    require(
        with_handle(&second, stage_size)? == (800, 600),
        "second was not usable after reset",
    )?;
    let second_final_owner = with_handle(&second, |handle| Ok(handle.owner_identity()))?;
    unregister_browser_handle_callback(&second_owner_before_first_reset);
    drop(second.borrow_mut().take());
    wait_for_host_event_recorded(
        &second_host_events,
        &format!("ownerRetired:{second_final_owner}"),
    )
    .await?;
    require(
        second_host_events
            .borrow()
            .iter()
            .any(|event| event == &format!("ownerRetired:{second_final_owner}")),
        "dropping the second handle did not retire its owner",
    )?;
    Ok(())
}

/// Exercise the real detached BrowserPlayerHandle host drain.  The first
/// callback queues a reentrant D event and throws after A; the unattempted
/// B/C tail must be restored ahead of D and delivered exactly once after a
/// replacement sink is bound.  The callback also re-enters the handle while
/// the detached drain is running, proving the session/handle borrow has ended.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub async fn test_browser_player_host_tail_preservation() -> Result<(), JsValue> {
    let handle = Rc::new(RefCell::new(Some(BrowserPlayerHandle::new()?)));
    let first_events = js_sys::Array::new();
    let reentered = Rc::new(Cell::new(false));
    let queue_callback = Closure::wrap(Box::new({
        let handle = Rc::downgrade(&handle);
        move || -> Result<(), JsValue> {
            let handle = handle
                .upgrade()
                .ok_or_else(|| JsValue::from_str("tail fixture handle was disposed"));
            let Ok(handle) = handle else {
                return Ok(());
            };
            with_handle(&handle, |handle| {
                handle.test_queue_host_events(vec![HostEvent::FrameChanged { frame: 4 }])
            })
        }
    }) as Box<dyn FnMut() -> Result<(), JsValue>>);
    let queue_callback = queue_callback
        .into_js_value()
        .dyn_into::<js_sys::Function>()?;
    let clear_callback = Closure::wrap(Box::new({
        let handle = Rc::downgrade(&handle);
        move || -> Result<(), JsValue> {
            let handle = handle
                .upgrade()
                .ok_or_else(|| JsValue::from_str("tail fixture handle was disposed"));
            let Ok(handle) = handle else {
                return Ok(());
            };
            with_handle_mut(&handle, |handle| handle.clear_host_event_sink())
        }
    }) as Box<dyn FnMut() -> Result<(), JsValue>>);
    let clear_callback = clear_callback
        .into_js_value()
        .dyn_into::<js_sys::Function>()?;
    let reenter_callback = Closure::wrap(Box::new({
        let handle = Rc::downgrade(&handle);
        let reentered = reentered.clone();
        move || -> Result<(), JsValue> {
            let handle = handle
                .upgrade()
                .ok_or_else(|| JsValue::from_str("tail fixture handle was disposed"));
            let Ok(handle) = handle else {
                return Ok(());
            };
            with_handle(&handle, |handle| handle.preview_state()).map(|_| {
                reentered.set(true);
            })
        }
    }) as Box<dyn FnMut() -> Result<(), JsValue>>);
    let reenter_callback = reenter_callback
        .into_js_value()
        .dyn_into::<js_sys::Function>()?;
    // Use a real JavaScript Function as the sink. Its clear() call is a
    // distinct Rust closure, so OwnerRetired can synchronously re-enter this
    // plain JS wrapper without recursively borrowing a Rust FnMut sink.
    let sink_factory = js_sys::Function::new_with_args(
        "events,reenter,queue,clear",
        "return function(event, owner) {\
            events.push(String(event.type));\
            if (event.type === 'stageSizeChanged') {\
                reenter();\
                queue();\
                clear();\
                throw new Error('intentional host callback failure');\
            }\
        };",
    );
    let first_sink = sink_factory
        .call4(
            &JsValue::UNDEFINED,
            &first_events,
            &reenter_callback,
            &queue_callback,
            &clear_callback,
        )?
        .dyn_into::<js_sys::Function>()?;

    with_handle_mut(&handle, |handle| {
        handle.set_host_event_sink(first_sink.clone())
    })?;
    wait_for_js_host_event(&first_events, "ownerBound").await?;
    with_handle(&handle, |handle| {
        handle.test_queue_host_events(vec![
            HostEvent::StageSizeChanged {
                width: 320,
                height: 200,
                center: false,
            },
            HostEvent::MovieLoaded {
                version: 1,
                cast_names: vec!["B".to_owned()],
            },
            HostEvent::MovieLoadFailed {
                path: "C".to_owned(),
                error: "expected".to_owned(),
            },
        ])
    })?;

    let context = with_handle(&handle, |handle| Ok(handle.test_notification_context()))?;
    require(
        test_dispatch_host_events(context).is_err(),
        "throwing host callback was not surfaced",
    )?;
    require(
        reentered.get(),
        "detached callback could not re-enter the handle",
    )?;
    let first_types: Vec<String> = (0..first_events.length())
        .filter_map(|index| first_events.get(index).as_string())
        .collect();
    require(
        first_types
            .iter()
            .filter(|kind| kind.as_str() == "stageSizeChanged")
            .count()
            == 1,
        "the attempted A event was replayed",
    )?;

    let replacement_events = js_sys::Array::new();
    let replacement_factory = js_sys::Function::new_with_args(
        "events",
        "return function(event, owner) { events.push(String(event.type)); };",
    );
    let replacement_sink = replacement_factory
        .call1(&JsValue::UNDEFINED, &replacement_events)?
        .dyn_into::<js_sys::Function>()?;
    with_handle_mut(&handle, |handle| {
        handle.set_host_event_sink(replacement_sink)
    })?;
    wait_for_js_host_event(&replacement_events, "ownerBound").await?;
    let context = with_handle(&handle, |handle| Ok(handle.test_notification_context()))?;
    test_dispatch_host_events(context)?;
    let expected_tail = [
        "ownerBound",
        "movieLoaded",
        "movieLoadFailed",
        "frameChanged",
    ];
    let replacement_types: Vec<String> = (0..replacement_events.length())
        .filter_map(|index| replacement_events.get(index).as_string())
        .collect();
    require(
        replacement_types
            == expected_tail
                .into_iter()
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>(),
        "replacement sink did not receive retained B/C/D in order",
    )?;

    // Keep the JS sink and its capability closures alive through Drop: the
    // handle emits OwnerRetired during teardown.
    drop(handle.borrow_mut().take());
    wait_for_host_event_turn().await?;
    Ok(())
}
