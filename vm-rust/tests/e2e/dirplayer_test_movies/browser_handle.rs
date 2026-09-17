#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

#[cfg(target_arch = "wasm32")]
use vm_rust::BrowserPlayerHandle;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(module = "dirplayer-js-api")]
extern "C" {
    #[wasm_bindgen(js_name = "__testRegisterBrowserHandleCallback")]
    fn register_browser_handle_callback(owner_key: &str, callback: &js_sys::Function);
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
    let first_mutated_during_callback = Rc::new(Cell::new(false));
    let first_callback_error = Rc::new(RefCell::new(None::<String>));
    let second_callback_error = Rc::new(RefCell::new(None::<String>));

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
    let first_callback = Closure::wrap(Box::new({
        let events = first_events.clone();
        let reentered = first_reentered.clone();
        let callback_error = first_callback_error.clone();
        let mutated_during_callback = first_mutated_during_callback.clone();
        let expected_owner = first_expected_owner.clone();
        let handle = first.clone();
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
            if kind.as_string().as_deref() == Some("channel:7")
                && !mutated_during_callback.replace(true)
            {
                if let Err(error) = with_handle(&handle, |handle| {
                    // This producer runs reentrantly from the callback. The
                    // nested drain must leave channel:8 queued for the next
                    // owner-bound scheduler boundary.
                    handle.set_debug_selected_channel(8)
                }) {
                    *callback_error.borrow_mut() = Some(format!(
                        "callback producer reentry failed: {error:?}"
                    ));
                }
            }
            match with_handle(&handle, |handle| handle.preview_state()) {
                Ok(_) => reentered.set(true),
                Err(error) => {
                    *callback_error.borrow_mut() =
                        Some(format!("callback reentry failed: {error:?}"))
                }
            }
        }
    }) as Box<dyn FnMut(JsValue, JsValue, JsValue)>);
    let second_callback = Closure::wrap(Box::new({
        let events = second_events.clone();
        let reentered = second_reentered.clone();
        let callback_error = second_callback_error.clone();
        let expected_owner = second_expected_owner.clone();
        let handle = second.clone();
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
            match with_handle(&handle, |handle| handle.preview_state()) {
                Ok(_) => reentered.set(true),
                Err(error) => {
                    *callback_error.borrow_mut() =
                        Some(format!("callback reentry failed: {error:?}"))
                }
            }
        }
    }) as Box<dyn FnMut(JsValue, JsValue, JsValue)>);
    register_browser_handle_callback(
        &first_initial_owner,
        first_callback.as_ref().unchecked_ref(),
    );
    register_browser_handle_callback(
        &second_initial_owner,
        second_callback.as_ref().unchecked_ref(),
    );

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
    let first_events_after_first = first_events.borrow().len();
    require(
        first_events_after_first > first_events_before,
        "first owner callback saw no first-player events",
    )?;
    let first_events_before_next_boundary = first_events_after_first;
    with_handle(&first, |handle| handle.set_debug_selected_channel(6))?;
    require(
        first_events.borrow().iter().any(|kind| kind == "channel:8"),
        "reentrant notification was not delivered at the next boundary",
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
    require(
        first_events.borrow().len() == first_events_before_second,
        "second-player events crossed to first owner",
    )?;
    require(
        second_events.borrow().len() > second_events_before_second,
        "second owner callback saw no second-player events",
    )?;

    throw_next_browser_handle_callback(&second_initial_owner);
    require(
        with_handle(&second, |handle| handle.set_debug_selected_channel(12)).is_err(),
        "throwing notification callback was not reported",
    )?;
    let second_events_before_after_throw = second_events.borrow().len();
    with_handle(&second, |handle| handle.set_debug_selected_channel(13))?;
    require(
        second_events.borrow().len() > second_events_before_after_throw,
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
    require(
        first_reset_owner != first_initial_owner,
        "first reset did not rotate its owner",
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
    register_browser_handle_callback(&first_reset_owner, first_callback.as_ref().unchecked_ref());
    let first_events_before_reset = first_events.borrow().len();
    let second_events_before_first_reset = second_events.borrow().len();
    with_handle(&first, |handle| handle.subscribe_to_score())?;
    with_handle(&first, |handle| handle.subscribe_to_channel_names())?;
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

    unregister_browser_handle_callback(&first_reset_owner);
    drop(first_callback);
    drop(first.borrow_mut().take());
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
    unregister_browser_handle_callback(&second_owner_before_first_reset);
    drop(second_callback);
    Ok(())
}
