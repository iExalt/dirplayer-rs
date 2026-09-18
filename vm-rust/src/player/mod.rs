/// Case-insensitive match for Lingo property lookups.
/// Lingo is case-insensitive, so `the markerlist` and `the markerList` must both work.
macro_rules! match_ci {
    ($val:expr, { $($($pat:literal)|+ => $body:expr),*, _ => $default:expr $(,)? }) => {
        $(if $( $val.eq_ignore_ascii_case($pat) )||+ { $body } else)*
        { $default }
    };
}

pub mod allocator;
pub mod bitmap;
pub mod bytecode;
pub mod cast_lib;
pub mod cast_manager;
pub mod cast_member;
pub mod ci_string;
pub mod commands;
pub mod compare;
pub mod compiled;
pub mod console;
pub mod context_vars;
pub mod datum_formatting;
pub mod datum_operations;
pub mod datum_ref;
pub mod debug;
pub mod eval;
pub mod events;
pub mod font;
pub mod geometry;
pub mod gif;
pub mod handlers;
pub mod host_events;
pub mod interp_stats;
pub mod js_lingo;
pub mod js_lingo_loader;
pub mod keyboard;
pub mod keyboard_events;
pub mod keyboard_map;
pub mod mcp;
pub mod movie;
pub mod net_manager;
pub mod net_task;
pub mod nested;
pub mod ownership;
pub mod profiling;
pub mod scope;
pub mod score;
pub mod score_keyframes;
pub mod script;
pub mod script_ref;
pub mod session;
pub mod driver;
pub(crate) mod datum_duplicate;
pub mod sprite;
pub mod stage;
pub mod stream_status;
pub mod symbols;
#[cfg(not(target_arch = "wasm32"))]
pub mod testing;
#[cfg(target_arch = "wasm32")]
pub mod testing_browser;
pub mod testing_shared;
pub mod timeout;
pub(crate) mod value_transfer;
pub mod virtual_scripts;
pub mod xtra;

use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet, VecDeque},
    future::Future,
    pin::Pin,
    rc::Rc,
    sync::{Arc, OnceLock},
    time::Duration,
};

/// Owner-local authority for Flash instance generations. The counter never
/// wraps and the map is replaced on player reset, so a late teardown from an
/// old instance cannot invalidate a replacement on the same sprite number.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FlashBindingOrigin {
    Absent,
    Exact { cast_lib: i32, cast_member: i32 },
    FirstPublished { cast_lib: i32, cast_member: i32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FlashMemberClassification {
    Absent,
    UnresolvedExact,
    ValidFlash,
    InvalidTarget,
}

#[derive(Debug)]
pub(crate) struct FlashBindingState {
    next_generation: u64,
    generations: HashMap<i16, u64>,
    teardown_generations: HashMap<(i16, i32, i32), Vec<u64>>,
    origins: HashMap<i16, FlashBindingOrigin>,
    pending_absent: HashSet<i16>,
}

impl FlashBindingState {
    pub(crate) fn new() -> Self {
        Self {
            next_generation: 0,
            generations: HashMap::new(),
            teardown_generations: HashMap::new(),
            origins: HashMap::new(),
            pending_absent: HashSet::new(),
        }
    }

    fn record_retirement(
        &mut self,
        sprite_num: i16,
        cast_lib: i32,
        cast_member: i32,
        generation: u64,
        teardown: bool,
    ) {
        let key = (sprite_num, cast_lib, cast_member);
        if teardown {
            self.teardown_generations
                .entry(key)
                .or_default()
                .push(generation);
        }
    }

    pub(crate) fn reserve(&mut self, sprite_num: i16) -> Result<u64, ScriptError> {
        if sprite_num <= 0 {
            return Err(ScriptError::new(
                "Flash sprite number must be positive".to_owned(),
            ));
        }
        let generation = self
            .next_generation
            .checked_add(1)
            .ok_or_else(|| ScriptError::new("Flash instance generation exhausted".to_owned()))?;
        if generation > 9_007_199_254_740_991 {
            return Err(ScriptError::new(
                "Flash instance generation exceeds JavaScript safe integer range".to_owned(),
            ));
        }
        if let Some(current) = self.generations.remove(&sprite_num) {
            let retired_pair = match self.origin(sprite_num) {
                FlashBindingOrigin::Exact {
                    cast_lib,
                    cast_member,
                }
                | FlashBindingOrigin::FirstPublished {
                    cast_lib,
                    cast_member,
                } => (cast_lib, cast_member),
                FlashBindingOrigin::Absent => (0, 0),
            };
            self.record_retirement(
                sprite_num,
                retired_pair.0,
                retired_pair.1,
                current,
                matches!(
                    self.origin(sprite_num),
                    FlashBindingOrigin::FirstPublished { .. }
                ),
            );
            self.pending_absent.remove(&sprite_num);
            self.origins.insert(sprite_num, FlashBindingOrigin::Absent);
        }
        self.next_generation = generation;
        self.generations.insert(sprite_num, generation);
        Ok(generation)
    }

    pub(crate) fn reserve_for_pair(
        &mut self,
        sprite_num: i16,
        cast_lib: i32,
        cast_member: i32,
    ) -> Result<u64, ScriptError> {
        if let Some(generation) = self.generations.get(&sprite_num).copied() {
            if self.pending_absent.contains(&sprite_num) {
                // The one legal Absent -> Exact association is performed by
                // transition_member after the score write. A queue/load path
                // arriving before that transition must not adopt an early
                // 0:0 handle's reservation for an arbitrary pair.
                self.invalidate(sprite_num, generation);
            } else {
                let same_origin = matches!(
                    self.origin(sprite_num),
                    FlashBindingOrigin::Exact {
                        cast_lib: current_lib,
                        cast_member: current_member,
                    } | FlashBindingOrigin::FirstPublished {
                        cast_lib: current_lib,
                        cast_member: current_member,
                    } if current_lib == cast_lib && current_member == cast_member
                );
                if same_origin {
                    return Ok(generation);
                }
                self.invalidate(sprite_num, generation);
            }
        }
        let generation = self.reserve(sprite_num)?;
        self.origins.insert(
            sprite_num,
            FlashBindingOrigin::Exact {
                cast_lib,
                cast_member,
            },
        );
        Ok(generation)
    }

    pub(crate) fn reserve_absent(&mut self, sprite_num: i16) -> Result<u64, ScriptError> {
        if let Some(generation) = self.generations.get(&sprite_num).copied() {
            if self.pending_absent.contains(&sprite_num) {
                return Ok(generation);
            }
            self.invalidate(sprite_num, generation);
        }
        let generation = self.reserve(sprite_num)?;
        self.pending_absent.insert(sprite_num);
        self.origins.insert(sprite_num, FlashBindingOrigin::Absent);
        Ok(generation)
    }

    pub(crate) fn invalidate(&mut self, sprite_num: i16, expected: u64) -> bool {
        if self.generations.get(&sprite_num).copied() == Some(expected) {
            self.generations.remove(&sprite_num);
            self.pending_absent.remove(&sprite_num);
            let retired_pair = match self.origin(sprite_num) {
                FlashBindingOrigin::Exact {
                    cast_lib,
                    cast_member,
                }
                | FlashBindingOrigin::FirstPublished {
                    cast_lib,
                    cast_member,
                } => (cast_lib, cast_member),
                FlashBindingOrigin::Absent => (0, 0),
            };
            self.record_retirement(
                sprite_num,
                retired_pair.0,
                retired_pair.1,
                expected,
                matches!(
                    self.origin(sprite_num),
                    FlashBindingOrigin::FirstPublished { .. }
                ),
            );
            self.origins.insert(sprite_num, FlashBindingOrigin::Absent);
            true
        } else {
            false
        }
    }

    pub(crate) fn retire_current_for_pair(
        &mut self,
        sprite_num: i16,
        cast_lib: i32,
        cast_member: i32,
        expected: u64,
    ) -> bool {
        if self.generations.get(&sprite_num).copied() == Some(expected) {
            self.generations.remove(&sprite_num);
            self.pending_absent.remove(&sprite_num);
            let published = matches!(
                self.origin(sprite_num),
                FlashBindingOrigin::FirstPublished { .. }
            );
            self.record_retirement(sprite_num, cast_lib, cast_member, expected, published);
            // Retirement removes the owner-visible binding as one atomic
            // state transition. Callers that establish a replacement origin
            // write it after this helper returns.
            self.origins.insert(sprite_num, FlashBindingOrigin::Absent);
            true
        } else {
            false
        }
    }

    pub(crate) fn retire_failed_publication(
        &mut self,
        sprite_num: i16,
        cast_lib: i32,
        cast_member: i32,
        expected: u64,
    ) -> bool {
        let retired = self.retire_current_for_pair(sprite_num, cast_lib, cast_member, expected);
        if retired {
            self.pending_absent.remove(&sprite_num);
            self.origins.insert(sprite_num, FlashBindingOrigin::Absent);
        }
        // A reentrant replacement may have removed this generation before
        // the failed Load is reported. The host may still have created the
        // old frontend instance, so retain exact teardown authority for this
        // failed publication even when it is no longer current. The caller
        // clears the record after the generation-qualified unload attempt.
        let key = (sprite_num, cast_lib, cast_member);
        let already_marked = self
            .teardown_generations
            .get(&key)
            .is_some_and(|generations| generations.contains(&expected));
        if !already_marked {
            self.teardown_generations
                .entry(key)
                .or_default()
                .push(expected);
        }
        retired
    }

    pub(crate) fn retired_generation_for_pair(
        &self,
        sprite_num: i16,
        cast_lib: i32,
        cast_member: i32,
    ) -> Option<u64> {
        self.teardown_generations
            .get(&(sprite_num, cast_lib, cast_member))
            .and_then(|generations| generations.last().copied())
    }

    pub(crate) fn retired_generations_for_pair(
        &self,
        sprite_num: i16,
        cast_lib: i32,
        cast_member: i32,
    ) -> Vec<u64> {
        self.teardown_generations
            .get(&(sprite_num, cast_lib, cast_member))
            .cloned()
            .unwrap_or_default()
    }

    pub(crate) fn is_retired_generation(
        &self,
        sprite_num: i16,
        cast_lib: i32,
        cast_member: i32,
        generation: u64,
    ) -> bool {
        self.teardown_generations
            .get(&(sprite_num, cast_lib, cast_member))
            .is_some_and(|generations| generations.contains(&generation))
    }

    pub(crate) fn clear_retired_generation(
        &mut self,
        sprite_num: i16,
        cast_lib: i32,
        cast_member: i32,
        expected: u64,
    ) {
        let key = (sprite_num, cast_lib, cast_member);
        if let Some(generations) = self.teardown_generations.get_mut(&key) {
            generations.retain(|generation| *generation != expected);
            if generations.is_empty() {
                self.teardown_generations.remove(&key);
            }
        }
    }

    pub(crate) fn publish_first(
        &mut self,
        sprite_num: i16,
        cast_lib: i32,
        cast_member: i32,
        generation: u64,
    ) -> bool {
        if self.generations.get(&sprite_num).copied() != Some(generation) {
            return false;
        }
        if self.origin(sprite_num)
            == (FlashBindingOrigin::Exact {
                cast_lib,
                cast_member,
            })
        {
            self.origins.insert(
                sprite_num,
                FlashBindingOrigin::FirstPublished {
                    cast_lib,
                    cast_member,
                },
            );
            true
        } else {
            false
        }
    }

    pub(crate) fn origin(&self, sprite_num: i16) -> FlashBindingOrigin {
        self.origins
            .get(&sprite_num)
            .copied()
            .unwrap_or(FlashBindingOrigin::Absent)
    }

    /// Record one score member transition. A first publication is the only
    /// transition that establishes the origin association. Replacements and
    /// clears retire the current generation; their next host load must use the
    /// exact new cast pair rather than inheriting the old origin.
    pub(crate) fn transition_member(
        &mut self,
        sprite_num: i16,
        previous: Option<(i32, i32)>,
        next: Option<(i32, i32)>,
        classification: FlashMemberClassification,
    ) {
        if previous == next {
            return;
        }
        let pair = next;
        let preserves_initial_reservation = previous.is_none()
            && next.is_some()
            && matches!(
                classification,
                FlashMemberClassification::UnresolvedExact | FlashMemberClassification::ValidFlash
            )
            && (self.pending_absent.remove(&sprite_num)
                || matches!(
                    (self.origin(sprite_num), pair),
                    (
                        FlashBindingOrigin::Exact { cast_lib, cast_member },
                        Some((next_lib, next_member))
                    ) if cast_lib == next_lib && cast_member == next_member
                ));
        if !preserves_initial_reservation {
            if let Some(generation) = self.generations.get(&sprite_num).copied() {
                if let Some((cast_lib, cast_member)) = previous {
                    self.retire_current_for_pair(sprite_num, cast_lib, cast_member, generation);
                } else {
                    self.invalidate(sprite_num, generation);
                }
            }
        }
        let origin = match (previous, next, classification) {
            (
                None,
                Some((cast_lib, cast_member)),
                FlashMemberClassification::UnresolvedExact | FlashMemberClassification::ValidFlash,
            ) => FlashBindingOrigin::Exact {
                cast_lib,
                cast_member,
            },
            _ => FlashBindingOrigin::Absent,
        };
        self.origins.insert(sprite_num, origin);
    }

    pub(crate) fn is_current(&self, sprite_num: i16, expected: u64) -> bool {
        self.generations.get(&sprite_num).copied() == Some(expected)
    }
}

/// The owner fence attached to a detached host effect. Owned scheduler paths
/// fill in the session and player so emission can revalidate the exact player
/// before and after crossing into JavaScript. Legacy global paths retain the
/// binding state and owner token, but do not claim a child-session capability.
#[derive(Clone)]
struct FlashActionFence {
    owner: OwnerToken,
    binding: Rc<RefCell<FlashBindingState>>,
    session: Option<RuntimeSessionHandle>,
    player_id: Option<session::PlayerId>,
}

impl FlashActionFence {
    fn legacy(owner: OwnerToken, binding: Rc<RefCell<FlashBindingState>>) -> Self {
        Self {
            owner,
            binding,
            session: None,
            player_id: None,
        }
    }

    fn owned(
        owner: OwnerToken,
        binding: Rc<RefCell<FlashBindingState>>,
        session: RuntimeSessionHandle,
        player_id: session::PlayerId,
    ) -> Self {
        Self {
            owner,
            binding,
            session: Some(session),
            player_id: Some(player_id),
        }
    }

    fn revalidated(
        &self,
        local_sprite: i16,
        generation: Option<u64>,
        cast_pair: Option<(i32, i32)>,
    ) -> Result<(), ScriptError> {
        if !self.owner.is_arena_live() {
            return Err(ScriptError::new_code(
                ScriptErrorCode::InvalidReference,
                "Flash host action owner is stale".to_owned(),
            ));
        }
        if let Some(generation) = generation {
            if !self.binding.borrow().is_current(local_sprite, generation) {
                return Err(ScriptError::new_code(
                    ScriptErrorCode::InvalidReference,
                    "Flash host action generation is stale".to_owned(),
                ));
            }
        }
        if let (Some(session), Some(player_id)) = (&self.session, self.player_id) {
            let mut session = session.try_borrow_mut().map_err(|_| {
                ScriptError::new_code(
                    ScriptErrorCode::InvalidReference,
                    "Flash host action session is already borrowed".to_owned(),
                )
            })?;
            let valid = session
                .with_player(player_id, |context| {
                    self.owner.is_arena_live()
                        && self.owner.same_identity(&context.player.owner)
                        && generation.map_or(true, |generation| {
                            context
                                .player
                                .is_flash_instance_generation_current(local_sprite, generation)
                        })
                        && cast_pair.map_or(true, |(cast_lib, cast_member)| {
                            context
                                .player
                                .movie
                                .score
                                .get_sprite(local_sprite)
                                .and_then(|sprite| sprite.member.as_ref())
                                .is_some_and(|member| {
                                    member.cast_lib == cast_lib && member.cast_member == cast_member
                                })
                        })
                })
                .unwrap_or(false);
            if !valid {
                return Err(ScriptError::new_code(
                    ScriptErrorCode::InvalidReference,
                    "Flash host action owner or generation is stale".to_owned(),
                ));
            }
        }
        Ok(())
    }

    fn bind_owned(&mut self, session: RuntimeSessionHandle, player_id: session::PlayerId) {
        self.session = Some(session);
        self.player_id = Some(player_id);
    }
}

/// Flash host route for a player. `NestedPending` is used between the session
/// child insertion and the frontend owner registration; it prevents a child
/// from publishing actions before its exact owner callback set exists.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FlashHostRoute {
    LocalOwned,
    NestedPending,
    LegacySynthetic,
}

/// A detached Flash host effect prepared while the player is borrowed. The
/// local sprite number remains an i16 for VM/state validation; host routing
/// uses the separate i32 key only after the borrow has ended.
#[derive(Clone)]
pub(crate) enum FlashHostAction {
    Load {
        fence: FlashActionFence,
        host_sprite: i32,
        local_sprite: i16,
        cast_lib: i32,
        cast_member: i32,
        data: Vec<u8>,
        width: u32,
        height: u32,
        paused_at_start: bool,
        asserted_frame: i32,
        generation: u64,
    },
    Unload {
        fence: FlashActionFence,
        host_sprite: i32,
        local_sprite: i16,
        cast_lib: i32,
        cast_member: i32,
        generation: u64,
    },
    Resize {
        fence: FlashActionFence,
        host_sprite: i32,
        local_sprite: i16,
        cast_lib: i32,
        cast_member: i32,
        generation: u64,
        width: u32,
        height: u32,
    },
}

impl FlashHostAction {
    fn owner(&self) -> OwnerToken {
        let fence = match self {
            Self::Load { fence, .. } | Self::Unload { fence, .. } | Self::Resize { fence, .. } => {
                fence
            }
        };
        fence.owner.clone()
    }

    fn bind_owned(&mut self, session: RuntimeSessionHandle, player_id: session::PlayerId) {
        let fence = match self {
            Self::Load { fence, .. } | Self::Unload { fence, .. } | Self::Resize { fence, .. } => {
                fence
            }
        };
        fence.bind_owned(session, player_id);
    }

    fn rollback_after_emit_failure(&self) {
        let (fence, local_sprite, generation, cast_pair) = match self {
            Self::Load {
                fence,
                local_sprite,
                generation,
                cast_lib,
                cast_member,
                ..
            } => (
                fence,
                *local_sprite,
                Some(*generation),
                Some((*cast_lib, *cast_member)),
            ),
            _ => return,
        };
        if let Some(generation) = generation {
            let _ = fence.binding.borrow_mut().retire_failed_publication(
                local_sprite,
                cast_pair.map(|pair| pair.0).unwrap_or_default(),
                cast_pair.map(|pair| pair.1).unwrap_or_default(),
                generation,
            );
            #[cfg(target_arch = "wasm32")]
            if cast_pair.is_some() {
                // A Load can create the old frontend instance before a
                // synchronous reentrant replacement invalidates its fence.
                // Teardown is qualified by that exact retired generation and
                // cannot address the replacement's current generation.
                crate::js_api::JsApi::dispatch_flash_member_unloaded_at_generation(
                    local_sprite as i32,
                    generation,
                    &owner_key_string(&fence.owner),
                );
            }
            if let Some((cast_lib, cast_member)) = cast_pair {
                fence.binding.borrow_mut().clear_retired_generation(
                    local_sprite,
                    cast_lib,
                    cast_member,
                    generation,
                );
            }
            if let (Some(session), Some(player_id)) = (&fence.session, fence.player_id) {
                if let Ok(mut session) = session.try_borrow_mut() {
                    let _ = session.with_player(player_id, |context| {
                        if context
                            .player
                            .flash_instance_generation(local_sprite)
                            .is_none()
                        {
                            if let Some(cast_pair) = cast_pair {
                                context.player.flash_sprite_loaded.remove(&(
                                    local_sprite,
                                    cast_pair.0,
                                    cast_pair.1,
                                ));
                            }
                        }
                    });
                }
            }
        }
    }

    fn emit(&self) -> Result<(), ScriptError> {
        match self {
            Self::Load {
                fence,
                host_sprite,
                local_sprite,
                cast_lib,
                cast_member,
                data,
                width,
                height,
                paused_at_start,
                asserted_frame,
                generation,
            } => {
                fence.revalidated(
                    *local_sprite,
                    Some(*generation),
                    Some((*cast_lib, *cast_member)),
                )?;
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let _ = (
                        host_sprite,
                        cast_lib,
                        cast_member,
                        data,
                        width,
                        height,
                        paused_at_start,
                        asserted_frame,
                    );
                    return Err(ScriptError::new(
                        "Flash host is unavailable on native".to_owned(),
                    ));
                }
                #[cfg(target_arch = "wasm32")]
                {
                    crate::js_api::JsApi::dispatch_flash_member_loaded_prepared(
                        *host_sprite,
                        *cast_lib,
                        *cast_member,
                        data.as_slice(),
                        *width,
                        *height,
                        *paused_at_start,
                        *asserted_frame,
                        &owner_key_string(&fence.owner),
                        *generation,
                    );
                    let result = fence.revalidated(
                        *local_sprite,
                        Some(*generation),
                        Some((*cast_lib, *cast_member)),
                    );
                    if result.is_ok() {
                        let _ = fence.binding.borrow_mut().publish_first(
                            *local_sprite,
                            *cast_lib,
                            *cast_member,
                            *generation,
                        );
                    }
                    result
                }
            }
            Self::Unload {
                fence,
                host_sprite,
                local_sprite,
                cast_lib,
                cast_member,
                generation,
            } => {
                if !fence.binding.borrow().is_retired_generation(
                    *local_sprite,
                    *cast_lib,
                    *cast_member,
                    *generation,
                ) {
                    return Err(ScriptError::new_code(
                        ScriptErrorCode::InvalidReference,
                        "Flash unload generation is not retired for its cast pair".to_owned(),
                    ));
                }
                // Unload is a retirement action: a replacement may already
                // own the same local sprite with a newer generation. The JS
                // route receives only this exact retired generation.
                fence.revalidated(*local_sprite, None, None)?;
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let _ = host_sprite;
                    // Native has no frontend instance to unload. Settle the
                    // exact authority immediately while preserving the
                    // explicit unsupported-host result.
                    fence.binding.borrow_mut().clear_retired_generation(
                        *local_sprite,
                        *cast_lib,
                        *cast_member,
                        *generation,
                    );
                    return Err(ScriptError::new(
                        "Flash host is unavailable on native".to_owned(),
                    ));
                }
                #[cfg(target_arch = "wasm32")]
                crate::js_api::JsApi::dispatch_flash_member_unloaded_at_generation(
                    *host_sprite,
                    *generation,
                    &owner_key_string(&fence.owner),
                );
                let result = fence.revalidated(*local_sprite, None, None);
                if result.is_ok() {
                    fence.binding.borrow_mut().clear_retired_generation(
                        *local_sprite,
                        *cast_lib,
                        *cast_member,
                        *generation,
                    );
                }
                result
            }
            Self::Resize {
                fence,
                host_sprite,
                local_sprite,
                cast_lib,
                cast_member,
                generation,
                width,
                height,
            } => {
                let _ = (cast_lib, cast_member);
                fence.revalidated(
                    *local_sprite,
                    Some(*generation),
                    Some((*cast_lib, *cast_member)),
                )?;
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let _ = (host_sprite, width, height);
                    return Err(ScriptError::new(
                        "Flash host is unavailable on native".to_owned(),
                    ));
                }
                #[cfg(target_arch = "wasm32")]
                crate::js_api::JsApi::dispatch_flash_member_resized(
                    *host_sprite,
                    *generation,
                    *width,
                    *height,
                    &owner_key_string(&fence.owner),
                );
                fence.revalidated(
                    *local_sprite,
                    Some(*generation),
                    Some((*cast_lib, *cast_member)),
                )
            }
        }
    }
}

pub(crate) fn bind_flash_host_actions(
    mut actions: Vec<FlashHostAction>,
    session: RuntimeSessionHandle,
    player_id: session::PlayerId,
) -> Vec<FlashHostAction> {
    for action in &mut actions {
        action.bind_owned(session.clone(), player_id);
    }
    actions
}

pub(crate) fn emit_flash_host_actions(actions: Vec<FlashHostAction>) -> Result<(), ScriptError> {
    for action in actions {
        let owner = action.owner();
        match action.emit() {
            Ok(()) => {}
            Err(error) if error.code == ScriptErrorCode::InvalidReference => {
                action.rollback_after_emit_failure();
                // A callback may synchronously replace the sprite or retire the
                // owner.  The failed action is stale in that case, but the
                // detached tail may still contain a distinct live sprite. Keep
                // draining while the owner remains live; reset cancellation
                // discards the old tail without touching the replacement.
                if owner.is_arena_live() {
                    continue;
                }
                break;
            }
            Err(error) => {
                action.rollback_after_emit_failure();
                return Err(error);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod flash_binding_state_tests {
    use super::FlashBindingState;
    use crate::player::{session::RuntimeSession, symbols::symbol_table::SymbolOwner};
    use async_std::channel;
    use std::rc::Rc;

    #[test]
    fn generations_are_monotonic_and_old_invalidation_cannot_remove_replacement() {
        let mut state = FlashBindingState::new();
        let first = state.reserve(7).expect("first generation");
        let replacement = state.reserve(7).expect("replacement generation");
        assert!(replacement > first);
        assert!(!state.invalidate(7, first));
        assert!(state.is_current(7, replacement));
        assert!(state.invalidate(7, replacement));
        assert!(!state.is_current(7, replacement));
    }

    #[test]
    fn invalid_inputs_and_safe_integer_exhaustion_do_not_mutate_state() {
        let mut state = FlashBindingState::new();
        assert!(state.reserve(0).is_err());
        assert!(state.reserve(-1).is_err());
        assert!(state.generations.is_empty());
        state.next_generation = 9_007_199_254_740_991;
        assert!(state.reserve(2).is_err());
        assert_eq!(state.next_generation, 9_007_199_254_740_991);
        assert!(state.generations.is_empty());
    }

    #[test]
    fn stale_invalidation_cannot_resurrect_a_replacement() {
        let mut old = FlashBindingState::new();
        let generation = old.reserve(3).expect("old generation");
        assert!(old.invalidate(3, generation));
        assert!(!old.is_current(3, generation));
        let replacement = old.reserve(3).expect("replacement generation");
        assert!(replacement > generation);
        assert!(!old.invalidate(3, generation));
        assert!(old.is_current(3, replacement));
    }

    #[test]
    fn member_transition_retire_keeps_old_generation_for_exact_unload() {
        let mut state = FlashBindingState::new();
        state.transition_member(
            1,
            None,
            Some((1, 7)),
            super::FlashMemberClassification::ValidFlash,
        );
        assert_eq!(
            state.origin(1),
            super::FlashBindingOrigin::Exact {
                cast_lib: 1,
                cast_member: 7,
            }
        );
        let first = state.reserve(1).expect("first published generation");
        assert_eq!(
            state.origin(1),
            super::FlashBindingOrigin::Exact {
                cast_lib: 1,
                cast_member: 7,
            }
        );
        assert!(state.publish_first(1, 1, 7, first));
        assert_eq!(
            state.origin(1),
            super::FlashBindingOrigin::FirstPublished {
                cast_lib: 1,
                cast_member: 7,
            }
        );
        state.transition_member(
            1,
            Some((1, 7)),
            Some((1, 8)),
            super::FlashMemberClassification::ValidFlash,
        );
        assert_eq!(state.generations.get(&1).copied(), None);
        assert_eq!(state.retired_generation_for_pair(1, 1, 7), Some(first));
        assert_eq!(state.origin(1), super::FlashBindingOrigin::Absent);
        state.clear_retired_generation(1, 1, 7, first);
        assert_eq!(state.retired_generation_for_pair(1, 1, 7), None);
    }

    #[test]
    fn forged_unload_requires_an_exact_retired_generation() {
        let state = FlashBindingState::new();
        assert!(!state.is_retired_generation(1, 2, 3, 1));
    }

    #[test]
    fn absent_and_exact_reservations_reuse_only_their_pending_binding() {
        let mut state = FlashBindingState::new();
        let absent = state.reserve_absent(1).expect("absent reservation");
        assert_eq!(
            state.reserve_absent(1).expect("repeat absent reservation"),
            absent
        );
        let exact = state
            .reserve_for_pair(1, 2, 3)
            .expect("exact reservation should rotate the unrelated absent binding");
        assert_ne!(exact, absent);
        assert_eq!(
            state
                .reserve_for_pair(1, 2, 3)
                .expect("repeat exact reservation"),
            exact
        );
        state.next_generation = 9_007_199_254_740_991;
        assert!(state.reserve_absent(2).is_err());

        let mut transitioned = FlashBindingState::new();
        let early = transitioned
            .reserve_absent(1)
            .expect("early absent reservation");
        transitioned.transition_member(
            1,
            None,
            Some((4, 5)),
            super::FlashMemberClassification::UnresolvedExact,
        );
        assert_eq!(
            transitioned.reserve_for_pair(1, 4, 5).expect("exact reuse"),
            early
        );
    }

    #[test]
    fn player_reset_replaces_flash_authority_and_retires_old_capability() {
        let mut session = RuntimeSession::new(SymbolOwner {
            session: 91,
            generation: 1,
        });
        assert!(session.add_player(1, channel::unbounded().0));
        let (old_owner, old_state, old_generation) = session
            .with_player(1, |context| {
                let generation = context
                    .player
                    .reserve_flash_instance_generation(3)
                    .expect("old Flash instance generation");
                (
                    context.player.owner.clone(),
                    context.player.flash_binding_state.clone(),
                    generation,
                )
            })
            .expect("player must exist");

        let replacement = session
            .reset_player_owned(1, &old_owner)
            .expect("reset must replace the player owner");
        let new_state = session
            .with_player(1, |context| context.player.flash_binding_state.clone())
            .expect("replacement player must exist");
        assert!(!old_owner.is_arena_live());
        assert!(!Rc::ptr_eq(&old_state, &new_state));
        assert!(old_state.borrow().is_current(3, old_generation));
        assert!(new_state.borrow().generations.is_empty());

        let replacement_generation = new_state
            .borrow_mut()
            .reserve(3)
            .expect("replacement Flash instance generation");
        assert!(old_state.borrow_mut().invalidate(3, replacement_generation));
        assert!(new_state.borrow().is_current(3, replacement_generation));
        assert!(replacement.is_arena_live());
    }

    #[test]
    fn root_player_ids_stay_local_and_legacy_children_use_synthetic_flash_keys() {
        let mut session = RuntimeSession::new(SymbolOwner {
            session: 92,
            generation: 1,
        });
        let (parent_tx, _parent_rx) = channel::unbounded();
        assert!(session.add_player(17, parent_tx));
        let parent_owner = session
            .with_player(17, |context| {
                assert!(!context.player.flash_host_is_nested);
                context.player.owner.clone()
            })
            .expect("nonzero root player must exist");

        let (child_tx, _child_rx) = channel::unbounded();
        let (child_event_tx, _child_event_rx) = channel::unbounded();
        let (child_id, _child_owner) = session
            .register_nested_player(
                17,
                &parent_owner,
                super::cast_lib::CastMemberRef {
                    cast_lib: 1,
                    cast_member: 1,
                },
                child_tx,
                child_event_tx,
            )
            .expect("nested child must register");
        session
            .with_player(child_id, |context| {
                assert!(context.player.flash_host_is_nested);
            })
            .expect("nested child must be present");

        let synthetic = super::nested_flash_key(child_id as usize, 7);
        assert_eq!(super::flash_host_sprite_key(false, 17, 7), 7);
        assert_eq!(
            super::flash_host_sprite_key(true, child_id as usize, 7),
            synthetic
        );
        assert_ne!(synthetic, 7);
        assert_eq!(
            super::decode_nested_flash_key(synthetic),
            Some((child_id as usize, 7))
        );
    }

    #[test]
    fn captured_flash_cast_pair_replacement_is_rejected_before_native_transport() {
        let session = RuntimeSession::new(SymbolOwner {
            session: 93,
            generation: 1,
        })
        .into_handle();
        assert!(session.borrow_mut().add_player(1, channel::unbounded().0));
        let (owner, binding, generation) = session
            .borrow_mut()
            .with_player(1, |context| {
                context.player.movie.score.channels = vec![
                    super::score::SpriteChannel::new(0),
                    super::score::SpriteChannel::new(1),
                ];
                context.player.movie.score.channels[1].sprite.member =
                    Some(super::cast_lib::CastMemberRef {
                        cast_lib: 1,
                        cast_member: 1,
                    });
                let generation = context
                    .player
                    .reserve_flash_instance_generation(1)
                    .expect("generation");
                (
                    context.player.owner.clone(),
                    context.player.flash_binding_state.clone(),
                    generation,
                )
            })
            .expect("fixture player must exist");
        session.borrow_mut().with_player(1, |context| {
            context.player.movie.score.channels[1].sprite.member =
                Some(super::cast_lib::CastMemberRef {
                    cast_lib: 1,
                    cast_member: 2,
                });
        });
        let action = super::FlashHostAction::Load {
            fence: super::FlashActionFence::owned(owner, binding, session.clone(), 1),
            host_sprite: 1,
            local_sprite: 1,
            cast_lib: 1,
            cast_member: 1,
            data: Vec::new(),
            width: 1,
            height: 1,
            paused_at_start: false,
            asserted_frame: -1,
            generation,
        };
        let error = action.emit().expect_err("replaced cast pair must reject");
        assert_eq!(error.code, crate::player::ScriptErrorCode::InvalidReference);
    }

    #[test]
    fn stale_flash_generation_is_rejected_before_native_transport() {
        let session = RuntimeSession::new(SymbolOwner {
            session: 94,
            generation: 1,
        })
        .into_handle();
        assert!(session.borrow_mut().add_player(1, channel::unbounded().0));
        let (owner, binding, generation) = session
            .borrow_mut()
            .with_player(1, |context| {
                context.player.movie.score.channels = vec![
                    super::score::SpriteChannel::new(0),
                    super::score::SpriteChannel::new(1),
                ];
                context.player.movie.score.channels[1].sprite.member =
                    Some(super::cast_lib::CastMemberRef {
                        cast_lib: 1,
                        cast_member: 1,
                    });
                let generation = context
                    .player
                    .reserve_flash_instance_generation(1)
                    .expect("generation");
                (
                    context.player.owner.clone(),
                    context.player.flash_binding_state.clone(),
                    generation,
                )
            })
            .expect("fixture player must exist");
        binding.borrow_mut().invalidate(1, generation);
        let action = super::FlashHostAction::Resize {
            fence: super::FlashActionFence::owned(owner, binding, session, 1),
            host_sprite: 1,
            local_sprite: 1,
            cast_lib: 1,
            cast_member: 1,
            generation,
            width: 2,
            height: 2,
        };
        let error = action.emit().expect_err("stale generation must reject");
        assert_eq!(error.code, crate::player::ScriptErrorCode::InvalidReference);
    }

    #[test]
    fn forged_unload_is_rejected_before_native_transport() {
        let session = RuntimeSession::new(SymbolOwner {
            session: 97,
            generation: 1,
        })
        .into_handle();
        assert!(session.borrow_mut().add_player(1, channel::unbounded().0));
        let (owner, binding) = session
            .borrow_mut()
            .with_player(1, |context| {
                (
                    context.player.owner.clone(),
                    context.player.flash_binding_state.clone(),
                )
            })
            .expect("fixture player must exist");
        let action = super::FlashHostAction::Unload {
            fence: super::FlashActionFence::owned(owner, binding, session, 1),
            host_sprite: 1,
            local_sprite: 1,
            cast_lib: 2,
            cast_member: 3,
            generation: 1,
        };
        let error = action.emit().expect_err("unretired unload must reject");
        assert_eq!(error.code, crate::player::ScriptErrorCode::InvalidReference);
    }

    #[test]
    fn stale_flash_action_does_not_drop_a_live_tail_action() {
        let session = RuntimeSession::new(SymbolOwner {
            session: 95,
            generation: 1,
        })
        .into_handle();
        assert!(session.borrow_mut().add_player(1, channel::unbounded().0));
        let (owner, binding, stale_generation, live_generation) = session
            .borrow_mut()
            .with_player(1, |context| {
                context.player.movie.score.channels = vec![
                    super::score::SpriteChannel::new(0),
                    super::score::SpriteChannel::new(1),
                    super::score::SpriteChannel::new(2),
                ];
                context.player.movie.score.channels[1].sprite.member =
                    Some(super::cast_lib::CastMemberRef {
                        cast_lib: 1,
                        cast_member: 1,
                    });
                context.player.movie.score.channels[2].sprite.member =
                    Some(super::cast_lib::CastMemberRef {
                        cast_lib: 1,
                        cast_member: 2,
                    });
                let stale_generation = context
                    .player
                    .reserve_flash_instance_generation(1)
                    .expect("stale generation");
                let live_generation = context
                    .player
                    .reserve_flash_instance_generation(2)
                    .expect("live generation");
                (
                    context.player.owner.clone(),
                    context.player.flash_binding_state.clone(),
                    stale_generation,
                    live_generation,
                )
            })
            .expect("fixture player must exist");
        binding.borrow_mut().invalidate(1, stale_generation);
        let stale = super::FlashHostAction::Resize {
            fence: super::FlashActionFence::owned(
                owner.clone(),
                binding.clone(),
                session.clone(),
                1,
            ),
            host_sprite: 1,
            local_sprite: 1,
            cast_lib: 1,
            cast_member: 1,
            generation: stale_generation,
            width: 2,
            height: 2,
        };
        let tail = super::FlashHostAction::Resize {
            fence: super::FlashActionFence::owned(owner, binding, session, 1),
            host_sprite: 2,
            local_sprite: 2,
            cast_lib: 1,
            cast_member: 2,
            generation: live_generation,
            width: 2,
            height: 2,
        };
        let error = super::emit_flash_host_actions(vec![stale, tail])
            .expect_err("live tail must reach the explicit native unsupported boundary");
        assert!(
            error
                .message
                .contains("Flash host is unavailable on native")
        );
    }
}

use allocator::{DatumAllocator, DatumAllocatorTrait, ResetableAllocator, ScriptInstanceAllocatorTrait};
use async_recursion::async_recursion;
use futures::future::{select, Either, FutureExt};
use async_std::{
    channel::{self, Receiver, Sender},
    future::{self, timeout},
    sync::Mutex,
    task::spawn_local,
};
use cast_manager::CastPreloadReason;
use cast_member::CastMemberType;
use datum_ref::DatumRef;
use fxhash::FxHashMap;
use handlers::datum_handlers::script_instance::ScriptInstanceUtils;
use indexmap::IndexMap;
use log::{debug, error, warn};
use manual_future::{ManualFuture, ManualFutureCompleter};
use net_manager::NetManager;
use ownership::OwnerToken;
use host_events::{HostEvent, HostEventMailbox, HostEventOverflow};
use driver::{DriverStart, DriverTurn};
use rand::SeedableRng;
use scope::ScopeResult;
use score::{ScoreRef, get_score_sprite_mut};
use script::script_get_prop_opt;
use script_ref::ScriptInstanceRef;
use sprite::Sprite;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::wasm_bindgen;
use xtra::leechprotection::EnvOverrides;
use xtra::manager::XtraManagerState;
use xtra::scene3d::Scene3dStore;

use crate::{
    console_warn,
    director::{
        chunks::handler::{Bytecode, HandlerDef},
        enums::ScriptType,
        file::{DirectorFile, read_director_file_bytes},
        lingo::{
            constants::{get_anim_prop_name, get_anim2_prop_name},
            datum::{Datum, DatumType, VarRef, datum_bool},
        },
    },
    js_api::JsApi,
    player::{
        bytecode::handler_manager::{
            BytecodeHandlerContext, HandlerCode, try_execute_bytecode_sync, try_execute_opcode_sync,
        },
        datum_formatting::format_datum,
        events::{
            dispatch_event_to_all_behaviors, dispatch_system_event_to_timeouts,
            player_dispatch_event_beginsprite, player_invoke_event_to_instances,
            player_invoke_frame_and_movie_scripts, player_invoke_targeted_event,
        },
        geometry::IntRect,
        profiling::{ProfileScope, ProfileScopeOwned, get_profiler_report},
        scope::Scope,
        session::{RuntimeSession, RuntimeSessionHandle},
        symbols::{
            builtin::BuiltInSymbol,
            symbol::{Symbol, SymbolError},
            symbol_table::{SymbolOwner, SymbolTable},
        },
    },
    rendering::with_renderer_mut,
    utils::{get_base_url, get_elapsed_ticks},
};
use url::Url;

use self::{
    bitmap::manager::{BitmapHandle, BitmapId},
    bytecode::handler_manager::StaticBytecodeHandlerManager,
    cast_lib::{CastMemberRef, PlayerNotification, PlayerNotificationKind},
    cast_manager::CastManager,
    commands::{PlayerVMCommand, run_command_loop},
    debug::{Breakpoint, BreakpointContext, BreakpointManager, StepMode},
    events::{
        PlayerVMEvent, player_dispatch_global_event, player_invoke_global_event,
        player_wait_available, run_event_loop,
    },
    font::FontManager,
    handlers::manager::BuiltInHandlerManager,
    keyboard::KeyboardManager,
    movie::Movie,
    net_manager::NetManagerSharedState,
    scope::ScopeRef,
    score::{Score, get_sprite_at},
    script::{Script, ScriptHandlerRef},
    sprite::{ColorRef, CursorRef},
    timeout::TimeoutManager,
};

use crate::player::handlers::datum_handlers::date::DateObject;
use crate::player::handlers::datum_handlers::math::MathObject;
use crate::player::handlers::datum_handlers::player_call_datum_handler;
use crate::player::handlers::datum_handlers::sound_channel::{
    AudioData, SoundChannelDatumHandlers, SoundManager,
};
use crate::player::handlers::datum_handlers::xml::{XmlDocument, XmlNode};
use crate::player::handlers::movie::MovieHandlers;

fn trace_output(player: &mut DirPlayer, message: &str) {
    use crate::js_api::JsApi;

    player.console.write_line(message);
    let trace_log_file = player.movie.trace_log_file.clone();
    if trace_log_file.is_empty() {
        JsApi::dispatch_debug_message(message);
    } else {
        // Append to file via FileIO virtual filesystem
        player.with_xtra_manager_state(|state, _| {
            let entry = state
                .fileio
                .virtual_fs
                .entry(trace_log_file.clone())
                .or_insert_with(Vec::new);
            entry.extend_from_slice(message.as_bytes());
            entry.push(b'\n');
        });
        // Emit file-append event for Electron/local file writing
        dispatch_file_write_event(&trace_log_file, message, &player.owner);
    }
}

pub fn dispatch_file_write_event(file_path: &str, content: &str, owner: &OwnerToken) {
    let window = web_sys::window();
    if let Some(window) = window {
        let event_init = web_sys::CustomEventInit::new();
        let detail = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&detail, &"filePath".into(), &file_path.into());
        let _ = js_sys::Reflect::set(&detail, &"content".into(), &content.into());
        let _ = js_sys::Reflect::set(&detail, &"append".into(), &true.into());
        let key = owner.key();
        let owner_identity = format!("{}:{}:{}", key.session, key.player, key.generation);
        let _ = js_sys::Reflect::set(&detail, &"ownerKey".into(), &owner_identity.into());
        event_init.set_detail(&detail);
        if let Ok(event) =
            web_sys::CustomEvent::new_with_event_init_dict("dirplayer:fileWrite", &event_init)
        {
            let _ = window.dispatch_event(&event);
        }
    }
}

pub fn owner_key_string(owner: &OwnerToken) -> String {
    let key = owner.key();
    format!("{}:{}:{}", key.session, key.player, key.generation)
}

pub enum HandlerExecutionResult {
    Advance,
    Stop,
    Jump,
    Error(ScriptError),
    /// A Lingo-handler call intercepted by the trampoline driver. The call
    /// opcode returns this instead of recursively `await`ing
    /// `player_call_script_handler_raw_args` (which boxes a future per call via
    /// `#[async_recursion]`). The driver pushes a frame onto the explicit scope
    /// stack, advances the caller past the call opcode, and on the callee's
    /// return pushes the return value back (unless `push_return` is false).
    Call(PendingCall),
}

/// A pending Lingo-handler call produced by a call opcode for the trampoline
/// driver (see [`HandlerExecutionResult::Call`]).
pub struct PendingCall {
    pub receiver: Option<ScriptInstanceRef>,
    pub handler_ref: ScriptHandlerRef,
    pub args: Vec<DatumRef>,
    /// Same meaning as `player_call_script_handler_raw_args`'s
    /// `use_raw_arg_list`: when false, `me` is prepended during scope setup.
    pub use_raw_arg_list: bool,
    /// Whether the caller wants the return value pushed onto its stack
    /// (i.e. `!is_no_ret`).
    pub push_return: bool,
}

pub struct HandlerExecutionResultContext {
    pub result: HandlerExecutionResult,
}

pub struct PlayerVMExecutionItem {
    pub command: PlayerVMCommand,
    pub completer: Option<ManualFutureCompleter<Result<DatumRef, ScriptError>>>,
}

/// Lingo call-stack depth at which we give up and report runaway recursion.
///
/// This is a backstop for a *hung movie*, not a correctness check, and it has to
/// clear the deepest recursion real content performs. 50 was far too low:
/// age_of_speed's `Gameplay::CalculateDistanceFromStart` walks the track ring
/// with one nested call per token, so it needs one frame per token plus the
/// enclosing event frames — 61 for level 1's 58-token ring, and its token tables
/// run to at least `t80` (83 frames) on other levels. A correct walk was being
/// killed at 46.
///
/// Note there is no cheaper structural test available: every frame of that
/// legitimate recursion shares one script, handler and bytecode index (a single
/// recursive call site), which is exactly the shape a runaway loop has. Depth is
/// the only signal that separates them, so the value just needs enough headroom.
/// Scopes are pre-allocated `Scope` structs, so headroom is cheap.
pub const MAX_STACK_SIZE: usize = 512;

/// One entry of the `tell <target> … end tell` stack.
///
/// A `tell` re-points two different things at the target, and which one applies
/// depends on the target's kind, so both are resolved up front when the block is
/// entered:
///   * `nested_player` — a `#movie` sprite hosts its own sub-player, and the
///     enclosed `tellcall`s dispatch into it (the loader→game command bridge).
///   * `film_loop` — a film-loop sprite carries its own playhead, and the
///     enclosed score reads (`the frame`, `the lastFrame`) resolve against that
///     film loop's internal score rather than the movie's.
/// Both are `None` for an unsupported/self target, in which case everything runs
/// against this player as if the `tell` weren't there.
#[derive(Clone, Default)]
pub struct TellTarget {
    pub nested_player: Option<usize>,
    pub film_loop: Option<CastMemberRef>,
}

pub struct DirPlayer {
    pub net_manager: NetManager,
    pub movie: Movie,
    /// Host-side composited stage image of each active nested `#movie` sub-player,
    /// keyed by the Linked Movie member. Each host frame the sub-player's stage is
    /// rendered (headless, CPU) and copied here into the HOST's bitmap_manager so
    /// the WebGL2 `Movie` sprite arm can blit it at the sprite rect. One BitmapId
    /// per member, overwritten in place.
    pub nested_movie_images: HashMap<CastMemberRef, BitmapId>,
    pub is_playing: bool,
    pub is_script_paused: bool,
    pub next_frame: Option<u32>,
    pub queue_tx: Sender<PlayerVMExecutionItem>,
    pub globals: FxHashMap<Symbol, DatumRef>,
    pub scopes: Vec<Scope>,
    pub bytecode_handler_manager: StaticBytecodeHandlerManager,
    pub breakpoint_manager: BreakpointManager,
    pub current_breakpoint: Option<BreakpointContext>,
    pub step_mode: StepMode,
    pub step_scope_depth: u32,
    pub break_on_error: bool,
    /// Run handlers through the register-IR compiler. Default OFF: this changes
    /// how every Lingo handler executes, so it is opt-in until measured.
    pub ir_enabled: bool,
    pub stage_size: (u32, u32),
    pub bitmap_manager: bitmap::manager::BitmapManager,
    pub cursor: CursorRef,
    pub start_time: chrono::DateTime<chrono::Local>,
    pub timeout_manager: TimeoutManager,
    pub title: String,
    pub bg_color: ColorRef,
    pub stage_draw_rect: Option<[f64; 4]>,
    /// Persistent script-owned stage framebuffer for "imaging Lingo" movies
    /// that draw directly into `(the stage).image` (e.g. spectral-wizard).
    /// Created lazily on first `(the stage).image` access and returned as the
    /// same BitmapId every call, so a cached `theStage = (the stage).image`
    /// keeps accumulating draws. `None` until first access.
    pub stage_image: Option<bitmap::manager::BitmapId>,
    /// Set true once a draw op (copyPixels/fill/etc.) targets `stage_image`.
    /// Only then does the renderer composite it over the sprite output —
    /// this keeps read-only camera-capture movies (which never draw) on the
    /// old per-call snapshot behavior.
    pub stage_image_dirty: bool,
    pub center_stage: bool,
    pub keyboard_focus_sprite: i16,
    pub text_selection_start: u16,
    pub text_selection_end: u16,
    /// Mirror of the OS clipboard's last-known plain text. Populated by the
    /// frontend's copy/cut listeners and by `the clipBoard = ...` assignment;
    /// read by `the clipBoard`. Not the source of truth — `paste` reads
    /// directly from the OS via the JS gesture event.
    pub clipboard_mirror: String,
    /// IME composition state. When `Some(start)`, an in-progress composition
    /// is active in the focused editable member: `start` is the byte offset
    /// where the provisional composition begins. `end` is the current end of
    /// the provisional run (start + composition_text.len()). When None,
    /// no composition is in flight.
    pub ime_composition: Option<(i32, i32)>,
    pub mouse_loc: (i32, i32),
    pub wants_pointer_lock: bool,
    pub cursor_is_hidden: bool,
    /// Track parent DatumRef for chained property access (transform.position.z = value)
    /// (vector DatumRef, parent transform DatumRef, sub-property name)
    pub transform_sub_refs: Vec<(DatumRef, DatumRef, Symbol)>,
    pub last_mouse_down_time: i64,
    pub is_double_click: bool,
    pub mouse_down_sprite: i16,
    pub drag_offset: (i32, i32),
    /// In-progress drag of a `#scroll` field's lift: (sprite number, grab offset
    /// from the lift's top edge in local px). Cleared on mouse up.
    pub field_scroll_drag: Option<(i16, i32)>,
    pub trails_bitmap: Option<bitmap::bitmap::Bitmap>,
    pub click_on_sprite: i16,
    /// Sprite whose CAST MEMBER script is currently executing. A member script
    /// is not a behavior — it runs with no receiver instance — so
    /// `the currentSpriteNum` has no `spriteNum` property to read and would
    /// otherwise report 0. Director still answers with the sprite the message
    /// was sent to, which member scripts rely on to identify themselves
    /// (Lifesavers Pineapple Treasure Hunt gives every walkable terrain member
    /// an `on prepareFrame addPlatform()` script, and `addPlatform` bails out
    /// on `the currentSpriteNum = 0`). Set around member-script dispatch only.
    pub member_script_sprite_num: i16,
    /// Queue of pending `on cuePassed` events that `SoundChannel::update`
    /// has detected but not yet dispatched. The frame loop drains this
    /// after `tick_sound_manager` (`SoundChannel::update` runs synchronously
    /// from inside that tick, so the actual `player_invoke_frame_and_movie_scripts`
    /// call — which is `async` — has to happen outside the tick).
    /// Tuple: (channel_num_1_based, cue_number_1_based, cue_name).
    pub pending_cue_events: Vec<(i32, i32, String)>,
    /// Host notifications captured during player mutation. The owning
    /// session drains this queue after releasing its player borrow.
    pub(crate) pending_player_notifications: Vec<PlayerNotification>,
    /// Bounded portable lifecycle/cast mailbox. Ordered transitions are
    /// retained; state snapshots may be coalesced by `HostEventMailbox`.
    pub(crate) host_event_mailbox: HostEventMailbox,
    /// One terminal backpressure marker is enough to stop the owner at its
    /// next detached drain without growing an unbounded fallback queue.
    pub(crate) host_event_backpressure: Option<HostEventOverflow>,
    /// Anchor for syncing score-frame advance to audio-context time while
    /// sound channel 1 is playing. Set the moment audio actually starts
    /// (`source.start()`), cleared when audio stops. The frame loop uses
    /// this to compute the target frame from `audio_currentTime` and skip
    /// the per-frame `target_delay_ms` wait when the score is behind —
    /// without it, the wall-clock'd frame loop drifts behind audio over
    /// time and visuals (Fugue No.4's Cross sprites tweened by the score)
    /// fall behind the audio-clocked `on cuePassed` driven cursor.
    /// Tuple: (score_frame_at_audio_start, audio_context_time_at_audio_start).
    pub audio_sync_anchor: Option<(u32, f64)>,
    pub subscribed_member_refs: Vec<CastMemberRef>, // TODO move to debug module
    pub is_subscribed_to_channel_names: bool,       // TODO move to debug module
    /// Debug-UI subscriptions. Serializing the score or a cast's member list
    /// costs tens of thousands of JS objects, so it only happens while an
    /// inspector is actually showing that data. Nothing is pushed by default:
    /// during ordinary playback (no debug UI) the boundary stays quiet.
    pub is_subscribed_to_score: bool,
    pub subscribed_cast_member_lists: HashSet<u32>,
    pub font_manager: FontManager,
    pub keyboard_manager: KeyboardManager,
    pub float_precision: u8,
    pub last_handler_result: DatumRef,
    /// Sprites the pointer is currently within, front-most first. Rollover
    /// events are per-sprite (see `get_sprites_at`), so this has to be a set
    /// rather than just the front-most sprite.
    pub hovered_sprites: Vec<i16>,
    /// Animated GIF cast members, keyed by (cast_lib, member number). Each
    /// holds its decoded frames; the frame loop swaps the member's image_ref
    /// as the delays elapse. See player::gif.
    pub gif_animations: std::collections::HashMap<(u32, u32), crate::player::gif::GifAnimation>,
    pub picking_mode: bool,
    /// Capability shared by all arena-backed handles owned by this player.
    /// It contains lifecycle metadata only; it never points back to this VM.
    pub owner: OwnerToken,
    /// Transform3d datum ids mutated in-place by this player. The set is
    /// consumed by the explicit W3D flush path; it is never process-global.
    pub(crate) w3d_dirty_transform_ids: HashSet<usize>,
    pub allocator: DatumAllocator,
    pub dir_cache: HashMap<Box<str>, Rc<DirectorFile>>,
    pub scope_count: u32,
    /// Invalidation generation for the whole handler stack. Scope slots have
    /// their own generations for reuse; this epoch covers operations that
    /// clear or replace the complete stack while a callback is in flight.
    pub(crate) scope_invalidation_epoch: u64,
    /// Insertion-ordered: Director's `externalParamName(n)` /
    /// `externalParamValue(n)` are indexed accessors, so the order the host
    /// (or `LeechProtectionRemovalHelp`'s `setExternalParam`) supplied the
    /// params in is observable from Lingo.
    pub external_params: IndexMap<String, String>,
    // XML document storage - maps XML document IDs to parsed XML structures
    pub xml_documents: HashMap<u32, XmlDocument>,
    // XML node storage - maps node IDs to XML nodes
    pub xml_nodes: HashMap<u32, XmlNode>,
    // Counter for generating unique XML IDs
    pub next_xml_id: u32,
    pub sound_manager: SoundManager,
    pub date_objects: HashMap<u32, DateObject>,
    pub math_objects: HashMap<u32, MathObject>,
    pub enable_stream_status_handler: bool,
    /// Tracks the last streamStatus phase reported per net task.
    pub stream_status_reported: HashMap<u32, net_task::StreamStatusPhase>,
    pub is_in_frame_update: bool,
    pub is_dispatching_events: bool, // Prevents re-entrant event dispatch
    pub is_in_send_all_sprites: bool, // Prevents re-entrant sendAllSprites calls
    /// `tell <target> … end tell` stack — see [`TellTarget`]. A Vec so `tell`
    /// blocks can nest; only the innermost entry is consulted.
    pub tell_target_stack: Vec<TellTarget>,
    pub system_start_time: chrono::DateTime<chrono::Local>, // For ticks & milliSeconds (system uptime)
    pub handler_stack_depth: usize,
    /// Per-player routing state for a Havok step callback turn.
    pub(crate) in_havok_step_callback: bool,
    pub in_frame_script: bool,
    /// Per-sprite Flash frame buffers. One Ruffle instance per (sprite,
    /// cast_member) gives each Flash sprite an independent playhead, which
    /// is what Director semantics actually require (e.g. storyscramble's 3
    /// story tiles share one Flash member but display different poster
    /// frames simultaneously). Keyed by sprite number; the renderer reads
    /// `flash_frame_buffers[channel_num]` when drawing a Flash sprite.
    pub flash_frame_buffers: HashMap<i16, bitmap::manager::BitmapId>,
    /// Whether the lazy-load dispatch has fired for a given (sprite,
    /// cast_lib, cast_member). Prevents the renderer from triggering
    /// duplicate `createFlashInstance` calls every frame before the
    /// instance's first pixels arrive.
    pub flash_sprite_loaded: HashSet<(i16, i32, i32)>,
    /// Flash host effects prepared under the player borrow and emitted only
    /// after the owning session borrow has ended.
    pub(crate) flash_host_actions: Vec<FlashHostAction>,
    /// Legacy nested players retain synthetic host keys. Session-owned nested
    /// children switch to `LocalOwned` after exact frontend registration.
    pub(crate) flash_host_is_nested: bool,
    pub(crate) flash_host_route: FlashHostRoute,
    /// Sprites whose Ruffle instance has been confirmed loaded + AS-initialized
    /// at least once. Flash interop (getVariable/setVariable/callFunction/
    /// setCallback) takes the SYNC fast path for these; only the FIRST access to
    /// a sprite ever goes through the async wait (which then adds it here).
    /// STICKY for this owner generation — once a sprite has had a ready instance
    /// we keep the sync path across same-owner member swaps, which safely returns
    /// null/void if the replacement instance is transiently not ready. The
    /// async path exists only to make the one-shot startup call (Coke Studios'
    /// SESSION_createSession, which runs before its SWF is dispatched) get a
    /// live instance; resetting the owner clears this cache so a new generation
    /// receives its own first-access wait. This keeps the hundreds of per-frame
    /// interop calls sync with no async-dispatch overhead.
    pub flash_ready_sprites: HashSet<i16>,
    /// Owner-local Flash instance generation authority shared with browser
    /// capabilities without borrowing the session during host callbacks.
    pub flash_binding_state: Rc<RefCell<FlashBindingState>>,
    /// Owner-local synthetic Flash object path counter.
    pub flash_object_counter: u32,
    /// Owner-scoped scripted Flash access is waiting for a ready Ruffle
    /// instance. Display-only loads do not set this flag.
    /// Owner-generation-local Flash scripted-access gate.  This is deliberately
    /// detached from the session borrow so a browser host callback can publish
    /// readiness while the VM is suspended in an async action.
    pub flash_scripted_access_pending: Rc<Cell<bool>>,
    /// Flash `LocalConnection` receiver registry (Neopets DGS score/protocol).
    /// A Director-created LocalConnection is `newObject("LocalConnection")` →
    /// synthetic `_root.__dpObj_LocalConnection_N` ref, `connect(name)` claims a
    /// name, and `setCallback(lc, method, #handler, target)` wires a method. The
    /// SWF's AS `LocalConnection.send(name, method, args)` is forwarded by the
    /// Ruffle fork to `local_connection_send`, which routes name→lc_path→handler.
    /// `flash_lc_connections`: connection name → lc synthetic path.
    pub flash_lc_connections: std::collections::HashMap<String, String>,
    /// `(lc_path, method)` → `(lingo handler, target instance)` — the target keeps
    /// `me` correct in `on <handler> me, aInfo, aMessage`.
    pub flash_lc_callbacks:
        std::collections::HashMap<(String, String), (String, ScriptInstanceRef)>,
    /// Set true whenever a script reads live input OR a wall clock —
    /// `keyPressed(...)`, `the keyPressed`, `the key`, `the keyCode`,
    /// `the mouseDown`, `the stillDown`, `the ticks`, `the milliSeconds`,
    /// `the timer`. The bytecode loop's busy-wait yield counts a loop iteration
    /// toward yielding ONLY when this was set that iteration, so it cooperatively
    /// yields for input-wait spins (Neopets' `repeat while keyPressed(" ") end`)
    /// and for time-based frame throttles (`repeat while (the ticks - t0) < N`),
    /// but NEVER for compute/AMF loops (Coke Studios' object-graph conversion),
    /// which read neither, so a yield there only adds latency.
    pub input_polled: bool,
    /// Off-screen Flash members rendered into W3D textures (frog01's environment:
    /// `newTexture(#fromCastMember, flashMember)`). Keyed by a synthetic NEGATIVE
    /// sprite number (so it never collides with on-stage positive channels);
    /// maps to the target W3D cast member + texture name. `update_flash_frame`
    /// routes captured frames for these synthetic numbers into the named texture
    /// instead of `flash_frame_buffers`. See `flash_texture_synthetic_id`.
    pub flash_texture_targets: HashMap<i16, (cast_lib::CastMemberRef, String)>,
    /// Cached rendered 3D scene bitmaps (populated during sprite rendering, read by world.image)
    pub w3d_frame_buffers: HashMap<(i32, i32), bitmap::manager::BitmapId>,
    /// Members whose `.image` a script has actually asked for. The per-frame FBO
    /// readback that fills `w3d_frame_buffers` is a synchronous GPU->CPU stall
    /// (22.9% of frame time in a profile) plus two full-size buffer allocations,
    /// and it ran speculatively for EVERY 3D member every frame. Most movies
    /// never touch `world.image`, so only capture once someone has asked.
    ///
    /// The first request finds no cached frame and falls through to the getter's
    /// offscreen `render_3d_to_rgba` path, so nothing is ever served stale.
    pub w3d_image_requested: std::collections::HashSet<(i32, i32)>,
    /// Set once any 3D member has rendered. Previously `!w3d_frame_buffers
    /// .is_empty()` stood in for "3D content is active" when deciding whether to
    /// request pointer lock; with the readback now lazy that map can legitimately
    /// stay empty, which would have silently broken FPS mouselook.
    pub w3d_any_rendered: bool,
    pub in_enter_frame: bool,
    pub in_prepare_frame: bool,
    pub in_step_frame: bool,
    pub in_event_dispatch: bool,
    /// Set by `stopEvent()` (Director 11.5 Scripting Dictionary, Movie method):
    /// "prevents scripts from passing an event message to subsequent locations
    /// in the message hierarchy… Neither subsequent scripts nor other behaviors
    /// on the sprite receive the event if it is stopped in this manner."
    /// Scoped to the event currently being dispatched — the dispatch entry
    /// points save/restore it so a nested `sendSprite` can't leak the flag.
    pub event_stopped: bool,
    pub command_handler_yielding: bool, // Pauses frame loop when a command handler (keyDown) needs updateStage to yield
    /// Frame/movie-script handlers currently executing via player_invoke_static_event,
    /// as (script member, handler name). Guards against a static-event handler
    /// being re-invoked while it's already on the call stack for the SAME event —
    /// Director's message system won't re-dispatch a message to a handler already
    /// processing it. Fish's movie `on keyDown` does `sendSprite(4, #keyDown)`,
    /// which (when sprite 4 has no keyDown behavior) propagates back up to the
    /// movie script per the sendSprite hierarchy; without this guard it recurses
    /// into the same `on keyDown` forever (stack overflow).
    /// Static-event re-entrancy guard: (script, handler, ARGS). The args are
    /// part of the key — a handler legitimately re-entered with DIFFERENT
    /// arguments is a nested dispatch, not a propagation loop. See
    /// `player_invoke_static_event`.
    pub active_static_event_handlers: Vec<(CastMemberRef, String, Vec<DatumRef>)>,
    /// Timestamp (Date.now ms) of the last frame update driven from updateStage()
    /// while `command_handler_yielding`. The frame loop is paused during a keyDown
    /// busy-wait loop, so updateStage() runs the frame's animation itself,
    /// throttled by tempo — see MovieHandlers::update_stage.
    pub last_kb_loop_frame_ms: f64,
    pub in_mouse_command: bool, // Pauses frame loop during mouse handlers; updateStage renders without yielding
    pub nothing_call_count: u32, // Consecutive nothing() calls, reset after yield
    pub last_nothing_yield_ms: f64, // Timestamp of last nothing() yield for time-throttled rendering
    /// Timestamp of the last `updateStage()` yield. Each yield is an
    /// `async_std::task::sleep`, which on wasm allocates a `gloo_timers`
    /// Timeout — one `setTimeout` on creation and one `clearTimeout` when the
    /// future drops. A script that calls `updateStage()` in a tight loop
    /// (progress bars, busy-waits) therefore paid two JS timer operations per
    /// iteration, and the churn cost more than the work: in Argent Free Ride's
    /// level load, `clearTimeout` alone was 42% of self time, and the tab
    /// stopped responding. Throttled the same way `nothing_async` already is.
    pub last_update_stage_yield_ms: f64,
    pub current_frame_tempo: u32, // Cached tempo for the current frame
    pub has_player_frame_changed: bool,
    pub stage_dirty: bool, // Set when any sprite property changes; cleared after render
    /// Set when an input handler (mouse or key) starts running, cleared once
    /// the next exitFrame has run. While it is set the stage is not redrawn:
    /// Director draws once per frame, after the handlers of that frame and
    /// its exitFrame have both run, so a handler's half-finished state is
    /// never on screen. Matematik i Maaneby's crane shows a claw sprite from
    /// mouseUp and moves it into place in exitFrame; drawing in between put
    /// the claw where that sprite last was for one frame. A timestamp, so a
    /// hold can never outlive a stalled frame loop.
    pub draw_hold_since_ms: Option<i64>,
    pub preview_dirty: bool, // Set when preview member/settings change; cleared after preview render
    pub has_frame_changed_in_go: bool,
    pub go_same_frame: bool,
    pub go_direction: u8,
    pub is_getting_property_descriptions: bool,
    pub is_initializing_behavior_props: bool,
    pub last_initialized_frame: Option<u32>,
    /// Playback lifecycle effects exposed to the browser regression harness.
    /// These counters are owner-local and reset with the player; they are not
    /// used to drive playback decisions.
    pub playback_init_count: u32,
    pub playback_frame_count: u64,
    pub playback_stop_count: u32,
    /// Current score context for sprite property access.
    /// When a filmloop sprite's behavior runs, this is set to the filmloop's ScoreRef
    /// so that sprite(n) accesses the filmloop's sprites, not the main stage.
    pub current_score_context: ScoreRef,
    pub debug_datum_refs: Vec<DatumRef>,
    pub eval_scope_index: Option<u32>,
    pub delay_until: Option<chrono::DateTime<chrono::Local>>,
    /// Pending gotoNetMovie operation: (task_id, frame_destination).
    /// Overwritten by subsequent gotoNetMovie/go-to-movie calls (cancels previous).
    pub pending_goto_net_movie: Option<(u32, MovieFrameTarget)>,
    /// True while `go(frame, movie)` is blocked waiting for the target movie's
    /// fetch. `maybe_hold_dcr_for_preloader` (the Neopets nested-.dcr hold)
    /// checks it and stands down: Director's `go()` loads the movie NOW — the
    /// artificial 3s "still streaming" window is for preloadNetThing-style
    /// game loads, not for a navigation the handler is synchronously waiting on.
    pub goto_wait_active: bool,
    /// Set by an EAGER `go(frame, movie)` mount (the movie was swapped while a
    /// Lingo handler was still executing). The frame loop runs
    /// `run_movie_init_sequence()` once the handler stack has unwound, and
    /// `run_single_frame` treats it as "stop processing this frame" so no
    /// event reaches the new movie's scripts before prepareMovie.
    pub pending_movie_init: bool,
    /// Cast libraries of movies replaced by an eager mid-handler `go(frame,
    /// movie)` mount. Suspended trampoline frames retain `Rc` snapshots of
    /// their scripts, handlers, and name symbols; keeping the old casts alive
    /// preserves the source owner until the handler stack unwinds.
    /// Moving the `Vec<CastLib>` here is safe: the element buffer (where the
    /// CastLib structs and their `name_symbols` headers live) does not move.
    pub retired_cast_libs: Vec<Vec<crate::player::cast_lib::CastLib>>,
    /// Bumped every time a movie is mounted over a live handler stack. Event
    /// dispatch loops capture it before iterating their (old-movie) receiver
    /// snapshots and stop when it changes, so no old-movie instance receives
    /// an event resolved against the new movie's casts.
    pub movie_mount_generation: u64,
    /// Set by `play movie <the current movie>` (restart). The frame loop, between
    /// frames, re-parses the retained movie bytes and runs the full load+init
    /// (rebuilds the cast → fresh W3D scenes), preserving globals + external params
    /// like Director's `play movie`. (The net loader can't always re-fetch by name
    /// once loaded, so we keep the original bytes.)
    pub pending_restart: bool,
    /// Raw bytes + file_name + base_url of the loaded movie, retained for restart.
    pub movie_reload_data: Option<(Vec<u8>, String, String)>,
    /// True while a net movie transition is in progress.
    /// Prevents the event loop from dispatching external events during the transition.
    pub is_in_transition: bool,
    /// A score transition detected on frame entry, awaiting playback start by the
    /// renderer (which snapshots the pre-transition stage). Set by advance_frame.
    pub pending_transition: Option<crate::player::cast_member::TransitionInfo>,
    /// True while a score/puppet transition holds the playhead (Director blocks
    /// during a transition). The renderer clears it when the animation completes
    /// (precise sync). A separate field from `is_in_transition` (which means a
    /// movie swap is loading) so the two never collide.
    pub score_transition_active: bool,
    /// Failsafe deadline (wall-clock ms) for the transition hold: if the renderer
    /// never signals completion (backend swap, stopped movie), the hold clears
    /// here so a transition can never permanently freeze the movie.
    pub transition_hold_until_ms: Option<i64>,
    pub actor_list_generation: u64,
    pub behavior_channel_cache_generation: u64,
    pub active_stage_filmloop_cache_generation: u64,
    pub rng: rand::rngs::SmallRng,
    /// Cache of allocated scriptInstanceList datums per sprite.
    /// Ensures that `sprite.scriptInstanceList.add(x)` modifies the live list
    /// rather than a copy. Keyed by sprite number.
    pub script_instance_list_cache: FxHashMap<i16, DatumRef>,
    /// Reverse lookup from cached scriptInstanceList datum id to sprite number.
    pub script_instance_list_cache_owner: FxHashMap<usize, i16>,
    /// Mutation generation per cached scriptInstanceList.
    pub script_instance_list_generation: FxHashMap<i16, u64>,
    /// Parsed script instance ids keyed by sprite and cache generation.
    pub script_instance_list_ids_cache: FxHashMap<i16, (u64, Vec<ScriptInstanceRef>)>,
    /// Cached stage channels with active behaviors for the current frame.
    pub active_stage_behavior_channels_cache: Option<(u32, u64, Vec<usize>)>,
    /// Channels that can RECEIVE a message this frame: those with behaviors
    /// (the behavior cache above) plus those whose cast member carries a
    /// script. Director sends an event to a sprite's behaviors and then to its
    /// cast member script, so a sprite with only the latter is still a
    /// receiver — it just isn't a "behavior channel".
    pub active_stage_message_channels_cache: Option<(u32, u64, Vec<usize>)>,
    /// Cached visible filmloop members currently present on the stage for a frame.
    pub active_stage_filmloop_members_cache: Option<(u32, u64, Vec<CastMemberRef>)>,
    /// Set by `sprite_get_prop` when a property returns a pre-allocated DatumRef
    /// (e.g. cached scriptInstanceList). Callers should check this before
    /// allocating a new DatumRef, to ensure mutations share the same arena entry.
    pub last_sprite_prop_ref: Option<DatumRef>,
    pub virtual_scripts: FxHashMap<CastMemberRef, Rc<dyn virtual_scripts::VirtualScriptHandler>>,
    /// Per-player Xtra instances. Numeric ids are scoped to this state.
    pub(crate) xtra_manager_state: XtraManagerState,
    /// Retained external Xtra scenes belong only to this player.
    pub(crate) scene3d_store: Scene3dStore,
    /// Runtime overrides for `the scriptText of member`. Director exposes a
    /// member's Lingo source as a settable string on ANY member type; some
    /// movies (e.g. freeT) use it as scratch data storage. We don't compile
    /// Lingo, so we just round-trip the string here keyed by member ref.
    pub script_text_overrides: FxHashMap<CastMemberRef, String>,
    /// `member(whichFlashMember).cdpCheckMode` — Flash cross-domain-policy
    /// check mode (Director 11.5 reference: Access Get/Set, default
    /// `useSwPolicy`; `#useMediaPolicy` selects the Flash player's checks,
    /// `#useSWPolicy` Shockwave's).
    ///
    /// Stored, not acted on: in a browser the cross-origin decision is made by
    /// CORS on the actual fetch, so neither policy path exists for us. Keeping
    /// the value means the documented Get/Set round-trip still works for a
    /// script that reads it back.
    pub cdp_check_modes: FxHashMap<CastMemberRef, String>,
    /// Optional fake movie path override. When set, `the moviePath` and `the movieName`
    /// return values derived from this path, while actual file fetching uses the real URL.
    /// URLs the script builds from `the moviePath` (e.g. `postNetText(the moviePath & "x.aspx")`)
    /// are rewritten back to the real base in net handlers.
    pub movie_path_override: Option<String>,
    /// Like `movie_path_override` but **purely informational** — sets
    /// `the moviePath` / `the movieName` for the script to read, and
    /// nothing more. Net handlers do NOT rewrite URLs constructed from
    /// it; the script-emitted URL is fetched as-is. Use this when the
    /// JS-side fetch interceptor already handles host routing (e.g.
    /// `flashPlayerManager.ts` rewriting `maidmarian.com` to a local
    /// CORS proxy) and you want the URL the script builds to land at
    /// the proxy unchanged. Mutually exclusive with the rewrite path —
    /// when both are set, this label wins for `the moviePath`.
    pub movie_path_label: Option<String>,
    /// Lingo to evaluate once, immediately before the launched movie's
    /// `prepareMovie` — the equivalent of the Shockwave projector's `--do`
    /// launch argument. Flashpoint drives archived Shockwave titles this way;
    /// Agent Free Ride's entry is
    ///
    /// ```text
    /// "…/wrapper_silentbaystudios.dcr"
    ///   --do "member('gameUrl').text = '…/agent_freeride.dcr'"
    ///   --bugfixShockwave3DBadDriverList
    /// ```
    ///
    /// The wrapper reads `member("gameUrl").text` on its first `exitFrame` and
    /// redirects there, but that member ships EMPTY — the launcher seeds it, so
    /// without this the wrapper has nowhere to go.
    ///
    /// Consumed on first use (`Option::take`), matching the projector: the
    /// argument applies to the movie being launched, not to every movie the
    /// session later loads.
    pub startup_do: Option<String>,
    /// The projector's `--doBefore`: Lingo evaluated once BEFORE the movie is
    /// loaded, as opposed to [`startup_do`](Self::startup_do)'s "after it has
    /// been loaded". SPR documents it for curator diagnostics
    /// (`--doBefore "alert('I am a jelly donut!')"`). Also consumed on use.
    pub startup_do_before: Option<String>,
    /// The projector's `--go`: a frame to jump to once the movie has started,
    /// e.g. `--go 2` to skip a title frame. Applied after the init sequence,
    /// so the movie's own `prepareMovie` / `startMovie` have run first.
    pub startup_go: Option<u32>,
    /// Fake environment installed by the LeechProtectionRemovalHelp Xtra.
    /// Lives on the Player rather than the Movie so it outlives a movie load,
    /// which is precisely what that Xtra promises.
    pub env_overrides: EnvOverrides,
    pub console: console::ConsoleBuffer,
}

/// Reduce a movie-location label to the directory string Director's `the
/// moviePath` / `the path` return — those are the *directory* part, never the
/// full file URL. A label may arrive either way: the `_moviePath` external
/// param and `set_movie_path_label` typically carry a full movie URL, while
/// LeechProtectionRemovalHelp's `setTheMoviePath` is documented with a
/// trailing-slash directory. Both reduce correctly here.
fn dir_part_of_path(path: &str) -> String {
    if let Ok(url) = Url::parse(path) {
        get_base_url(&url).to_string()
    } else if let Some(pos) = path.rfind('/') {
        path[..=pos].to_string()
    } else if let Some(pos) = path.rfind('\\') {
        path[..=pos].to_string()
    } else {
        // Single-segment path: nothing to strip — append a separator so
        // concatenation with movieName produces a well-formed path.
        let mut s = path.to_string();
        if !s.ends_with('/') && !s.ends_with('\\') {
            s.push('/');
        }
        s
    }
}

/// Target frame for a movie transition (gotoNetMovie or go movie).
#[derive(Clone)]
pub enum MovieFrameTarget {
    /// No specific frame — start at frame 1
    Default,
    /// Jump to a labeled frame (from URL #fragment or string arg)
    Label(String),
    /// Jump to a specific frame number
    Frame(u32),
}

impl DirPlayer {
    pub(crate) fn set_nested_flash_host_route(&mut self, route: FlashHostRoute) {
        debug_assert!(matches!(
            route,
            FlashHostRoute::LocalOwned | FlashHostRoute::NestedPending
        ));
        self.flash_host_is_nested = true;
        self.flash_host_route = route;
    }

    pub(crate) fn set_legacy_flash_host_route(&mut self) {
        self.flash_host_is_nested = true;
        self.flash_host_route = FlashHostRoute::LegacySynthetic;
    }

    pub(crate) fn reserve_flash_instance_generation(
        &self,
        sprite_num: i16,
    ) -> Result<u64, ScriptError> {
        self.flash_binding_state.borrow_mut().reserve(sprite_num)
    }

    pub(crate) fn invalidate_flash_instance_generation(
        &self,
        sprite_num: i16,
        expected: u64,
    ) -> bool {
        self.flash_binding_state
            .borrow_mut()
            .invalidate(sprite_num, expected)
    }

    pub(crate) fn is_flash_instance_generation_current(
        &self,
        sprite_num: i16,
        expected: u64,
    ) -> bool {
        self.flash_binding_state
            .borrow()
            .is_current(sprite_num, expected)
    }

    pub(crate) fn flash_instance_generation(&self, sprite_num: i16) -> Option<u64> {
        self.flash_binding_state
            .borrow()
            .generations
            .get(&sprite_num)
            .copied()
    }

    pub(crate) fn flash_binding_origin(&self, sprite_num: i16) -> FlashBindingOrigin {
        self.flash_binding_state.borrow().origin(sprite_num)
    }

    /// Return every exact generation requiring teardown. If the current
    /// generation still owns the requested pair, retire it before returning
    /// it so an Unload action always carries explicit retired authority.
    pub(crate) fn retire_or_get_flash_unload_generations(
        &self,
        sprite_num: i16,
        cast_lib: i32,
        cast_member: i32,
    ) -> Vec<u64> {
        let retired = self
            .flash_binding_state
            .borrow()
            .retired_generations_for_pair(sprite_num, cast_lib, cast_member);
        if !retired.is_empty() {
            let current = self.flash_instance_generation(sprite_num);
            let current_is_pair = matches!(
                self.flash_binding_origin(sprite_num),
                FlashBindingOrigin::Exact {
                    cast_lib: current_lib,
                    cast_member: current_member,
                }
                | FlashBindingOrigin::FirstPublished {
                    cast_lib: current_lib,
                    cast_member: current_member,
                } if (current_lib, current_member) == (cast_lib, cast_member)
            );
            if let Some(current) = current.filter(|_| current_is_pair) {
                self.flash_binding_state
                    .borrow_mut()
                    .retire_current_for_pair(sprite_num, cast_lib, cast_member, current);
            }
            return self
                .flash_binding_state
                .borrow()
                .retired_generations_for_pair(sprite_num, cast_lib, cast_member);
        }
        let current = self.flash_instance_generation(sprite_num);
        let current_is_pair = matches!(
            self.flash_binding_origin(sprite_num),
            FlashBindingOrigin::Exact {
                cast_lib: current_lib,
                cast_member: current_member,
            }
            | FlashBindingOrigin::FirstPublished {
                cast_lib: current_lib,
                cast_member: current_member,
            } if (current_lib, current_member) == (cast_lib, cast_member)
        );
        if let Some(current) = current.filter(|_| current_is_pair) {
            if self
                .flash_binding_state
                .borrow_mut()
                .retire_current_for_pair(sprite_num, cast_lib, cast_member, current)
            {
                return self
                    .flash_binding_state
                    .borrow()
                    .retired_generations_for_pair(sprite_num, cast_lib, cast_member);
            }
        }
        Vec::new()
    }

    pub(crate) fn current_flash_bindings_for_cast_lib(
        &self,
        cast_lib: i32,
    ) -> Vec<(i16, i32, i32, u64)> {
        let state = self.flash_binding_state.borrow();
        state
            .generations
            .iter()
            .filter_map(|(sprite_num, generation)| match state.origin(*sprite_num) {
                FlashBindingOrigin::Exact {
                    cast_lib: current_lib,
                    cast_member,
                }
                | FlashBindingOrigin::FirstPublished {
                    cast_lib: current_lib,
                    cast_member,
                } if current_lib == cast_lib => {
                    Some((*sprite_num, current_lib, cast_member, *generation))
                }
                _ => None,
            })
            .collect()
    }

    pub(crate) fn reconcile_flash_binding_origins(&mut self) {
        let bindings: Vec<_> = {
            let state = self.flash_binding_state.borrow();
            state
                .generations
                .iter()
                .filter_map(|(sprite_num, generation)| match state.origin(*sprite_num) {
                    FlashBindingOrigin::Exact {
                        cast_lib,
                        cast_member,
                    }
                    | FlashBindingOrigin::FirstPublished {
                        cast_lib,
                        cast_member,
                    } => Some((
                        *sprite_num,
                        cast_lib,
                        cast_member,
                        *generation,
                        state.origin(*sprite_num),
                    )),
                    FlashBindingOrigin::Absent => None,
                })
                .collect()
        };
        for (sprite_num, cast_lib, cast_member, generation, origin) in bindings {
            let current_pair = self
                .movie
                .score
                .get_sprite(sprite_num)
                .and_then(|sprite| sprite.member.as_ref())
                .map(|member| (member.cast_lib, member.cast_member));
            let classification = match current_pair {
                Some((current_lib, current_member))
                    if (current_lib, current_member) == (cast_lib, cast_member) =>
                {
                    match self.movie.cast_manager.find_member_by_ref(&CastMemberRef {
                        cast_lib,
                        cast_member,
                    }) {
                        None => FlashMemberClassification::UnresolvedExact,
                        Some(member) if matches!(&member.member_type, CastMemberType::Flash(flash) if crate::rendering::has_swf_signature(&flash.data)) => {
                            FlashMemberClassification::ValidFlash
                        }
                        Some(_) => FlashMemberClassification::InvalidTarget,
                    }
                }
                None => FlashMemberClassification::Absent,
                Some(_) => FlashMemberClassification::InvalidTarget,
            };
            let must_retire = match origin {
                FlashBindingOrigin::FirstPublished { .. } => {
                    !matches!(classification, FlashMemberClassification::ValidFlash)
                }
                FlashBindingOrigin::Exact { .. } => matches!(
                    classification,
                    FlashMemberClassification::InvalidTarget | FlashMemberClassification::Absent
                ),
                FlashBindingOrigin::Absent => false,
            };
            if must_retire {
                let mut state = self.flash_binding_state.borrow_mut();
                if state.retire_current_for_pair(sprite_num, cast_lib, cast_member, generation) {
                    state.origins.insert(sprite_num, FlashBindingOrigin::Absent);
                }
            }
        }
    }

    pub(crate) fn transition_flash_member(
        &mut self,
        sprite_num: i16,
        previous: Option<(i32, i32)>,
        next: Option<(i32, i32)>,
    ) {
        let classification = match next {
            None => FlashMemberClassification::Absent,
            Some((cast_lib, cast_member)) => {
                match self
                    .movie
                    .cast_manager
                    .find_member_by_ref(&cast_lib::CastMemberRef {
                        cast_lib,
                        cast_member,
                    }) {
                    None => FlashMemberClassification::UnresolvedExact,
                    Some(member) if matches!(&member.member_type, CastMemberType::Flash(flash) if crate::rendering::has_swf_signature(&flash.data)) => {
                        FlashMemberClassification::ValidFlash
                    }
                    Some(_) => FlashMemberClassification::InvalidTarget,
                }
            }
        };
        self.flash_binding_state.borrow_mut().transition_member(
            sprite_num,
            previous,
            next,
            classification,
        );
    }

    pub(crate) fn take_flash_host_actions(&mut self) -> Vec<FlashHostAction> {
        std::mem::take(&mut self.flash_host_actions)
    }

    fn flash_action_fence(&self) -> FlashActionFence {
        FlashActionFence::legacy(self.owner.clone(), self.flash_binding_state.clone())
    }

    fn queue_flash_host_action(&mut self, action: FlashHostAction) {
        self.flash_host_actions.push(action);
    }

    pub(crate) fn queue_flash_member_load(
        &mut self,
        host_sprite: i32,
        local_sprite: i16,
        cast_lib: i32,
        cast_member: i32,
        data: Vec<u8>,
        width: u32,
        height: u32,
        paused_at_start: bool,
        asserted_frame: i32,
    ) -> Result<bool, ScriptError> {
        if local_sprite <= 0
            || self
                .flash_sprite_loaded
                .contains(&(local_sprite, cast_lib, cast_member))
        {
            return Ok(false);
        }
        let generation = self.flash_binding_state.borrow_mut().reserve_for_pair(
            local_sprite,
            cast_lib,
            cast_member,
        )?;
        self.queue_flash_host_action(FlashHostAction::Load {
            fence: self.flash_action_fence(),
            host_sprite,
            local_sprite,
            cast_lib,
            cast_member,
            data,
            width,
            height,
            paused_at_start,
            asserted_frame,
            generation,
        });
        self.flash_sprite_loaded
            .insert((local_sprite, cast_lib, cast_member));
        Ok(true)
    }

    pub(crate) fn queue_flash_resize(
        &mut self,
        host_sprite: i32,
        local_sprite: i16,
        cast_lib: i32,
        cast_member: i32,
        width: u32,
        height: u32,
    ) {
        let Some(generation) = self.flash_instance_generation(local_sprite) else {
            return;
        };
        self.queue_flash_host_action(FlashHostAction::Resize {
            fence: self.flash_action_fence(),
            host_sprite,
            local_sprite,
            cast_lib,
            cast_member,
            generation,
            width,
            height,
        });
    }

    pub(crate) fn next_flash_object_id(&mut self) -> Result<u32, ScriptError> {
        self.flash_object_counter = self
            .flash_object_counter
            .checked_add(1)
            .ok_or_else(|| ScriptError::new("Flash object path counter exhausted".to_owned()))?;
        Ok(self.flash_object_counter)
    }

    pub(crate) fn queue_player_notification(&mut self, kind: PlayerNotificationKind) {
        let wake_owner_loop =
            self.pending_player_notifications.is_empty() && self.host_event_mailbox.is_empty();
        self.pending_player_notifications.push(PlayerNotification {
            owner: self.owner.clone(),
            kind,
        });
        if wake_owner_loop {
            let _ = self.queue_tx.try_send(PlayerVMExecutionItem {
                command: commands::PlayerVMCommand::PumpPending,
                completer: None,
            });
        }
    }

    /// Restore detached non-host notifications ahead of work queued
    /// reentrantly during their callback. These are the existing evaluator
    /// notification queue entries, not a spill path for host events.
    pub(crate) fn prepend_player_notifications(&mut self, events: Vec<PlayerNotification>) {
        if events.is_empty() {
            return;
        }
        let wake_owner_loop =
            self.pending_player_notifications.is_empty() && self.host_event_mailbox.is_empty();
        let mut retained = std::mem::take(&mut self.pending_player_notifications);
        self.pending_player_notifications = events;
        self.pending_player_notifications.append(&mut retained);
        if wake_owner_loop {
            let _ = self.queue_tx.try_send(PlayerVMExecutionItem {
                command: commands::PlayerVMCommand::PumpPending,
                completer: None,
            });
        }
    }

    /// Queue a portable host event without retaining any VM/JS reference.
    /// Callers that cannot propagate the error must surface it through their
    /// existing script error path; this method never evicts an older ordered
    /// transition to make room for a new one.
    pub(crate) fn queue_host_event(&mut self, event: HostEvent) -> Result<(), HostEventOverflow> {
        // Mailbox overflow is terminal for this owner generation. Do not
        // accept later events behind the marker: callers must reset or
        // rebind the owner before host delivery can resume.
        if let Some(error) = self.host_event_backpressure {
            return Err(error);
        }
        let wake_owner_loop =
            self.host_event_mailbox.is_empty() && self.pending_player_notifications.is_empty();
        if let Err(error) = self.host_event_mailbox.push(event) {
            // Do not spill into pending_player_notifications: that queue is
            // intentionally unbounded for evaluator continuations. Record a
            // single terminal marker so the next detached drain stops this
            // owner and reports explicit backpressure to its caller.
            self.host_event_backpressure = Some(error);
            if wake_owner_loop {
                let _ = self.queue_tx.try_send(PlayerVMExecutionItem {
                    command: commands::PlayerVMCommand::PumpPending,
                    completer: None,
                });
            }
            return Err(error);
        }
        if wake_owner_loop {
            let _ = self.queue_tx.try_send(PlayerVMExecutionItem {
                command: commands::PlayerVMCommand::PumpPending,
                completer: None,
            });
        }
        Ok(())
    }

    pub(crate) fn take_host_events(&mut self) -> Vec<HostEvent> {
        self.host_event_mailbox.drain()
    }

    /// Restore detached host events ahead of work queued reentrantly during
    /// their callback. Overflow remains terminal for this owner generation.
    pub(crate) fn prepend_host_events(
        &mut self,
        events: Vec<HostEvent>,
    ) -> Result<(), HostEventOverflow> {
        if events.is_empty() {
            return Ok(());
        }
        if let Some(error) = self.host_event_backpressure {
            return Err(error);
        }
        if let Err(error) = self.host_event_mailbox.prepend(events) {
            self.host_event_backpressure = Some(error);
            return Err(error);
        }
        Ok(())
    }

    pub(crate) fn with_xtra_manager_state<T>(
        &mut self,
        f: impl FnOnce(&mut XtraManagerState, &mut DirPlayer) -> T,
    ) -> T {
        let owner = self.owner.clone();
        let mut state =
            std::mem::replace(&mut self.xtra_manager_state, XtraManagerState::new(owner));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut state, self)));
        self.xtra_manager_state = state;
        match result {
            Ok(result) => result,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    pub fn new<'a>(tx: Sender<PlayerVMExecutionItem>) -> DirPlayer {
        let mut player = Self::new_with_owner(tx, OwnerToken::transitional());
        // Legacy entrypoints historically performed host cleanup during
        // construction. Session-owned construction uses the pure path below.
        player.reset();
        player
    }

    pub(crate) fn new_with_owner<'a>(
        tx: Sender<PlayerVMExecutionItem>,
        owner: OwnerToken,
    ) -> DirPlayer {
        // Director 11.5 Scripting Dictionary, Sound object: "The Director sound
        // object controls audio playback in all SIXTEEN available sound channels."
        // (The Sound Channel entry still says eight — legacy text from before the
        // count was raised; the Sound object is the authority, and the Score UI
        // exposing only two channels is a separate, authoring-only limit.)
        // AreaZero's `[M] Sound Manager.SetupSoundManager` loops 1..16 setting
        // `sound(i).volume` and raised "Invalid sound channel: 9" at eight.
        let sound_manager = SoundManager::new(16).expect("Sound manager failed to initialize");
        let now = chrono::Local::now();

        let mut result = DirPlayer {
            movie: Movie::empty(),
            nested_movie_images: HashMap::new(),
            net_manager: NetManager {
                owner_key: owner.key(),
                base_path: None,
                override_base_path: None,
                tasks: HashMap::new(),
                task_states: HashMap::new(),
                shared_state: Arc::new(Mutex::new(NetManagerSharedState::new())),
            },
            is_playing: false,
            is_script_paused: false,
            next_frame: None,
            queue_tx: tx,
            globals: FxHashMap::default(),
            scopes: Vec::with_capacity(MAX_STACK_SIZE),
            bytecode_handler_manager: StaticBytecodeHandlerManager {},
            breakpoint_manager: BreakpointManager::new(),
            current_breakpoint: None,
            step_mode: StepMode::None,
            step_scope_depth: 0,
            break_on_error: true,
            // ON so the browser e2e suite exercises the IR path. `set_ir_enabled`
            // still toggles it at runtime for A/B measurement.
            ir_enabled: true,
            stage_size: (100, 100),
            bitmap_manager: bitmap::manager::BitmapManager::new(),
            cursor: CursorRef::System(0),
            start_time: now, // supposed to be time at which computer started, but we don't have access from browser. this is sufficient for calculating elapsed time.
            timeout_manager: TimeoutManager::new(),
            title: "".to_string(),
            bg_color: ColorRef::Rgb(0, 0, 0),
            stage_draw_rect: None,
            stage_image: None,
            stage_image_dirty: false,
            center_stage: true,
            keyboard_focus_sprite: -1, // Setting keyboardFocusSprite to -1 returns keyboard focus control to the Score, and setting it to 0 disables keyboard entry into any editable sprite.
            mouse_loc: (0, 0),
            wants_pointer_lock: false,
            cursor_is_hidden: false,
            transform_sub_refs: Vec::new(),
            last_mouse_down_time: 0,
            is_double_click: false,
            mouse_down_sprite: 0,
            drag_offset: (0, 0),
            field_scroll_drag: None,
            trails_bitmap: None,
            subscribed_member_refs: vec![],
            is_subscribed_to_channel_names: false,
            is_subscribed_to_score: false,
            subscribed_cast_member_lists: HashSet::new(),
            font_manager: FontManager::new(),
            keyboard_manager: KeyboardManager::new(),
            text_selection_start: 0,
            text_selection_end: 0,
            clipboard_mirror: String::new(),
            ime_composition: None,
            float_precision: 4,
            last_handler_result: DatumRef::Void,
            hovered_sprites: Vec::new(),
            gif_animations: std::collections::HashMap::new(),
            picking_mode: false,
            owner: owner.clone(),
            w3d_dirty_transform_ids: HashSet::new(),
            allocator: DatumAllocator::from_owner(owner.clone()),
            dir_cache: HashMap::new(),
            scope_count: 0,
            scope_invalidation_epoch: 0,
            external_params: IndexMap::new(),
            xml_documents: HashMap::new(),
            xml_nodes: HashMap::new(),
            next_xml_id: 1000,
            sound_manager: sound_manager,
            date_objects: HashMap::new(),
            math_objects: HashMap::new(),
            click_on_sprite: 0,
            member_script_sprite_num: 0,
            pending_cue_events: Vec::new(),
            pending_player_notifications: Vec::new(),
            host_event_mailbox: HostEventMailbox::default(),
            host_event_backpressure: None,
            audio_sync_anchor: None,
            enable_stream_status_handler: false,
            stream_status_reported: HashMap::new(),
            is_in_frame_update: false,
            is_dispatching_events: false,
            is_in_send_all_sprites: false,
            tell_target_stack: Vec::new(),
            system_start_time: now - chrono::Duration::days(8), // Simulated system start
            handler_stack_depth: 0,
            in_havok_step_callback: false,
            in_frame_script: false,
            flash_frame_buffers: HashMap::new(),
            flash_sprite_loaded: HashSet::new(),
            flash_host_actions: Vec::new(),
            flash_host_is_nested: false,
            flash_host_route: FlashHostRoute::LocalOwned,
            flash_ready_sprites: HashSet::new(),
            flash_binding_state: Rc::new(RefCell::new(FlashBindingState::new())),
            flash_object_counter: 0,
            flash_scripted_access_pending: Rc::new(Cell::new(false)),
            flash_lc_connections: std::collections::HashMap::new(),
            flash_lc_callbacks: std::collections::HashMap::new(),
            input_polled: false,
            flash_texture_targets: HashMap::new(),
            w3d_frame_buffers: HashMap::new(),
            w3d_image_requested: std::collections::HashSet::new(),
            w3d_any_rendered: false,
            in_enter_frame: false,
            in_prepare_frame: false,
            in_step_frame: false,
            in_event_dispatch: false,
            event_stopped: false,
            command_handler_yielding: false,
            active_static_event_handlers: Vec::new(),
            last_kb_loop_frame_ms: 0.0,
            in_mouse_command: false,
            nothing_call_count: 0,
            last_nothing_yield_ms: 0.0,
            last_update_stage_yield_ms: 0.0,
            current_frame_tempo: 30, // Default to 30 fps
            has_player_frame_changed: false,
            stage_dirty: true,
            draw_hold_since_ms: None,
            preview_dirty: true,
            has_frame_changed_in_go: false,
            go_same_frame: false,
            go_direction: 0,
            is_getting_property_descriptions: false,
            is_initializing_behavior_props: false,
            last_initialized_frame: None,
            playback_init_count: 0,
            playback_frame_count: 0,
            playback_stop_count: 0,
            current_score_context: ScoreRef::Stage,
            debug_datum_refs: vec![],
            eval_scope_index: None,
            delay_until: None,
            pending_goto_net_movie: None,
            goto_wait_active: false,
            pending_movie_init: false,
            retired_cast_libs: Vec::new(),
            movie_mount_generation: 0,
            pending_restart: false,
            movie_reload_data: None,
            is_in_transition: false,
            pending_transition: None,
            score_transition_active: false,
            transition_hold_until_ms: None,
            actor_list_generation: 0,
            behavior_channel_cache_generation: 0,
            active_stage_filmloop_cache_generation: 0,
            script_text_overrides: FxHashMap::default(),
            cdp_check_modes: FxHashMap::default(),
            script_instance_list_cache: FxHashMap::default(),
            script_instance_list_cache_owner: FxHashMap::default(),
            script_instance_list_generation: FxHashMap::default(),
            script_instance_list_ids_cache: FxHashMap::default(),
            active_stage_behavior_channels_cache: None,
            active_stage_message_channels_cache: None,
            active_stage_filmloop_members_cache: None,
            last_sprite_prop_ref: None,
            virtual_scripts: FxHashMap::default(),
            xtra_manager_state: XtraManagerState::new(owner.clone()),
            scene3d_store: Scene3dStore::new(),
            movie_path_override: None,
            movie_path_label: None,
            startup_do: None,
            startup_do_before: None,
            startup_go: None,
            env_overrides: EnvOverrides::default(),
            console: console::ConsoleBuffer::new(),
            rng: rand::rngs::SmallRng::seed_from_u64(0),
        };

        result.initialize_new_player_state();
        result
    }

    /// Pure initialization for a newly-owned player. This only creates VM
    /// globals and call-stack scopes; it does not notify JS, clear ambient
    /// runtimes, cancel host requests, or advance allocator generations.
    fn initialize_new_player_state(&mut self) {
        self.initialize_globals();
        for i in 0..MAX_STACK_SIZE {
            self.scopes.push(Scope::default(i));
        }
    }

    /// Pre-dispatch Flash members for all active sprites so they start loading
    /// before Lingo scripts try to access them. Per-sprite: each sprite that
    /// references a Flash member gets its own dedicated Ruffle instance.
    pub fn pre_dispatch_flash_members(&mut self) -> Result<(), ScriptError> {
        // When running a nested `#movie` sub-player, dispatch its Flash sprites
        // to Ruffle under a synthetic per-player key so they don't collide with
        // the host's channel keys and their captured frames route back into THIS
        // sub's `flash_frame_buffers` (see NESTED_FLASH_BASE / update_flash_frame).
        let active = unsafe { ACTIVE_PLAYER_ID };
        let route = self.flash_host_route;
        if route == FlashHostRoute::NestedPending {
            return Err(ScriptError::new_code(
                ScriptErrorCode::InvalidReference,
                "nested Flash host registration is pending".to_owned(),
            ));
        }
        self.reconcile_flash_binding_origins();
        let js_flash_key =
            |ch: i16| flash_host_sprite_key(route == FlashHostRoute::LegacySynthetic, active, ch);
        // UNLOAD pass FIRST: tear down any Ruffle instance whose channel no
        // longer holds that exact Flash member — BEFORE the load pass below, so
        // a member swap is deterministically unload(old) → load(new). If the
        // load ran first, `createFlashInstance(new)` sets the new instance at
        // the sprite's key, and the subsequent unload(old) — same key —
        // destroys the freshly-created instance, so the swapped-in member
        // (bogey_nights' bogeyman straw/longarm) never renders (frame stays 0,
        // "stale straw"). Also frees instances when a channel empties
        // (splash `killer`) so we don't leak players.
        let loaded: Vec<(i16, i32, i32)> = self.flash_sprite_loaded.iter().cloned().collect();
        for (ch, cl, cm) in loaded {
            let cur = self
                .movie
                .score
                .get_sprite(ch)
                .and_then(|s| s.member.as_ref())
                .map(|m| (m.cast_lib, m.cast_member));
            let still_present = cur == Some((cl, cm)) && self.channel_holds_live_flash(ch);
            if still_present {
                continue;
            }
            // The channel no longer holds THIS member. A flash→flash swap
            // leaves both the old and new member entries in the bookkeeping
            // set for one pass. A retired generation owns its own teardown
            // authority. Queue the
            // exact old generation even when a replacement Flash member is
            // already live on this channel; the generation-qualified host
            // route must leave that replacement untouched.
            let owner_key = owner_key_string(&self.owner);
            let generations = self.retire_or_get_flash_unload_generations(ch, cl, cm);
            match route {
                FlashHostRoute::LocalOwned => {
                    for generation in generations {
                        self.queue_flash_host_action(FlashHostAction::Unload {
                            fence: self.flash_action_fence(),
                            host_sprite: ch as i32,
                            local_sprite: ch,
                            cast_lib: cl,
                            cast_member: cm,
                            generation,
                        });
                    }
                }
                FlashHostRoute::LegacySynthetic if !self.channel_holds_live_flash(ch) => {
                    JsApi::dispatch_flash_member_unloaded(js_flash_key(ch), &owner_key);
                }
                FlashHostRoute::LegacySynthetic | FlashHostRoute::NestedPending => {}
            }
            if !self.channel_holds_live_flash(ch) {
                self.flash_frame_buffers.remove(&ch);
            }
            self.flash_sprite_loaded.remove(&(ch, cl, cm));
        }

        // LOAD pass: dispatch newly-present Flash members.
        for channel_index in 0..self.movie.score.channels.len() {
            let (channel_num, member_ref, asserted_frame, rect) = {
                let channel = &self.movie.score.channels[channel_index];
                (
                    channel.number as i16,
                    channel.sprite.member.clone(),
                    channel.sprite.flash_asserted_frame,
                    channel.sprite.member.as_ref().map(|_| {
                        crate::player::score::get_concrete_sprite_rect(self, &channel.sprite)
                    }),
                )
            };
            if let Some(member_ref) = member_ref {
                let dispatch_key = (channel_num, member_ref.cast_lib, member_ref.cast_member);
                if self.flash_sprite_loaded.contains(&dispatch_key) {
                    // Already loaded: keep the Ruffle render resolution matched
                    // to the sprite's current on-stage size so a Flash sprite
                    // scaled up on stage (bogey_nights' boogyflash/spitflash
                    // splashes GROW; the bogeyman arm swaps member dims) stays
                    // sharp — Ruffle re-renders the vector at the new size
                    // instead of dirplayer upscaling a stale small capture.
                    // setFlashSize no-ops on sub-2px changes so static sprites
                    // (StoryScramble posters) never reflow. Only for on-stage
                    // sprites (positive channel); 3D-texture instances excluded
                    // in the JS twin.
                    // Match the Ruffle render resolution to the RESOLVED sprite
                    // rect (member natural size when un-stretched), not the raw
                    // score cell — otherwise a 626x100 member in a 100x320 cell
                    // captures at the wrong aspect and resamples to a thin strip.
                    let rect = rect.expect("Flash member has a resolved sprite rectangle");
                    let width = rect.width().max(1) as u32;
                    let height = rect.height().max(1) as u32;
                    match route {
                        FlashHostRoute::LocalOwned => self.queue_flash_resize(
                            channel_num as i32,
                            channel_num,
                            member_ref.cast_lib,
                            member_ref.cast_member,
                            width,
                            height,
                        ),
                        FlashHostRoute::LegacySynthetic => {
                            let _ = ruffle_set_size(
                                js_flash_key(channel_num),
                                width as i32,
                                height as i32,
                            );
                        }
                        FlashHostRoute::NestedPending => unreachable!(),
                    }
                    continue;
                }
                if let Some(member) = self.movie.cast_manager.find_member_by_ref(&member_ref) {
                    if let CastMemberType::Flash(flash_member) = &member.member_type {
                        if crate::rendering::has_swf_signature(&flash_member.data) {
                            let data = flash_member.data.clone();
                            // Capture at the resolved sprite rect (member natural
                            // size when un-stretched), not the raw score cell.
                            let rect = rect.expect("Flash member has a resolved sprite rectangle");
                            let w = rect.width().max(1) as u32;
                            let h = rect.height().max(1) as u32;
                            let paused_at_start = flash_member
                                .flash_info
                                .as_ref()
                                .map(|fi| fi.paused_at_start)
                                .unwrap_or(false);
                            // Re-project the sprite's asserted frame onto the new
                            // instance so shared-member siblings show unique
                            // posters and swaps keep their frame.
                            let asserted_frame = asserted_frame.unwrap_or(-1);
                            debug!(
                                "[Flash] Pre-dispatching sprite#{} {}:{} ({}x{}, {} bytes, pausedAtStart={}, assertedFrame={})",
                                channel_num,
                                member_ref.cast_lib,
                                member_ref.cast_member,
                                w,
                                h,
                                data.len(),
                                paused_at_start,
                                asserted_frame,
                            );
                            match route {
                                FlashHostRoute::LocalOwned => {
                                    self.queue_flash_member_load(
                                        channel_num as i32,
                                        channel_num,
                                        member_ref.cast_lib,
                                        member_ref.cast_member,
                                        data,
                                        w,
                                        h,
                                        paused_at_start,
                                        asserted_frame,
                                    )?;
                                }
                                FlashHostRoute::LegacySynthetic => {
                                    JsApi::dispatch_flash_member_loaded(
                                        js_flash_key(channel_num),
                                        member_ref.cast_lib,
                                        member_ref.cast_member,
                                        &data,
                                        w,
                                        h,
                                        paused_at_start,
                                        asserted_frame,
                                        &owner_key_string(&self.owner),
                                    );
                                }
                                FlashHostRoute::NestedPending => unreachable!(),
                            }
                            if route == FlashHostRoute::LegacySynthetic {
                                self.flash_sprite_loaded.insert(dispatch_key);
                            }
                        }
                    }
                }
            }
        }

        // LOOP=false stop-at-end pass. A Flash member with `loop` disabled plays
        // once and halts at its last frame (Director: `the playing of sprite`
        // then becomes FALSE — eds_kart_attack's "Wait for Flash" behavior loops
        // the Director frame until sprite(1).playing = 0). Ruffle, however, loops
        // the root timeline forever when the SWF has no internal stop(). So for
        // loop-disabled Flash sprites, watch the root playhead and, once it
        // reaches the last frame (or wraps past it), gotoAndStop the final frame.
        let loop_check: Vec<(i16, i32)> = self
            .movie
            .score
            .channels
            .iter()
            .filter_map(|channel| {
                let cn = channel.number as i16;
                let member_ref = channel.sprite.member.as_ref()?;
                let member = self.movie.cast_manager.find_member_by_ref(member_ref)?;
                if let CastMemberType::Flash(f) = &member.member_type {
                    let loops = f.flash_info.as_ref().map_or(true, |i| i.loop_enabled);
                    if !loops {
                        let total =
                            crate::player::cast_member::CastMember::parse_swf_frame_count(&f.data)
                                .map(|n| n as i32)
                                .unwrap_or(0);
                        if total > 1 {
                            return Some((cn, total));
                        }
                    }
                }
                None
            })
            .collect();
        for (cn, total) in loop_check {
            // Bridge not installed (test harness / pre-init) → treat as "not
            // playing" and skip; the `catch` keeps the frame loop alive.
            if !ruffle_is_playing(cn as i32).unwrap_or(false) {
                continue;
            }
            let cur = ruffle_get_current_frame(cn as i32).unwrap_or(0);
            if cur < 1 {
                continue;
            }
            let prev = self
                .movie
                .score
                .get_sprite(cn)
                .map(|s| s.flash_prev_frame)
                .unwrap_or(0);
            // Ruffle loops a stop-less SWF from the LAST frame back to 1, so a
            // genuine wrap always STARTS near the end of the timeline. A Lingo
            // seek jumps backwards too, but from an arbitrary frame — the
            // mission popup's `goToFrame("diamondhead")` lands on frame 1 from
            // wherever the SWF autoplayed to during load. Reading that as
            // end-of-timeline yanks the sprite to its final frame and kills the
            // animation the script just cued up. Requiring `prev` to be in the
            // tail keeps the real overshoot case working: we sample once per
            // Director frame while the SWF runs faster, so the exact last frame
            // is often skipped and only the wrap is observable.
            let tail = (total / 8).max(4);
            let wrapped = prev >= 1 && cur < prev && prev >= total - tail;
            if cur >= total || wrapped {
                let owner_key = owner_key_string(&self.owner);
                let _ = ruffle_goto_frame_and_stop_owned(&owner_key, cn as i32, &total.to_string());
                // …and actually HALT it. `goToFrameAndStop` carries the
                // `sprite.frame = N` SETTER semantics: it only pins when the
                // member is `pausedAtStart`. For an animated member (the usual
                // case, and what `loop = false` members are) it seeks and KEEPS
                // PLAYING — so the SWF ran on past the last frame, wrapped,
                // tripped this check again and re-parked, forever. battleready's
                // 70-frame intro_anim did that 672 times, blinking black on
                // every re-seek. Halting the root timeline is what `loop = false`
                // actually means, and it latches the `ruffle_is_playing` gate
                // above so this runs once instead of every frame.
                ruffle_stop(cn as i32);
            }
            self.movie.score.get_sprite_mut(cn).flash_prev_frame = cur;
        }
        Ok(())
    }

    /// True if the given score channel currently holds a Flash (SWF) cast
    /// member — i.e. there should be exactly one live Ruffle instance keyed by
    /// this channel number. Used by the reconcile to distinguish a flash→flash
    /// member swap (keep the instance `createFlashInstance` just made) from a
    /// flash→non-flash/empty change (genuinely tear the instance down).
    fn channel_holds_live_flash(&self, ch: i16) -> bool {
        self.movie
            .score
            .get_sprite(ch)
            .and_then(|s| s.member.as_ref())
            .and_then(|mref| self.movie.cast_manager.find_member_by_ref(mref))
            .map(|member| match &member.member_type {
                CastMemberType::Flash(f) => crate::rendering::has_swf_signature(&f.data),
                _ => false,
            })
            .unwrap_or(false)
    }

    /// Tear down any Flash (Ruffle) instances whose source member lives in the
    /// given cast lib, so the renderer re-creates them from the member's CURRENT
    /// bytes.
    ///
    /// Director keeps a member ref stable across an external-cast swap, but the
    /// bytes behind it change: Storyscramble's `castLib("story").fileName =
    /// nextStory.castFile` reloads cast lib 2 in place, so member 2:1 (the story
    /// SWF the tiles render) is replaced while the sprites still point at 2:1.
    /// The lazy-load gate (`flash_sprite_loaded`) and the captured-frame buffer
    /// would otherwise keep the STALE Ruffle player on screen forever. Clearing
    /// both — and destroying the JS-side player (which also stops its capture
    /// RAF) — makes the next render see `flash_bitmap_ref.is_none()` and
    /// re-dispatch `createFlashInstance` with the new story's SWF.
    pub fn invalidate_flash_for_cast_lib(&mut self, cast_lib: i32) {
        let loaded: Vec<(i16, i32, i32)> = self
            .flash_sprite_loaded
            .iter()
            .filter(|(_, cl, _)| *cl == cast_lib)
            .map(|(sn, cl, cm)| (*sn, *cl, *cm))
            .collect();
        let mut sprites = loaded.clone();
        sprites.extend(
            self.current_flash_bindings_for_cast_lib(cast_lib)
                .into_iter()
                .map(|(sn, cl, cm, _)| (sn, cl, cm)),
        );
        sprites.sort_unstable();
        sprites.dedup();
        if sprites.is_empty() {
            return;
        }
        self.flash_sprite_loaded
            .retain(|(_, cl, _)| *cl != cast_lib);
        for (sn, cl, cm) in sprites {
            // Destroy the JS-side Ruffle instance first (cancels its capture
            // RAF so it can't re-insert a frame buffer after we drop it).
            if self.flash_host_route == FlashHostRoute::LegacySynthetic {
                if loaded.contains(&(sn, cl, cm)) {
                    JsApi::dispatch_flash_member_unloaded(
                        sn as i32,
                        &owner_key_string(&self.owner),
                    );
                }
            } else {
                for generation in self.retire_or_get_flash_unload_generations(sn, cl, cm) {
                    self.queue_flash_host_action(FlashHostAction::Unload {
                        fence: self.flash_action_fence(),
                        host_sprite: sn as i32,
                        local_sprite: sn,
                        cast_lib: cl,
                        cast_member: cm,
                        generation,
                    });
                }
            }
            self.flash_frame_buffers.remove(&sn);
        }
    }

    pub async fn load_movie_from_file(
        &mut self,
        path: &str,
        symbols: &mut SymbolTable,
    ) -> Result<(), String> {
        let task_id = self.net_manager.preload_net_thing(path.to_owned());
        self.net_manager.await_task(task_id).await;
        let task = self
            .net_manager
            .get_task(task_id)
            .ok_or_else(|| format!("Network task not found for '{}'", path))?;
        let data_bytes = self
            .net_manager
            .get_task_result(Some(task_id))
            .ok_or_else(|| format!("No response received for '{}'", path))?
            .map_err(|_| format!("Network request failed for '{}'", path))?;

        let file_name = task
            .resolved_url
            .path_segments()
            .and_then(|segments| segments.last())
            .unwrap_or("untitled.dcr");

        let base_url = get_base_url(&task.resolved_url).to_string();
        let file_name_owned = file_name.to_string();
        let movie_file = read_director_file_bytes(&data_bytes, &file_name, &base_url)
            .map_err(|e| format!("Failed to parse movie file '{}': {}", path, e))?;
        // Retain the raw bytes so a `play movie <current>` restart can re-parse and
        // rebuild the cast (the net loader often can't re-fetch by name once loaded).
        self.movie_reload_data = Some((data_bytes, file_name_owned, base_url));
        self.load_movie_from_dir(movie_file, symbols).await;
        Ok(())
    }

    /// Instantiate a full nested `DirPlayer` for a Linked `#movie` member whose
    /// linked bytes are loaded, register it in `NESTED_PLAYERS`, and start it
    /// (load + play + its own command loop) on its own active-player id. The sub
    /// runs the entire engine against itself — its own scripts/score/cast — via
    /// the active-player indirection; the loader keeps running independently.
    /// Synchronous: the async load+play happens in a spawned task bound to the
    /// sub's id (a manual `ACTIVE_PLAYER_ID` set can't span an await here, since
    /// the enclosing task's `WithActivePlayer` wrapper would restore it).
    pub fn spawn_nested_player(&self, member_ref: CastMemberRef) {
        if nested_player_id(&member_ref).is_some() {
            return;
        }
        let (bytes, base_url, file_name) = match self
            .movie
            .cast_manager
            .find_member_by_ref(&member_ref)
            .and_then(|m| {
                if let CastMemberType::Movie(mv) = &m.member_type {
                    mv.bytes.as_ref().map(|b| {
                        let fname = mv
                            .file_name
                            .rsplit(|c| c == '/' || c == '\\')
                            .next()
                            .filter(|s| !s.is_empty())
                            .unwrap_or("nested.dcr")
                            .to_string();
                        (b.clone(), mv.base_url.clone(), fname)
                    })
                } else {
                    None
                }
            }) {
            Some(t) => t,
            None => return,
        };
        let dir =
            match crate::director::file::read_director_file_bytes(&bytes, &file_name, &base_url) {
                Ok(d) => d,
                Err(e) => {
                    warn!("[nested-player] parse '{}' failed: {}", file_name, e);
                    return;
                }
            };
        let (tx, rx) = async_std::channel::unbounded();
        // The sub gets its OWN event channel (parallel to its command channel) so
        // its events are processed by its own event loop with ACTIVE_PLAYER_ID =
        // its id — otherwise a sub's script-instance ids resolve against the host
        // allocator and panic (see NESTED_EVENT_TX).
        let (event_tx, event_rx) = async_std::channel::unbounded();
        let session_handle = retained_session_handle()
            .expect("legacy nested startup requires its owning RuntimeSession");
        let id = unsafe {
            let id = NESTED_PLAYERS.len() + 1;
            // Legacy callers reach this path through the retained session;
            // PLAYER_OPT no longer has a standalone production writer.
            assert!(
                session_handle
                    .borrow_mut()
                    .add_player(id as u32, tx.clone())
            );
            session_handle
                .borrow_mut()
                .with_player(id as u32, |context| {
                    context.player.set_legacy_flash_host_route();
                })
                .expect("legacy nested player must be present in RuntimeSession");
            NESTED_PLAYERS.push(None);
            NESTED_PLAYER_KEYS.push(Some(member_ref.clone()));
            NESTED_EVENT_TX.push(Some(event_tx));
            id
        };
        // The sub never receives the frontend's `SetSystemFontPath` command, so
        // its font_manager has no system font — text whose font isn't a cast
        // member (Verdana etc.) falls back to `get_system_font()` and, finding
        // none, fails to render ("No font found for 'Verdana'"). Inherit the
        // host's system font (a shared `Rc<BitmapFont>`, cheap to clone).
        let host_system_font = self.font_manager.system_font.clone();
        let prev = unsafe { ACTIVE_PLAYER_ID };
        // Bind the load/play task (and everything it spawns, incl. the sub's
        // frame loop via play()) to the sub's id; spawn_player_local captures
        // the id set right here.
        unsafe {
            ACTIVE_PLAYER_ID = id;
        }
        crate::player::spawn_player_local(async move {
            reserve_player_mut_async(|p| {
                Box::pin(async move {
                    // This legacy nested-player path predates RuntimeSession's
                    // explicit context API. The load body is synchronous; keep
                    // the compatibility call local until the nested frontend is
                    // fully owner-bound.
                    let mut symbols = SymbolTable::new();
                    p.load_movie_from_dir(dir, &mut symbols).await;
                })
            })
            .await;
            if host_system_font.is_some() {
                reserve_player_mut(|p| {
                    if p.font_manager.system_font.is_none() {
                        p.font_manager.system_font = host_system_font.clone();
                    }
                });
            }
            reserve_player_mut(|p| p.play());
            let owner = session_handle
                .borrow_mut()
                .with_player(id as u32, |context| context.player.owner.clone())
                .expect("nested player must be present in RuntimeSession");
            let event_session = session_handle.clone();
            let event_owner = owner.clone();
            crate::player::spawn_player_local(crate::player::commands::run_command_loop(
                rx,
                session_handle,
                id as u32,
                owner,
            ));
            crate::player::spawn_player_local(crate::player::events::run_event_loop(
                event_rx,
                event_session,
                id as u32,
                event_owner,
            ));
            let (rw, rh, ver) = reserve_player_ref(|p| {
                (
                    p.movie.rect.width(),
                    p.movie.rect.height(),
                    p.movie.dir_version,
                )
            });
            debug!(
                "[nested-player] id={} started '{}' dir_version={} rect={}x{}",
                id, file_name, ver, rw, rh
            );
        });
        unsafe {
            ACTIVE_PLAYER_ID = prev;
        }
    }

    /// Scan the host movie's on-stage sprites for Linked Movie (`#movie`)
    /// members whose linked bytes are loaded, and start a nested `DirPlayer` for
    /// any not yet running. Called each frame so assigning a #movie member to a
    /// sprite (DGS `sprite(spShk).member = member(gMember)`) activates playback.
    pub fn activate_nested_players(&self) {
        let to_build: Vec<CastMemberRef> = self
            .movie
            .score
            .channels
            .iter()
            .filter_map(|channel| channel.sprite.member.clone())
            .filter(|m| nested_player_id(m).is_none())
            .filter(|m| {
                matches!(
                    self.movie.cast_manager.find_member_by_ref(m).map(|mem| &mem.member_type),
                    Some(CastMemberType::Movie(mv)) if mv.bytes.is_some()
                )
            })
            .collect();
        for member_ref in to_build {
            self.spawn_nested_player(member_ref);
        }
    }

    /// Render each on-stage nested `#movie` sub-player's stage (headless, CPU)
    /// and copy it into THIS (host) player's `bitmap_manager`, keyed by member,
    /// so the WebGL2 `Movie` sprite arm can blit it. Called on the host each
    /// frame. The sub-player renders from its own score/cast/bitmap_manager under
    /// its active-player id; the resulting stage bitmap is a plain owned `Bitmap`
    /// which is then stored in the host's manager (a separate manager, so the
    /// pixels are copied across — no id collision).
    pub fn render_nested_player_stages(&mut self) {
        self.drain_allocator_reclaims();
        use std::collections::HashSet;
        let refs: Vec<CastMemberRef> = self
            .movie
            .score
            .channels
            .iter()
            .filter_map(|ch| ch.sprite.member.clone())
            .filter(|m| nested_player_id(m).is_some())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        for member_ref in refs {
            let id = match nested_player_id(&member_ref) {
                Some(i) => i,
                None => continue,
            };
            // Render the sub-player headlessly. Set ACTIVE_PLAYER_ID = id so any
            // internal reserve_player_* inside the renderer resolves to the sub;
            // no await here, so the manual set is safe. `self` (host) and the sub
            // are distinct DirPlayers, so the &mut aliasing is only the usual
            // raw-pointer re-entrancy the engine already relies on.
            let prev = unsafe { ACTIVE_PLAYER_ID };
            let rendered = unsafe {
                ACTIVE_PLAYER_ID = id;
                let out = if let Some(handle) =
                    PLAYER_SESSION_HANDLE.with(|slot| slot.borrow().clone())
                {
                    handle
                        .borrow_mut()
                        .with_player(id as u32, |mut context| {
                            render_nested_player_bitmap(context.player, Some(context.symbols), id)
                        })
                        .flatten()
                } else {
                    NESTED_PLAYERS
                        .get_mut(id - 1)
                        .and_then(|slot| slot.as_mut())
                        .and_then(|sub| render_nested_player_bitmap(sub, None, id))
                };
                ACTIVE_PLAYER_ID = prev;
                out
            };
            // Store into the host's bitmap_manager (ACTIVE is back to host).
            if let Some(bmp) = rendered {
                let (w, h) = (bmp.width, bmp.height);
                let reuse = self.nested_movie_images.get(&member_ref).copied().filter(|r| {
                    matches!(self.bitmap_manager.get_bitmap(*r), Some(b) if b.width == w && b.height == h)
                });
                match reuse {
                    Some(r) => {
                        // `replace_bitmap` bumps the bitmap version so the WebGL
                        // texture cache re-uploads this frame's content. A plain
                        // `*slot = bmp` reset version to 0, so the composite went
                        // STALE after the first frame (only refreshing when the
                        // texture was LRU-evicted) — that was the hover-highlight
                        // "flickers to the state where it should be showing".
                        self.bitmap_manager.replace_bitmap(r, bmp);
                    }
                    None => {
                        let r = self.bitmap_manager.add_bitmap(bmp);
                        self.nested_movie_images.insert(member_ref.clone(), r);
                    }
                }
            }
        }
    }

    /// The `cursor` setting belongs to the movie that made it: a new movie
    /// starts with the system arrow. A task movie that hid the cursor for a
    /// drag otherwise left the next movie without one.
    pub fn reset_cursor_for_new_movie(&mut self) {
        self.cursor = CursorRef::System(0);
        self.cursor_is_hidden = false;
        self.wants_pointer_lock = false;
    }

    fn load_movie_from_dir_sync_with_options(
        &mut self,
        dir: DirectorFile,
        symbols: &mut SymbolTable,
        begin_sprites: bool,
    ) {
        // Start this movie from the builtin display-spelling baseline. A cast's
        // name table claims the spelling for symbols it defines
        // (`Symbol::from_str_authoritative`), and Director resets that per movie.
        // Our interner is process-global and monotonic, so without this reset the
        // first movie loaded would claim spellings for every movie after it —
        // load-order-dependent behaviour, which matters for the e2e suite (~48
        // movies in one process) and for any session that changes movie.
        // Pick the platform-correct default system palette before loading. Mac
        // movies (Director 4 titles like thead) default to System-Mac, Windows
        // movies to System-Win; these differ at high indices and decide how
        // indexed bitmaps / shape pattern fills resolve. Read before `dir` moves.
        crate::player::bitmap::bitmap::set_default_system_palette_from_platform(
            dir.config.platform,
        );
        // The GIF animations belong to the cast that is going away. Left in
        // place, their keys land on whatever the next movie keeps at those
        // member numbers: the map's planet and smoke frames turned up on a
        // task scene's craftsman and plank piles.
        crate::player::gif::forget_all(self);
        self.reset_cursor_for_new_movie();
        self.movie.load_from_file(
            dir,
            &mut self.net_manager,
            &mut self.bitmap_manager,
            &mut self.dir_cache,
            symbols,
        );
        self.queue_player_notification(PlayerNotificationKind::ScoreChanged);
        if self.is_subscribed_to_channel_names {
            let channels: Vec<i16> = self
                .movie
                .score
                .channels
                .iter()
                .map(|channel| channel.number as i16)
                .collect();
            for channel in channels {
                self.queue_player_notification(PlayerNotificationKind::ChannelNameChanged(channel));
            }
        }

        // Apply fake movie path override if set (moviePath/movieName use
        // this, but net_manager.base_path stays real for actual file
        // fetching).
        //
        // Three sources, listed by precedence:
        //   1. `movie_path_label` (set_movie_path_label JS API) —
        //      label-only, no URL rewrite.
        //   2. external_params["_moviePath"] — same semantics as #1; just
        //      a more declarative way to set it (drop a key in the
        //      externalParams the host passes to dirplayer).
        //   3. `movie_path_override` (set_movie_path_override JS API) —
        //      rewrite-mode: registers `net_manager.override_base_path`
        //      so URLs the script builds get translated back to the real
        //      base before they're fetched.
        //
        // The first two are "label-only"; the third is the legacy
        // rewrite path. Only the rewrite path registers an
        // override_base_path with the net manager.
        let label_value: Option<String> = self
            .movie_path_label
            .as_ref()
            .filter(|s| !s.is_empty())
            .cloned()
            .or_else(|| {
                self.external_param_ci("_moviePath")
                    .filter(|s| !s.is_empty())
            });
        let path_to_apply: Option<(String, bool /* is_label */)> =
            label_value.map(|s| (s, true)).or_else(|| {
                self.movie_path_override
                    .as_ref()
                    .filter(|s| !s.is_empty())
                    .map(|s| (s.clone(), false))
            });
        if let Some((path, is_label)) = path_to_apply {
            if let Ok(url) = Url::parse(&path) {
                self.movie.base_path = get_base_url(&url).to_string();
                if let Some(name) = url.path_segments().and_then(|s| s.last()) {
                    self.movie.file_name = name.to_string();
                }
            } else {
                // Treat as a plain path
                if let Some(pos) = path.rfind('/') {
                    self.movie.base_path = path[..pos].to_string();
                    self.movie.file_name = path[pos + 1..].to_string();
                } else {
                    self.movie.file_name = path.clone();
                }
            }
            if !is_label {
                self.net_manager.override_base_path = Some(self.movie.base_path.clone());
            }
        }

        self.bg_color = self.movie.stage_color_ref.clone();

        // Load all fonts from cast members into the font manager
        log::debug!("Loading fonts from cast members...");
        self.movie
            .cast_manager
            .load_fonts_into_manager(&mut self.font_manager);

        // A nested `#movie` sub-player is headless — it must never resize the
        // shared WebGL2 renderer (that canvas belongs to the host stage). Only
        // the host player (active id 0) owns the on-screen renderer size.
        if unsafe { ACTIVE_PLAYER_ID } == 0 {
            with_renderer_mut(|renderer_opt| {
                if let Some(renderer) = renderer_opt {
                    use crate::rendering_gpu::Renderer;
                    let (stage_w, stage_h) = crate::player::stage::stage_canvas_dims(self);
                    renderer.set_size(stage_w, stage_h);
                }
            });
        }

        let (stage_w, stage_h) = crate::player::stage::stage_canvas_dims(self);
        if let Err(error) =
            self.queue_host_event(crate::player::host_events::HostEvent::StageSizeChanged {
                width: stage_w,
                height: stage_h,
                center: self.center_stage,
            })
        {
            log::error!("host event mailbox overflow: {:?}", error);
        }
        let movie = self.movie.file.as_ref().unwrap();
        if let Err(error) =
            self.queue_host_event(crate::player::host_events::HostEvent::MovieLoaded {
                version: movie.version,
                cast_names: movie
                    .cast_entries
                    .iter()
                    .map(|cast| cast.name.clone())
                    .collect(),
            })
        {
            log::error!("host event mailbox overflow: {:?}", error);
        }

        // Register built-in virtual scripts
        virtual_scripts::register_virtual_scripts(self);

        if begin_sprites {
            self.begin_all_sprites(symbols);
        }
        if let Err(error) =
            self.queue_host_event(crate::player::host_events::HostEvent::FrameChanged {
                frame: self.movie.current_frame,
            })
        {
            log::error!("host event mailbox overflow: {:?}", error);
        }
    }

    pub(crate) fn load_movie_from_dir_sync(
        &mut self,
        dir: DirectorFile,
        symbols: &mut SymbolTable,
    ) {
        self.load_movie_from_dir_sync_with_options(dir, symbols, true);
    }

    /// Async compatibility entrypoint for callers that already own a direct
    /// player borrow. The actual load body is synchronous, so owner-bound
    /// session callers can invoke `load_movie_from_dir_owned` without holding
    /// a session borrow across an await.
    pub(crate) async fn load_movie_from_dir(
        &mut self,
        dir: DirectorFile,
        symbols: &mut SymbolTable,
    ) {
        self.load_movie_from_dir_sync(dir, symbols);
    }

    /// Load a parsed Director file through the session that owns its player.
    /// The owner and player id are captured by the frontend before any async
    /// work; only the short synchronous mutation borrow is taken here.
    pub async fn load_movie_from_dir_owned(
        session: RuntimeSessionHandle,
        player_id: u32,
        owner: OwnerToken,
        dir: DirectorFile,
    ) -> Result<(), ScriptError> {
        // The projector's --doBefore hook runs against the owning runtime
        // before any cast bytes are installed.  This lets the payload set
        // preload policy and other globals that affect cast loading.
        run_startup_do_owned(session.clone(), player_id, owner.clone(), true).await?;
        let loaded = session
            .borrow_mut()
            .with_player(player_id, |mut context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(cancelled_scope_error());
                }
                context
                    .player
                    .load_movie_from_dir_sync_with_options(dir, context.symbols, false);
                Ok(())
            })
            .ok_or_else(cancelled_scope_error)??;
        let _ = crate::js_api::JsApi::dispatch_player_notifications(session.clone(), player_id);
        let cast_count = session
            .borrow_mut()
            .with_player(player_id, |context| {
                context.player.movie.cast_manager.casts.len()
            })
            .ok_or_else(cancelled_scope_error)?;
        for cast_lib in 1..=cast_count as u32 {
            cast_lib::CastLib::install_pending_js_registrations(
                session.clone(),
                player_id,
                owner.clone(),
                cast_lib,
            )?;
        }
        session
            .borrow_mut()
            .with_player(player_id, |mut context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(cancelled_scope_error());
                }
                context.player.begin_all_sprites(context.symbols);
                Ok(())
            })
            .ok_or_else(cancelled_scope_error)??;
        Ok(loaded)
    }

    pub fn play(&mut self) {
        if self.is_playing {
            return;
        }
        self.is_playing = true;
        self.is_script_paused = false;

        use crate::js_api::safe_string;
        debug!(
            "Loading Movie: {} (version: {})",
            safe_string(&self.movie.file_name),
            self.movie.dir_version
        );

        crate::player::spawn_player_local(async move {
            run_movie_init_sequence().await;
            run_frame_loop().await;
        });
    }

    /// Recompute the cached frame tempo and republish it to JS.
    ///
    /// MUST run every frame, not only on a frame CHANGE. `begin_all_sprites` is
    /// skipped while the playhead stays on the same frame (`stayed_on_same_frame`
    /// in `run_single_frame`, and the same test in `go`), and holding a frame with
    /// `go the frame` is the standard Director idiom — every game in the corpus
    /// does it. Refreshing the cache only from there meant `puppetTempo()` could
    /// update `movie.puppet_tempo`, and `get_effective_tempo()` would happily
    /// report the new value, while the frame loop went on pacing from a
    /// `current_frame_tempo` frozen at whatever the tempo was when the frame was
    /// first entered — so puppetTempo simply had NO EFFECT on a looping frame.
    /// Measured with `docs/fps-probe`: a movie whose authored frame_rate is 1
    /// stayed at 1.00 fps (1002 ms/frame) through puppetTempo 30/60/120/999,
    /// where Shockwave gives 30/60/120/1000.
    pub fn refresh_frame_tempo(&mut self) {
        self.current_frame_tempo = self.movie.get_effective_tempo();

        // Publish it for the JS side. flashPlayerManager's Ruffle frame capture
        // paces itself to this: capturing faster than the stage redraws is pure
        // waste, and each capture is a full GPU→CPU `getImageData` readback.
        //
        // wasm32 only. `web_sys::window()` reaches a wasm-bindgen imported static,
        // and on a native target that PANICS ("cannot access imported statics on
        // non-wasm targets") rather than returning None — so an unguarded call
        // here takes down every native e2e test on the first `begin_all_sprites`.
        #[cfg(target_arch = "wasm32")]
        {
            if let Some(window) = web_sys::window() {
                let _ = js_sys::Reflect::set(
                    &window,
                    &wasm_bindgen::JsValue::from_str("__dirplayerFrameTempo"),
                    &wasm_bindgen::JsValue::from_f64(self.current_frame_tempo as f64),
                );
            }
        }
    }

    pub fn begin_all_sprites(&mut self, symbols: &mut SymbolTable) {
        self.begin_score_sprites(ScoreRef::Stage, self.movie.current_frame, symbols);

        // Cache the tempo for this frame
        self.refresh_frame_tempo();

        // If the player isn't playing yet (i.e., during initial load),
        // reset the entered flags so that beginSprite will be called again
        // when the movie actually starts playing with the fixed spriteNum
        if !self.is_playing {
            for channel in &mut self.movie.score.channels {
                channel.sprite.entered = false;
                channel.sprite.script_instance_list.clear();
                // The properties HAVE been applied — only the behavior
                // lifecycle is being rewound. Mark them so the second pass
                // (which runs after prepareMovie) re-enters the span without
                // overwriting anything prepareMovie changed.
                if channel.sprite.member.is_some() {
                    channel.sprite.score_props_already_applied = true;
                }
            }
            self.clear_script_instance_list_caches();
        }

        self.invalidate_active_stage_filmloop_cache();
        let active_filmloops = self.active_stage_filmloop_member_refs();
        for member_ref in active_filmloops {
            let current_frame = match self
                .movie
                .cast_manager
                .find_member_by_ref(&member_ref)
                .and_then(|m| m.member_type.as_film_loop())
            {
                Some(fl) => fl.current_frame,
                None => continue,
            };
            // Use filmloop's own current_frame instead of movie's current_frame
            self.begin_score_sprites(
                ScoreRef::FilmLoop(member_ref.clone()),
                current_frame,
                symbols,
            );
            if let Some(film_loop) = self
                .movie
                .cast_manager
                .find_mut_member_by_ref(&member_ref)
                .and_then(|m| m.member_type.as_film_loop_mut())
            {
                film_loop.score.apply_tween_modifiers(current_frame);
            }
        }

        self.invalidate_behavior_channel_cache();
    }

    pub async fn end_all_sprites(&mut self) -> Vec<(ScoreRef, u32)> {
        let next_frame = self.get_next_frame();
        let mut all_ended_sprite_nums: Vec<(ScoreRef, u32)> = vec![];
        let ended_sprite_nums = self
            .movie
            .score
            .end_sprites(ScoreRef::Stage, self.movie.current_frame, next_frame)
            .await;
        all_ended_sprite_nums.extend(ended_sprite_nums.iter().map(|&x| (ScoreRef::Stage, x)));

        let active_filmloops = self.active_stage_filmloop_member_refs();
        for member_ref in active_filmloops {
            let score_ref = ScoreRef::FilmLoop(member_ref.clone());
            let film_loop = match self
                .movie
                .cast_manager
                .find_mut_member_by_ref(&member_ref)
                .and_then(|m| m.member_type.as_film_loop_mut())
            {
                Some(fl) => fl,
                None => continue,
            };

            let filmloop_current_frame = film_loop.current_frame;
            let filmloop_next_frame = filmloop_current_frame + 1;

            let ended_sprite_nums = film_loop
                .score
                .end_sprites(
                    score_ref.clone(),
                    filmloop_current_frame,
                    filmloop_next_frame,
                )
                .await;
            all_ended_sprite_nums.extend(ended_sprite_nums.iter().map(|&x| (score_ref.clone(), x)));
        }
        // for sprite_num in ended_sprite_nums.iter() {
        //   let sprite = self.movie.score.get_sprite_mut(*sprite_num as i16);
        //   sprite.exited = true;
        // }
        self.invalidate_behavior_channel_cache();
        self.invalidate_active_stage_filmloop_cache();
        all_ended_sprite_nums
    }

    /// Get all active filmloop scores with their member references.
    /// Returns a vector of tuples containing (member_ref, current_frame).
    /// Only includes filmloops that are currently visible on stage.
    /// Deduplicates filmloops - each unique filmloop is only returned once even if used in multiple sprites.
    pub fn get_active_filmloop_scores(&mut self) -> Vec<(CastMemberRef, u32)> {
        let member_refs = self.active_stage_filmloop_member_refs();
        let mut active_filmloops = Vec::with_capacity(member_refs.len());

        for member_ref in member_refs {
            if let Some(member) = self.movie.cast_manager.find_member_by_ref(&member_ref) {
                if let cast_member::CastMemberType::FilmLoop(film_loop) = &member.member_type {
                    active_filmloops.push((member_ref, film_loop.current_frame));
                }
            }
        }

        active_filmloops
    }

    pub fn pause_script(&mut self) {
        self.is_script_paused = true;
    }

    pub fn resume_script(&mut self) {
        self.is_script_paused = false;
    }

    pub fn resume_breakpoint(&mut self) {
        self.step_mode = StepMode::None;
        self.eval_scope_index = None;
        let breakpoint = self.current_breakpoint.take();

        if let Some(breakpoint) = breakpoint {
            crate::player::spawn_player_local(breakpoint.completer.complete(()));
        }
    }

    pub fn step_into(&mut self) {
        self.step_mode = StepMode::Into;
        self.step_scope_depth = self.scope_count;
        self.eval_scope_index = None;
        let breakpoint = self.current_breakpoint.take();

        if let Some(breakpoint) = breakpoint {
            crate::player::spawn_player_local(breakpoint.completer.complete(()));
        }
    }

    pub fn step_over(&mut self) {
        self.step_mode = StepMode::Over;
        self.step_scope_depth = self.scope_count;
        self.eval_scope_index = None;
        let breakpoint = self.current_breakpoint.take();

        if let Some(breakpoint) = breakpoint {
            crate::player::spawn_player_local(breakpoint.completer.complete(()));
        }
    }

    pub fn step_out(&mut self) {
        self.step_mode = StepMode::Out;
        self.step_scope_depth = self.scope_count;
        self.eval_scope_index = None;
        let breakpoint = self.current_breakpoint.take();

        if let Some(breakpoint) = breakpoint {
            crate::player::spawn_player_local(breakpoint.completer.complete(()));
        }
    }

    pub fn step_over_line(&mut self, skip_bytecode_indices: Vec<usize>) {
        self.step_mode = StepMode::OverLine {
            skip_bytecode_indices,
        };
        self.step_scope_depth = self.scope_count;
        self.eval_scope_index = None;
        let breakpoint = self.current_breakpoint.take();

        if let Some(breakpoint) = breakpoint {
            crate::player::spawn_player_local(breakpoint.completer.complete(()));
        }
    }

    pub fn step_into_line(&mut self, skip_bytecode_indices: Vec<usize>) {
        self.step_mode = StepMode::IntoLine {
            skip_bytecode_indices,
        };
        self.step_scope_depth = self.scope_count;
        self.eval_scope_index = None;
        let breakpoint = self.current_breakpoint.take();

        if let Some(breakpoint) = breakpoint {
            crate::player::spawn_player_local(breakpoint.completer.complete(()));
        }
    }

    #[inline]
    pub fn get_datum(&self, id: &DatumRef) -> &Datum {
        self.allocator.get_datum(id)
    }

    pub(crate) fn get_datum_mut(&mut self, id: &DatumRef) -> &mut Datum {
        self.allocator.get_datum_mut(id)
    }

    /// Resolve a datum to a BitmapId.  Accepts:
    ///  - `Datum::BitmapRef(n)` → direct handle
    ///  - `Datum::Int(n)` where n > 0 → treated as a member slot number;
    ///    the member is looked up and its bitmap image_ref is returned.
    pub fn resolve_bitmap_ref(&self, datum: &Datum) -> Result<BitmapId, ScriptError> {
        match datum {
            Datum::BitmapRef(br) => self
                .bitmap_manager
                .local_id(br)
                .ok_or_else(|| ScriptError::new("stale or foreign bitmap handle".to_string())),
            Datum::Int(n) if *n > 0 => {
                let member_ref = handlers::datum_handlers::cast_member_ref::CastMemberRefHandlers
                    ::member_ref_from_slot_number(*n as u32);
                if let Some(member) = self.movie.cast_manager.find_member_by_ref(&member_ref) {
                    if let Some(bitmap_member) = member.member_type.as_bitmap() {
                        Ok(bitmap_member.image_ref)
                    } else {
                        Err(ScriptError::new(format!(
                            "Cannot convert Int({}) to bitmap ref: member {}:{} is not a bitmap",
                            n, member_ref.cast_lib, member_ref.cast_member
                        )))
                    }
                } else {
                    Err(ScriptError::new(format!(
                        "Cannot convert Int({}) to bitmap ref: no member found for slot number",
                        n
                    )))
                }
            }
            _ => datum.to_bitmap_ref().and_then(|br| {
                self.bitmap_manager
                    .local_id(br)
                    .ok_or_else(|| ScriptError::new("stale or foreign bitmap handle".to_string()))
            }),
        }
    }

    pub(crate) fn bitmap_handle_for_id(
        &self,
        bitmap_id: BitmapId,
    ) -> Result<BitmapHandle, ScriptError> {
        self.bitmap_manager
            .local_handle(bitmap_id)
            .ok_or_else(|| ScriptError::new(format!("missing bitmap {bitmap_id}")))
    }

    pub fn get_fps(&self) -> u32 {
        if self.movie.puppet_tempo > 0 {
            self.movie.puppet_tempo
        } else {
            self.movie.frame_rate as u32
        }
    }

    pub fn get_hydrated_globals(&self) -> FxHashMap<Symbol, &Datum> {
        self.globals
            .iter()
            .map(|(k, v)| (k.clone(), self.get_datum(v)))
            .collect()
    }

    #[allow(dead_code)]
    pub fn get_global(&self, name: Symbol) -> Option<&Datum> {
        self.globals
            .get(&name)
            .map(|datum_ref| self.get_datum(datum_ref))
    }

    pub fn get_next_frame(&self) -> u32 {
        if !self.is_playing {
            return self.movie.current_frame;
        } else if let Some(next_frame) = self.next_frame {
            return next_frame;
        } else {
            let next = self.movie.current_frame + 1;
            // Loop back to frame 1 when past the last frame
            if let Some(frame_count) = self.movie.score.frame_count {
                if next > frame_count {
                    return 1;
                }
            }
            return next;
        }
    }

    pub fn advance_frame(&mut self) {
        if !self.is_playing {
            return;
        }
        // A score transition is playing out (rendered per-frame by the renderer):
        // hold the playhead until it finishes so the movie doesn't run ahead of the
        // visible dissolve/wipe (matches Director, which blocks during a transition).
        // Time-based so it self-clears — it can never permanently freeze the movie.
        if self.transition_hold_active() {
            self.stage_dirty = true;
            return;
        }
        self.stage_dirty = true;

        let prev_frame = self.movie.current_frame;
        let next_frame = self.get_next_frame();

        // Always advance logic (scripts, behaviors)
        self.next_frame = None;
        self.movie.current_frame = next_frame;

        // Leaving a frame drops the "imaging Lingo" stage overlay. An imaging
        // engine (spectral-wizard) draws its game/menu/worldmap into
        // `(the stage).image` on ONE looping frame (`go(the frame)` keeps the
        // same frame → overlay persists). When it actually changes frames —
        // e.g. Game Over does `go("highscore")` to a sprite-based frame — the
        // stale game framebuffer would otherwise keep compositing over the new
        // frame's sprites (a black screen). Clear the dirty flag on a real
        // frame change; the next frame re-dirties it only if it draws into the
        // stage image again.
        if prev_frame != next_frame {
            self.stage_image_dirty = false;

            // A transition placed on the entered frame plays between the previous
            // stage and this frame's stage. Look up the transition member and hand
            // its effect to the renderer, then pause the playhead until it completes.
            if let Some(trans_ref) = self.movie.score.get_frame_transition(next_frame) {
                let info = self
                    .movie
                    .cast_manager
                    .find_member_by_ref(&trans_ref)
                    .and_then(|m| match &m.member_type {
                        crate::player::cast_member::CastMemberType::Transition(t) => Some(t.info),
                        _ => None,
                    });
                if let Some(info) = info {
                    self.pending_transition = Some(info);
                    self.begin_transition_hold(info.duration_ms);
                }
            }
        }

        // NOTE: Filmloop frames are advanced solely by update_filmloop_frames() in the main loop.
        // Do NOT call advance_filmloop_frames() here - it would cause double advancement since
        // advance_frame() is also called from the `go` handler, and update_filmloop_frames()
        // runs separately in the main loop.

        // Only dispatch and render if updateLock is off
        if !self.movie.update_lock && prev_frame != self.movie.current_frame {
            if let Err(error) =
                self.queue_host_event(crate::player::host_events::HostEvent::FrameChanged {
                    frame: self.movie.current_frame,
                })
            {
                log::error!("host event mailbox overflow: {:?}", error);
            }
            self.has_player_frame_changed = true;
        }
    }

    /// True while the playhead is held for a score/puppet transition. Normally
    /// the renderer clears `score_transition_active` the moment its animation
    /// completes (precise sync); the wall-clock deadline is only a failsafe so a
    /// missed completion signal can never permanently freeze the movie.
    pub fn transition_hold_active(&mut self) -> bool {
        if !self.score_transition_active {
            return false;
        }
        if let Some(deadline) = self.transition_hold_until_ms {
            if chrono::Utc::now().timestamp_millis() >= deadline {
                self.score_transition_active = false;
                self.transition_hold_until_ms = None;
                return false;
            }
        }
        true
    }

    /// Begin holding the playhead for a transition of `duration_ms`. The renderer
    /// releases the hold on completion; the failsafe deadline is set generously
    /// past the animation window so it never trips during normal playback.
    pub fn begin_transition_hold(&mut self, duration_ms: u16) {
        self.score_transition_active = true;
        let dur = (duration_ms as i64).clamp(1, 4000);
        self.transition_hold_until_ms = Some(chrono::Utc::now().timestamp_millis() + dur + 2000);
    }

    pub fn stop(&mut self) {
        // TODO dispatch stop movie
        self.is_playing = false;
        self.next_frame = None;
        //scopes.clear();
        // currentBreakpoint?.completer.completeError(CancelledException());
        // currentBreakpoint = null;
        //self.timeout_manager.clear();
        //notifyListeners();

        warn!("Profiler report: {}", get_profiler_report());
    }

    fn bump_scope_invalidation_epoch(&mut self) {
        self.scope_invalidation_epoch = self
            .scope_invalidation_epoch
            .checked_add(1)
            .expect("scope invalidation epoch exhausted");
    }

    pub fn reset(&mut self) {
        self.reset_core(true);
    }

    /// Reset only this player owner. Frontend/session callers use this path so
    /// a nested or secondary player cannot clear process-wide JS, Xtra, or
    /// scene registries belonging to another owner.
    pub(crate) fn reset_owned_core(&mut self) {
        self.reset_core(false);
    }

    fn reset_core(&mut self, global_resources: bool) {
        self.owner
            .key()
            .checked_next_generation()
            .expect("owner generation exhausted before player reset");
        let transient_bitmap_ids = self
            .flash_frame_buffers
            .values()
            .chain(self.nested_movie_images.values())
            .chain(self.w3d_frame_buffers.values())
            .copied()
            .chain(self.stage_image)
            .collect::<Vec<_>>();
        self.bump_scope_invalidation_epoch();
        self.stop();
        self.pending_player_notifications.clear();
        self.host_event_mailbox.clear();
        self.host_event_backpressure = None;

        // Silence any sound still playing from the movie we're leaving and tear
        // down its Flash/Ruffle instances (their capture RAF loops + SWF audio),
        // so switching movies doesn't leave old sounds looping or leak players.
        self.sound_manager.stop_all();
        self.flash_frame_buffers.clear();
        self.flash_sprite_loaded.clear();
        self.flash_host_actions.clear();
        self.flash_ready_sprites.clear();
        self.nested_movie_images.clear();
        self.w3d_frame_buffers.clear();
        self.stage_image = None;
        self.stage_image_dirty = false;
        self.flash_scripted_access_pending.set(false);
        JsApi::dispatch_flash_reset_all(&owner_key_string(&self.owner));
        self.scene3d_store.reset();
        // Tear down player-owned Xtra instances before allocator reset. Any
        // pending intent retains the old owner and cannot reach a replacement
        // instance with the same numeric id.
        self.xtra_manager_state.reset();

        // Clear all references before resetting the allocator
        // This ensures all DatumRef and ScriptInstanceRef objects are dropped properly
        debug!("Clearing scopes");
        self.scopes.clear();
        debug!("Clearing globals");
        self.globals.clear();
        debug!("Clearing timeout manager");
        self.timeout_manager.clear();
        debug!("Clearing debug datum refs");
        self.debug_datum_refs.clear();
        // netManager.clear();
        debug!("Resetting score");
        self.movie.score.reset();
        self.clear_script_instance_list_caches();
        self.invalidate_active_stage_filmloop_cache();
        self.movie.current_frame = 1;
        // TODO cancel breakpoints
        self.current_breakpoint = None;
        self.scope_count = 0;
        self.pending_goto_net_movie = None;
        self.goto_wait_active = false;
        self.pending_movie_init = false;
        self.last_initialized_frame = None;
        self.playback_init_count = 0;
        self.playback_frame_count = 0;
        self.playback_stop_count = 0;
        self.retired_cast_libs.clear();
        self.pending_restart = false;
        self.w3d_dirty_transform_ids.clear();

        debug!("Resetting allocator");
        // Now it's safe to reset the allocator
        self.owner = self.allocator.reset(&mut self.bitmap_manager);
        for bitmap_id in transient_bitmap_ids {
            self.bitmap_manager.remove_ephemeral_bitmap(bitmap_id);
        }
        // A reset rotates the owner generation.  Retain the old cell only for
        // stale capabilities; the replacement generation receives a distinct
        // cell so an old capability can never change the new player state.
        self.flash_scripted_access_pending = Rc::new(Cell::new(false));
        self.flash_binding_state = Rc::new(RefCell::new(FlashBindingState::new()));
        self.flash_object_counter = 0;
        // The allocator creates a fresh epoch on reset. Keep the teardown
        // queue owned by this player, but bind new instances to that epoch so
        // stale Xtra completions cannot reach a replacement instance.
        self.xtra_manager_state.rebind_owner(self.owner.clone());
        self.net_manager.reset_owner(self.owner.key());

        self.initialize_globals();

        // Initialize scopes (call stack)
        for i in 0..MAX_STACK_SIZE {
            self.scopes.push(Scope::default(i));
        }

        if let Err(error) =
            self.queue_host_event(crate::player::host_events::HostEvent::FrameChanged {
                frame: self.movie.current_frame,
            })
        {
            log::error!("host event mailbox overflow: {:?}", error);
        }
        JsApi::dispatch_scope_list(self);
        JsApi::dispatch_script_error_cleared();
        self.queue_player_notification(PlayerNotificationKind::ScoreChanged);
    }

    pub fn initialize_globals(&mut self) {
        // Initialize the actorList as a global variable
        let actor_list_datum =
            self.alloc_datum(Datum::List(DatumType::List, VecDeque::new(), false));
        self.globals
            .insert(Symbol::builtin(BuiltInSymbol::ActorList), actor_list_datum);
        self.actor_list_generation = 0;

        // Mathematical constant
        let pi_datum = self.alloc_datum(Datum::Float(std::f64::consts::PI));
        self.globals
            .insert(Symbol::builtin(BuiltInSymbol::Pi), pi_datum);

        // Special values
        let void_datum = self.alloc_datum(Datum::Void);
        self.globals
            .insert(Symbol::builtin(BuiltInSymbol::Void), void_datum);

        let empty_datum = self.alloc_datum(Datum::String("".to_string()));
        self.globals
            .insert(Symbol::builtin(BuiltInSymbol::Empty), empty_datum);

        // String constants
        let return_datum = self.alloc_datum(Datum::String("\r".to_string()));
        self.globals
            .insert(Symbol::builtin(BuiltInSymbol::Return), return_datum);

        let enter_datum = self.alloc_datum(Datum::String("\x03".to_string()));
        self.globals
            .insert(Symbol::builtin(BuiltInSymbol::Enter), enter_datum);

        let quote_datum = self.alloc_datum(Datum::String("\"".to_string()));
        self.globals
            .insert(Symbol::builtin(BuiltInSymbol::Quote), quote_datum);

        let tab_datum = self.alloc_datum(Datum::String("\t".to_string()));
        self.globals
            .insert(Symbol::builtin(BuiltInSymbol::Tab), tab_datum);

        // Backspace character
        let backspace_datum = self.alloc_datum(Datum::String("\x08".to_string()));
        self.globals
            .insert(Symbol::builtin(BuiltInSymbol::BackSpace), backspace_datum);

        // Boolean constants (these are typically handled as keywords, but can be globals)
        let true_datum = self.alloc_datum(Datum::Int(1));
        self.globals
            .insert(Symbol::builtin(BuiltInSymbol::True), true_datum);

        let false_datum = self.alloc_datum(Datum::Int(0));
        self.globals
            .insert(Symbol::builtin(BuiltInSymbol::False), false_datum);
    }

    #[inline]
    pub fn alloc_datum(&mut self, datum: Datum) -> DatumRef {
        self.allocator.drain_reclaims(&mut self.bitmap_manager);
        self.allocator
            .alloc_datum(datum, &mut self.bitmap_manager)
            .unwrap()
    }

    /// Drain owner-local deferred arena drops at execution boundaries.
    pub fn drain_allocator_reclaims(&mut self) {
        self.allocator.drain_reclaims(&mut self.bitmap_manager);
    }

    /// Fast path: push an int without constructing a 64-byte `Datum` (pooled
    /// values return a cached immortal ref). Hot path for pushint*/pushzero.
    #[inline]
    pub fn alloc_int(&mut self, n: i32) -> DatumRef {
        self.allocator.drain_reclaims(&mut self.bitmap_manager);
        self.allocator.alloc_int(n)
    }

    /// Fast path: push an interned symbol without constructing a `Datum`.
    #[inline]
    pub fn alloc_symbol(&mut self, sym: crate::player::symbols::symbol::Symbol) -> DatumRef {
        self.allocator.drain_reclaims(&mut self.bitmap_manager);
        self.allocator.alloc_symbol(sym)
    }

    /// Sync cached scriptInstanceList back to sprite's Vec for a given sprite.
    /// Call this before reading script_instance_list on sprites that may have been
    /// modified via .add() on the cached Datum::List.
    pub fn sync_script_instance_list(&mut self, sprite_id: i16) {
        if self.script_instance_list_cache.contains_key(&sprite_id) {
            let existing_ids = self
                .movie
                .score
                .get_sprite(sprite_id)
                .map(|sprite| sprite.script_instance_list.clone())
                .unwrap_or_default();
            let synced_ids = self.get_sprite_script_instance_ids(sprite_id, &existing_ids);
            {
                let sprite = self.movie.score.get_sprite_mut(sprite_id);
                sprite.script_instance_list = synced_ids.clone();
            }
            // Stamp `spriteNum` on every instance so `the currentSpriteNum`
            // resolves for behaviors attached at RUNTIME via
            // `add(the scriptInstanceList of sprite X, new(script(...)))`. That
            // add-to-cache path bypasses the scriptInstanceList *setter* (which
            // stamps spriteNum) and the score begin-sprite path, so without this
            // the instance's spriteNum stayed Void and `the currentSpriteNum`
            // returned 0. Summer Resort's inventory icons attach
            // inventory.select.item this way and read `the currentSpriteNum` in
            // their mouseUp to identify the clicked item — a 0 there made
            // `getPos(page, "")` return 0 and crashed showDescription.
            let sprite_num_ref = self.alloc_datum(Datum::Int(sprite_id as i32));
            for inst in &synced_ids {
                let mut symbols = SymbolTable::new();
                let _ = crate::player::script::script_set_prop(
                    self,
                    &symbols,
                    inst,
                    Symbol::builtin(BuiltInSymbol::SpriteNum),
                    &sprite_num_ref,
                    false,
                );
            }
        }
    }

    /// Sync ALL cached scriptInstanceLists back to sprite Vecs.
    pub fn sync_all_script_instance_lists(&mut self) {
        let sprite_ids: Vec<i16> = self.script_instance_list_cache.keys().cloned().collect();
        for sprite_id in sprite_ids {
            self.sync_script_instance_list(sprite_id);
        }
    }

    pub fn cache_script_instance_list(
        &mut self,
        sprite_id: i16,
        list_ref: DatumRef,
        initial_ids: Vec<ScriptInstanceRef>,
    ) {
        self.remove_script_instance_list_cache(sprite_id);
        let generation = *self
            .script_instance_list_generation
            .entry(sprite_id)
            .or_insert(0);
        self.script_instance_list_cache_owner
            .insert(list_ref.unwrap(), sprite_id);
        self.script_instance_list_cache.insert(sprite_id, list_ref);
        self.script_instance_list_ids_cache
            .insert(sprite_id, (generation, initial_ids));
    }

    pub fn remove_script_instance_list_cache(&mut self, sprite_id: i16) {
        if let Some(cached_ref) = self.script_instance_list_cache.remove(&sprite_id) {
            self.script_instance_list_cache_owner
                .remove(&cached_ref.unwrap());
        }
        self.script_instance_list_generation.remove(&sprite_id);
        self.script_instance_list_ids_cache.remove(&sprite_id);
    }

    pub fn clear_script_instance_list_caches(&mut self) {
        self.script_instance_list_cache.clear();
        self.script_instance_list_cache_owner.clear();
        self.script_instance_list_generation.clear();
        self.script_instance_list_ids_cache.clear();
        self.invalidate_behavior_channel_cache();
    }

    pub fn note_script_instance_list_mutation(&mut self, datum_ref: &DatumRef) {
        if let Some(sprite_id) = self
            .script_instance_list_cache_owner
            .get(&datum_ref.unwrap())
            .copied()
        {
            let generation = self
                .script_instance_list_generation
                .entry(sprite_id)
                .or_insert(0);
            *generation = generation.wrapping_add(1);
            self.script_instance_list_ids_cache.remove(&sprite_id);
            self.refresh_stage_behavior_channel_cache_entry(sprite_id);
        }
    }

    pub fn invalidate_behavior_channel_cache(&mut self) {
        self.behavior_channel_cache_generation =
            self.behavior_channel_cache_generation.wrapping_add(1);
        self.active_stage_behavior_channels_cache = None;
        self.active_stage_message_channels_cache = None;
    }

    pub fn invalidate_active_stage_filmloop_cache(&mut self) {
        self.active_stage_filmloop_cache_generation =
            self.active_stage_filmloop_cache_generation.wrapping_add(1);
        self.active_stage_filmloop_members_cache = None;
    }

    pub fn get_sprite_script_instance_ids(
        &mut self,
        sprite_id: i16,
        fallback: &[ScriptInstanceRef],
    ) -> Vec<ScriptInstanceRef> {
        let Some(cached_ref) = self.script_instance_list_cache.get(&sprite_id).cloned() else {
            return fallback.to_vec();
        };

        let generation = *self
            .script_instance_list_generation
            .get(&sprite_id)
            .unwrap_or(&0);
        if let Some((cached_generation, ids)) = self.script_instance_list_ids_cache.get(&sprite_id)
        {
            if *cached_generation == generation {
                return ids.clone();
            }
        }

        let ids = match self.get_datum(&cached_ref) {
            Datum::List(_, item_refs, _) => item_refs
                .iter()
                .filter_map(|item_ref| match self.get_datum(item_ref) {
                    Datum::ScriptInstanceRef(id) => Some(id.clone()),
                    _ => None,
                })
                .collect(),
            _ => fallback.to_vec(),
        };

        self.script_instance_list_ids_cache
            .insert(sprite_id, (generation, ids.clone()));
        ids
    }

    pub fn sprite_has_script_instance_ids(
        &self,
        sprite_id: i16,
        fallback: &[ScriptInstanceRef],
    ) -> bool {
        if !fallback.is_empty() {
            return true;
        }

        self.script_instance_list_cache
            .get(&sprite_id)
            .is_some_and(|cached_ref| {
                matches!(self.get_datum(cached_ref), Datum::List(_, items, _) if !items.is_empty())
            })
    }

    pub fn refresh_stage_behavior_channel_cache_entry(&mut self, sprite_id: i16) {
        let channel_number = sprite_id as usize;
        let should_include = self
            .movie
            .score
            .channels
            .get(channel_number)
            .is_some_and(|channel| {
                (channel.sprite.entered || channel.sprite.puppet)
                    && self.sprite_has_script_instance_ids(
                        sprite_id,
                        &channel.sprite.script_instance_list,
                    )
            });

        // The message-channel cache is keyed on the same (frame, generation)
        // pair but has a BROADER predicate (`is_active_sprite`: behaviors OR a
        // cast member script), so it can't be patched with `should_include`.
        // Drop it and let the next read rebuild.
        //
        // Without this it kept a stale entry for the whole time the playhead
        // sat on one frame, because nothing here bumps the generation. Coke
        // Studios' navigator builds its scrollbar at runtime —
        //   puppetSprite(N, 1)                       -- no behaviors yet
        //   sprite(N).scriptInstanceList = [new(script("scrollvert slider"),…)]
        // — and the second line refreshed only the behavior cache below, while
        // `dispatch_event_to_all_behaviors` walks the MESSAGE cache. The lift's
        // `exitFrame` was therefore never dispatched: it kept its authored
        // `.passive` member and ignored clicks, until an unrelated change
        // finally bumped the generation. `active_stage_message_channels` was
        // added later than the incremental refresh and was never wired into it.
        self.active_stage_message_channels_cache = None;

        let Some((_, _, channels)) = self.active_stage_behavior_channels_cache.as_mut() else {
            return;
        };

        match channels.binary_search(&channel_number) {
            Ok(index) if !should_include => {
                channels.remove(index);
            }
            Err(index) if should_include => {
                channels.insert(index, channel_number);
            }
            _ => {}
        }
    }

    pub fn active_stage_behavior_channels(&mut self) -> Vec<usize> {
        let frame_num = self.movie.current_frame;
        let generation = self.behavior_channel_cache_generation;

        if let Some((cached_frame, cached_generation, channels)) =
            &self.active_stage_behavior_channels_cache
        {
            if *cached_frame == frame_num && *cached_generation == generation {
                return channels.clone();
            }
        }

        let channels: Vec<usize> = self
            .movie
            .score
            .channels
            .iter()
            .filter(|channel| channel.sprite.entered || channel.sprite.puppet)
            .filter(|channel| {
                self.sprite_has_script_instance_ids(
                    channel.number as i16,
                    &channel.sprite.script_instance_list,
                )
            })
            .map(|channel| channel.number)
            .collect();

        self.active_stage_behavior_channels_cache = Some((frame_num, generation, channels.clone()));
        channels
    }

    /// Channels that can receive an event this frame — behaviors OR a cast
    /// member script. `active_stage_behavior_channels` deliberately keeps only
    /// the former (behavior instantiation and beginSprite work off it), but
    /// event dispatch must also reach a sprite whose only handler lives on its
    /// cast member: Lifesavers Pineapple Treasure Hunt puts
    /// `on prepareFrame addPlatform()` on every walkable terrain MEMBER and
    /// attaches no behaviors at all, so filtering on behaviors alone dropped
    /// every collision surface in the game and the player fell through the
    /// world.
    pub fn active_stage_message_channels(&mut self) -> Vec<usize> {
        let frame_num = self.movie.current_frame;
        let generation = self.behavior_channel_cache_generation;

        if let Some((cached_frame, cached_generation, channels)) =
            &self.active_stage_message_channels_cache
        {
            if *cached_frame == frame_num && *cached_generation == generation {
                return channels.clone();
            }
        }

        let channels: Vec<usize> = self
            .movie
            .score
            .channels
            .iter()
            .filter(|channel| channel.sprite.entered || channel.sprite.puppet)
            .filter(|channel| crate::player::score::is_active_sprite(self, &channel.sprite))
            .map(|channel| channel.number)
            .collect();

        self.active_stage_message_channels_cache = Some((frame_num, generation, channels.clone()));
        channels
    }

    pub fn active_stage_filmloop_member_refs(&mut self) -> Vec<CastMemberRef> {
        let frame_num = self.movie.current_frame;
        let generation = self.active_stage_filmloop_cache_generation;
        if let Some((cached_frame, cached_generation, member_refs)) =
            &self.active_stage_filmloop_members_cache
        {
            if *cached_frame == frame_num && *cached_generation == generation {
                return member_refs.clone();
            }
        }

        let mut channel_numbers = self.movie.score.active_channel_numbers_for_frame(frame_num);
        let mut seen_channels = channel_numbers
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();
        for channel in self.movie.score.channels.iter() {
            if channel.number != 0
                && channel.sprite.puppet
                && channel.sprite.visible
                && seen_channels.insert(channel.number)
            {
                channel_numbers.push(channel.number);
            }
        }

        let mut member_refs = Vec::new();
        let mut seen_members = std::collections::HashSet::new();
        for channel_number in channel_numbers {
            let Some(channel) = self.movie.score.channels.get(channel_number) else {
                continue;
            };

            if channel.number == 0 || !channel.sprite.visible {
                continue;
            }
            let Some(member_ref) = channel.sprite.member.as_ref() else {
                continue;
            };

            if !seen_members.insert(member_ref.clone()) {
                continue;
            }

            let is_filmloop = self
                .movie
                .cast_manager
                .find_member_by_ref(member_ref)
                .is_some_and(|member| {
                    member.member_type.member_type_id() == cast_member::CastMemberTypeId::FilmLoop
                });
            if is_filmloop {
                member_refs.push(member_ref.clone());
            }
        }

        self.active_stage_filmloop_members_cache =
            Some((frame_num, generation, member_refs.clone()));
        member_refs
    }

    pub fn active_stage_script_instance_ids(&mut self) -> Vec<ScriptInstanceRef> {
        let mut channel_numbers = self.active_stage_behavior_channels();
        let frame_channel_is_active = self
            .movie
            .score
            .active_channel_numbers_for_frame(self.movie.current_frame)
            .contains(&0);
        if frame_channel_is_active && !channel_numbers.contains(&0) {
            channel_numbers.push(0);
        }

        let mut receivers = Vec::new();
        for channel_number in channel_numbers {
            let Some(fallback) = self
                .movie
                .score
                .channels
                .get(channel_number)
                .map(|channel| channel.sprite.script_instance_list.clone())
            else {
                continue;
            };
            receivers.extend(
                self.get_sprite_script_instance_ids(channel_number as i16, fallback.as_slice()),
            );
        }
        receivers
    }

    pub fn note_actor_list_mutation(&mut self, datum_ref: &DatumRef) {
        if self
            .globals
            .get(&Symbol::builtin(BuiltInSymbol::ActorList))
            .is_some_and(|actor_list_ref| actor_list_ref == datum_ref)
        {
            self.actor_list_generation = self.actor_list_generation.wrapping_add(1);
        }
    }

    pub fn actor_list_stepframe_snapshot(&self) -> (VecDeque<DatumRef>, HashSet<usize>, u64) {
        let actor_list_ref = self
            .globals
            .get(&Symbol::builtin(BuiltInSymbol::ActorList))
            .cloned()
            .unwrap_or(DatumRef::Void);
        match self.get_datum(&actor_list_ref) {
            Datum::List(_, items, _) => {
                let snapshot = items.clone();
                let active_ids = items.iter().map(|actor_ref| actor_ref.unwrap()).collect();
                (snapshot, active_ids, self.actor_list_generation)
            }
            _ => (VecDeque::new(), HashSet::new(), self.actor_list_generation),
        }
    }

    pub fn actor_list_active_ids(&self) -> (HashSet<usize>, u64) {
        let actor_list_ref = self
            .globals
            .get(&Symbol::builtin(BuiltInSymbol::ActorList))
            .cloned()
            .unwrap_or(DatumRef::Void);
        match self.get_datum(&actor_list_ref) {
            Datum::List(_, items, _) => (
                items.iter().map(|actor_ref| actor_ref.unwrap()).collect(),
                self.actor_list_generation,
            ),
            _ => (HashSet::new(), self.actor_list_generation),
        }
    }

    fn datum_leak_scan(&mut self) -> String {
        use crate::player::datum_formatting::format_datum;
        use std::collections::{HashMap, HashSet};

        let snapshot_id = self.allocator.snapshot_max_id;
        let is_new = |id: usize| -> bool { snapshot_id > 0 && id > snapshot_id };

        let ref_id = |r: &DatumRef| -> Option<usize> {
            if let DatumRef::Ref(_) = r {
                Some(r.unwrap())
            } else {
                None
            }
        };

        // Step 1: Count ALL new datums by type
        let mut type_counts_new: HashMap<String, usize> = HashMap::new();
        let mut new_datum_ids: HashSet<usize> = HashSet::new();
        for (id, entry) in self.allocator.iter_datums() {
            if is_new(id) {
                let type_name = entry.datum.type_str().to_string();
                *type_counts_new.entry(type_name).or_insert(0) += 1;
                new_datum_ids.insert(id);
            }
        }

        // Step 2: Find OLD containers that hold NEW datums (= GROWING containers)
        let mut growing_containers: Vec<(String, usize)> = Vec::new();
        let mut accounted: HashSet<usize> = HashSet::new();

        for (cid, entry) in self.allocator.iter_datums() {
            if is_new(cid) {
                continue;
            } // only look at old containers
            let rc = unsafe { *entry.ref_count.get() };
            match &entry.datum {
                Datum::List(list_type, items, _) => {
                    let new_items: Vec<usize> = items
                        .iter()
                        .filter_map(|r| ref_id(r).filter(|id| new_datum_ids.contains(id)))
                        .collect();
                    if !new_items.is_empty() {
                        // Show what the new items ARE (types)
                        let mut child_types: HashMap<String, usize> = HashMap::new();
                        for &nid in &new_items {
                            if let Some(e) = self.allocator.get_datum_entry(nid) {
                                *child_types
                                    .entry(e.datum.type_str().to_string())
                                    .or_insert(0) += 1;
                            }
                        }
                        let types_str: Vec<String> = child_types
                            .iter()
                            .map(|(t, c)| format!("{}x{}", c, t))
                            .collect();
                        growing_containers.push((
                            format!(
                                "OLD {:?}List #{} (len={},rc={}) +{} new [{}]",
                                list_type,
                                cid,
                                items.len(),
                                rc,
                                new_items.len(),
                                types_str.join(",")
                            ),
                            new_items.len(),
                        ));
                        for nid in new_items {
                            accounted.insert(nid);
                        }
                    }
                }
                Datum::PropList(pairs, _) => {
                    let mut new_count = 0;
                    for (k, v) in pairs {
                        if ref_id(v).map_or(false, |id| new_datum_ids.contains(&id)) {
                            new_count += 1;
                            accounted.insert(ref_id(v).unwrap());
                        }
                        if ref_id(k).map_or(false, |id| new_datum_ids.contains(&id)) {
                            new_count += 1;
                            accounted.insert(ref_id(k).unwrap());
                        }
                    }
                    if new_count > 0 {
                        let keys_preview: Vec<String> = pairs
                            .iter()
                            .take(3)
                            .map(|(k, _)| format!("{:?}", k))
                            .collect();
                        growing_containers.push((
                            format!(
                                "OLD PropList #{} (len={},rc={}) +{} new keys=[{}...]",
                                cid,
                                pairs.len(),
                                rc,
                                new_count,
                                keys_preview.join(",")
                            ),
                            new_count,
                        ));
                    }
                }
                Datum::Point(..) | Datum::Rect(..) => {
                    // Inline storage - no DatumRef children to track
                }
                _ => {}
            }
        }

        // Step 3: Find NEW compound datums containing NEW children (new sub-trees)
        let mut new_subtree_types: HashMap<String, usize> = HashMap::new();
        for (cid, entry) in self.allocator.iter_datums() {
            if !is_new(cid) {
                continue;
            }
            let rc = unsafe { *entry.ref_count.get() };
            let new_child_count = match &entry.datum {
                Datum::List(_, items, _) => {
                    let c: usize = items
                        .iter()
                        .filter(|r| ref_id(r).map_or(false, |id| new_datum_ids.contains(&id)))
                        .count();
                    if c > 0 {
                        for r in items {
                            if let Some(id) = ref_id(r).filter(|id| new_datum_ids.contains(id)) {
                                accounted.insert(id);
                            }
                        }
                    }
                    c
                }
                Datum::PropList(pairs, _) => {
                    let mut c = 0;
                    for (k, v) in pairs {
                        if ref_id(v).map_or(false, |id| new_datum_ids.contains(&id)) {
                            c += 1;
                            accounted.insert(ref_id(v).unwrap());
                        }
                        if ref_id(k).map_or(false, |id| new_datum_ids.contains(&id)) {
                            c += 1;
                            accounted.insert(ref_id(k).unwrap());
                        }
                    }
                    c
                }
                Datum::Point(..) | Datum::Rect(..) => {
                    // Inline storage - no DatumRef children to track
                    0
                }
                _ => 0,
            };
            if new_child_count > 0 {
                *new_subtree_types
                    .entry(format!("NEW {}(rc={})", entry.datum.type_str(), rc))
                    .or_insert(0) += 1;
            }
        }

        // Step 4: Check script instance properties for new datums
        let mut si_new: HashMap<String, usize> = HashMap::new();
        for (si_id, entry) in self.allocator.iter_script_instances() {
            for (prop_name, r) in &entry.script_instance.properties {
                if let Some(id) = ref_id(r).filter(|id| new_datum_ids.contains(id)) {
                    let sn = self
                        .movie
                        .cast_manager
                        .get_script_by_ref(&entry.script_instance.script)
                        .map(|s| s.name.clone())
                        .unwrap_or_else(|| format!("si#{}", si_id));
                    let dtype = self
                        .allocator
                        .get_datum_entry(id)
                        .map(|e| e.datum.type_str())
                        .unwrap_or("?");
                    *si_new
                        .entry(format!("si({}).{:?} [{}]", sn, prop_name, dtype))
                        .or_insert(0) += 1;
                    accounted.insert(id);
                }
            }
        }

        // Step 5: Check globals and scopes
        let mut other_roots: HashMap<String, usize> = HashMap::new();
        for (name, r) in &self.globals {
            if let Some(id) = ref_id(r).filter(|id| new_datum_ids.contains(id)) {
                *other_roots.entry(format!("global.{:?}", name)).or_insert(0) += 1;
                accounted.insert(id);
            }
        }
        for i in 0..self.scopes.len() {
            let stack_refs = {
                let (scopes, allocator, bitmap_manager) = (
                    &mut self.scopes,
                    &mut self.allocator,
                    &mut self.bitmap_manager,
                );
                scopes[i]
                    .stack
                    .snapshot_refs_with(allocator, bitmap_manager)
            };
            let scope = &self.scopes[i];
            let pfx = if i >= self.scope_count as usize {
                "STALE"
            } else {
                "active"
            };
            for r in &stack_refs {
                if let Some(id) = ref_id(r).filter(|id| new_datum_ids.contains(id)) {
                    *other_roots
                        .entry(format!("{}[{}].stack", pfx, i))
                        .or_insert(0) += 1;
                    accounted.insert(id);
                }
            }
            for r in scope.locals.iter().filter_map(|v| match v {
                crate::player::scope::StackDatum::Ref(r) => Some(r),
                _ => None,
            }) {
                if let Some(id) = ref_id(r).filter(|id| new_datum_ids.contains(id)) {
                    *other_roots
                        .entry(format!("{}[{}].locals", pfx, i))
                        .or_insert(0) += 1;
                    accounted.insert(id);
                }
            }
            if let Some(id) = ref_id(&scope.return_value).filter(|id| new_datum_ids.contains(id)) {
                *other_roots
                    .entry(format!("{}[{}].retval", pfx, i))
                    .or_insert(0) += 1;
                accounted.insert(id);
            }
        }
        if let Some(id) = ref_id(&self.last_handler_result).filter(|id| new_datum_ids.contains(id))
        {
            *other_roots
                .entry("last_handler_result".to_string())
                .or_insert(0) += 1;
            accounted.insert(id);
        }

        // Build report
        let mut result = format!(
            "=== Datum Leak Scan v3 ===\nSnapshot: {}, Total new: {}\n\n",
            snapshot_id,
            new_datum_ids.len()
        );

        result.push_str("New datums by type:\n");
        let mut ns: Vec<_> = type_counts_new.into_iter().collect();
        ns.sort_by(|a, b| b.1.cmp(&a.1));
        for (t, c) in &ns {
            result.push_str(&format!("  {}: {}\n", t, c));
        }

        result.push_str(&format!("\nOLD containers with new items (GROWING):\n"));
        growing_containers.sort_by(|a, b| b.1.cmp(&a.1));
        for (desc, _) in growing_containers.iter().take(25) {
            result.push_str(&format!("  {}\n", desc));
        }

        if !new_subtree_types.is_empty() {
            result.push_str(&format!("\nNew compound sub-trees:\n"));
            let mut nst: Vec<_> = new_subtree_types.into_iter().collect();
            nst.sort_by(|a, b| b.1.cmp(&a.1));
            for (t, c) in nst.iter().take(15) {
                result.push_str(&format!("  {} x{}\n", t, c));
            }
        }

        if !si_new.is_empty() {
            result.push_str(&format!("\nScript props with new datums:\n"));
            let mut si: Vec<_> = si_new.into_iter().collect();
            si.sort_by(|a, b| b.1.cmp(&a.1));
            for (k, c) in si.iter().take(15) {
                result.push_str(&format!("  {}: {}\n", k, c));
            }
        }

        if !other_roots.is_empty() {
            result.push_str(&format!("\nOther roots with new datums:\n"));
            let mut or: Vec<_> = other_roots.into_iter().collect();
            or.sort_by(|a, b| b.1.cmp(&a.1));
            for (k, c) in or.iter().take(10) {
                result.push_str(&format!("  {}: {}\n", k, c));
            }
        }

        let unacc = new_datum_ids.len() - accounted.len();
        result.push_str(&format!(
            "\nAccounted: {}, Unaccounted: {}\n",
            accounted.len(),
            unacc
        ));

        // Step 6: Inspect top growing containers - show content samples and ownership chain
        // Collect top 3 growing container IDs from the sorted list
        let top_container_ids: Vec<usize> = growing_containers
            .iter()
            .take(3)
            .filter_map(|(desc, _)| {
                // Parse the datum ID from the description string "OLD ...List #XXXX ..."
                desc.find('#').and_then(|start| {
                    let after_hash = &desc[start + 1..];
                    after_hash
                        .split(|c: char| !c.is_ascii_digit())
                        .next()
                        .and_then(|s| s.parse::<usize>().ok())
                })
            })
            .collect();

        for &cid in &top_container_ids {
            if let Some(entry) = self.allocator.get_datum_entry(cid) {
                result.push_str(&format!("\n--- Inspect #{} ---\n", cid));

                // Show content sample
                match &entry.datum {
                    Datum::List(_, items, _) => {
                        result.push_str(&format!("List len={}\n", items.len()));
                        // First 5 items
                        result.push_str("First 5: ");
                        for (i, r) in items.iter().enumerate().take(5) {
                            if i > 0 {
                                result.push_str(", ");
                            }
                            result.push_str(&format!("{:?}", r));
                        }
                        result.push('\n');
                        // Last 5 items
                        let start = if items.len() > 5 { items.len() - 5 } else { 0 };
                        result.push_str("Last 5: ");
                        for (i, r) in items.iter().skip(start).enumerate() {
                            if i > 0 {
                                result.push_str(", ");
                            }
                            result.push_str(&format!("{:?}", r));
                        }
                        result.push('\n');
                    }
                    _ => {
                        result.push_str(&format!("Type: {}\n", entry.datum.type_str()));
                    }
                }

                // Trace ownership: who holds a reference to this datum?
                result.push_str("Stored in: ");
                let mut found_owner = false;

                // Check script instance properties
                for (si_id, si_entry) in self.allocator.iter_script_instances() {
                    for (prop_name, r) in &si_entry.script_instance.properties {
                        if ref_id(r) == Some(cid) {
                            let sn = self
                                .movie
                                .cast_manager
                                .get_script_by_ref(&si_entry.script_instance.script)
                                .map(|s| s.name.clone())
                                .unwrap_or_else(|| format!("si#{}", si_id));
                            result.push_str(&format!("si({}).{:?}", sn, prop_name));
                            found_owner = true;
                        }
                    }
                }

                // Check globals
                if !found_owner {
                    for (name, r) in &self.globals {
                        if ref_id(r) == Some(cid) {
                            result.push_str(&format!("global.{:?}", name));
                            found_owner = true;
                        }
                    }
                }

                // Check inside compound datums (one level up)
                if !found_owner {
                    for (pid, pentry) in self.allocator.iter_datums() {
                        if pid == cid {
                            continue;
                        }
                        let contains = match &pentry.datum {
                            Datum::List(_, items, _) => {
                                items.iter().any(|r| ref_id(r) == Some(cid))
                            }
                            Datum::PropList(pairs, _) => pairs
                                .iter()
                                .any(|(k, v)| ref_id(k) == Some(cid) || ref_id(v) == Some(cid)),
                            _ => false,
                        };
                        if contains {
                            let prc = unsafe { *pentry.ref_count.get() };
                            result.push_str(&format!(
                                "inside {} #{} (rc={})",
                                pentry.datum.type_str(),
                                pid,
                                prc
                            ));

                            // Trace one more level: who holds the parent?
                            for (si_id, si_entry) in self.allocator.iter_script_instances() {
                                for (prop_name, r) in &si_entry.script_instance.properties {
                                    if ref_id(r) == Some(pid) {
                                        let sn = self
                                            .movie
                                            .cast_manager
                                            .get_script_by_ref(&si_entry.script_instance.script)
                                            .map(|s| s.name.clone())
                                            .unwrap_or_else(|| format!("si#{}", si_id));
                                        result.push_str(&format!(
                                            " ← si({}).{}",
                                            sn,
                                            format!("{:?}", prop_name)
                                        ));
                                    }
                                }
                            }
                            for (name, r) in &self.globals {
                                if ref_id(r) == Some(pid) {
                                    result.push_str(&format!(" ← global.{:?}", name));
                                }
                            }

                            found_owner = true;
                            break;
                        }
                    }
                }

                if !found_owner {
                    result.push_str("(unknown - not found in script props, globals, or compounds)");
                }
                result.push('\n');
            }
        }

        result
    }

    /// Case-insensitive lookup into `external_params`. The browser lowercases
    /// every HTML `<embed>` attribute name, so a movie's `_runMode` /
    /// `_moviePath` param arrives as `_runmode` / `_moviepath` when delivered
    /// via the Shockwave polyfill's `<embed>` path. Director treats external
    /// parameter names case-insensitively (see `external_param_value`), so the
    /// internal lookups for these special params must too — otherwise an exact
    /// `.get("_runMode")` misses and the movie silently falls back to defaults
    /// (e.g. runMode "Plugin", which trips server-license checks).
    fn external_param_ci(&self, key: &str) -> Option<String> {
        self.external_params
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.clone())
    }

    /// `the runMode` / `the environment.runMode` / `_player.runMode`.
    /// LeechProtectionRemovalHelp's `setTheRunMode` outranks the `_runMode`
    /// external param, which in turn outranks the "Plugin" default.
    fn effective_run_mode(&self) -> String {
        self.env_overrides
            .run_mode
            .clone()
            .or_else(|| self.external_param_ci("_runMode"))
            .unwrap_or_else(|| "Plugin".to_string())
    }

    /// `the platform` / `the environment.platform`.
    fn effective_platform(&self) -> String {
        self.env_overrides
            .platform
            .clone()
            .unwrap_or_else(|| "Windows,32".to_string())
    }

    /// `the productVersion` / `the environment.productVersion` /
    /// `_player.productVersion`.
    fn effective_product_version(&self) -> String {
        self.env_overrides
            .product_version
            .clone()
            .unwrap_or_else(|| "11.0".to_string())
    }

    /// The fake `the moviePath` label, highest precedence first:
    ///   1. LeechProtectionRemovalHelp's `setTheMoviePath`
    ///   2. `external_params["_moviePath"]`
    ///   3. `movie_path_label` (the `set_movie_path_label` JS API)
    /// `None` means "report the real loaded path". Callers still reduce the
    /// label to its directory / filename part — the Xtra is documented with a
    /// directory (`"http://addictinggames.com/newGames/foo/"`), but the other
    /// two sources may carry a full movie URL.
    fn movie_path_label_effective(&self) -> Option<String> {
        self.env_overrides
            .movie_path
            .clone()
            .filter(|s| !s.is_empty())
            .or_else(|| {
                self.external_param_ci("_moviePath")
                    .filter(|s| !s.is_empty())
            })
            .or_else(|| {
                self.movie_path_label
                    .as_ref()
                    .filter(|s| !s.is_empty())
                    .cloned()
            })
    }

    /// The film loop the innermost active `tell` block targets, if any.
    fn tell_film_loop(&self) -> Option<CastMemberRef> {
        self.tell_target_stack
            .last()
            .and_then(|t| t.film_loop.clone())
    }

    fn alloc_movie_prop(
        &mut self,
        symbols: &mut SymbolTable,
        prop: Symbol,
    ) -> Result<DatumRef, ScriptError> {
        let datum = Movie::get_prop(self, symbols, prop)?;
        self.validate_movie_datum_shallow(symbols, &datum)?;
        Ok(self.alloc_datum(datum))
    }

    /// Validate values as they cross the movie/property boundary without
    /// walking an arbitrary datum graph. Direct symbol and script-instance
    /// fields are owned by this session, and a retained list validates only
    /// its immediate entries. Nested lists and prop-lists remain opaque so
    /// VOID entries and cycles preserve Director's value semantics.
    pub(crate) fn validate_movie_datum_shallow(
        &self,
        symbols: &SymbolTable,
        datum: &Datum,
    ) -> Result<(), ScriptError> {
        self.validate_movie_datum_direct(symbols, datum)?;
        match datum {
            Datum::List(_, items, _) => {
                for item_ref in items {
                    self.validate_movie_datum_ref_shallow(symbols, item_ref)?;
                }
            }
            Datum::PropList(items, _) => {
                for (key_ref, value_ref) in items {
                    self.validate_movie_datum_ref_shallow(symbols, key_ref)?;
                    self.validate_movie_datum_ref_shallow(symbols, value_ref)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn validate_movie_datum_ref_shallow(
        &self,
        symbols: &SymbolTable,
        datum_ref: &DatumRef,
    ) -> Result<(), ScriptError> {
        let datum = match datum_ref {
            DatumRef::Void => &Datum::Void,
            _ => self.allocator.try_get_datum(datum_ref).ok_or_else(|| {
                ScriptError::new_code(
                    ScriptErrorCode::InvalidReference,
                    format!("invalid datum reference {datum_ref}"),
                )
            })?,
        };
        self.validate_movie_datum_direct(symbols, datum)
    }

    fn validate_movie_datum_direct(
        &self,
        symbols: &SymbolTable,
        datum: &Datum,
    ) -> Result<(), ScriptError> {
        crate::player::compare::validate_direct_symbol_fields(datum, symbols)?;
        if let Datum::ScriptInstanceRef(instance_ref) = datum {
            self.allocator
                .get_script_instance_opt(instance_ref)
                .ok_or_else(|| {
                    ScriptError::new_code(
                        ScriptErrorCode::InvalidReference,
                        "foreign or stale ScriptInstanceRef".to_owned(),
                    )
                })?;
        }
        Ok(())
    }

    fn checked_movie_datum_ref<'a>(
        &'a self,
        symbols: &SymbolTable,
        datum_ref: &DatumRef,
    ) -> Result<&'a Datum, ScriptError> {
        let datum = match datum_ref {
            DatumRef::Void => &Datum::Void,
            _ => self.allocator.try_get_datum(datum_ref).ok_or_else(|| {
                ScriptError::new_code(
                    ScriptErrorCode::InvalidReference,
                    format!("invalid datum reference {datum_ref}"),
                )
            })?,
        };
        self.validate_movie_datum_direct(symbols, datum)?;
        Ok(datum)
    }

    pub(crate) fn get_movie_prop(
        &mut self,
        symbols: &mut SymbolTable,
        prop: Symbol,
    ) -> Result<DatumRef, ScriptError> {
        symbols.lower(&prop).map_err(|_| SymbolError::Foreign)?;
        let builtin_prop = prop.into_builtin_or_error(symbols)?;
        match builtin_prop {
            BuiltInSymbol::DatumStats => {
                let stats = self.allocator.datum_type_stats();
                web_sys::console::log_1(&stats.clone().into());
                Ok(self.alloc_datum(Datum::String(stats)))
            }
            BuiltInSymbol::DatumSnapshot => {
                self.allocator.take_datum_snapshot();
                debug!(
                    "Datum snapshot taken. Use 'put the datumStats' to see new datums since snapshot."
                );
                Ok(DatumRef::Void)
            }
            BuiltInSymbol::DatumLeakScan => {
                let stats = self.datum_leak_scan();
                web_sys::console::log_1(&stats.clone().into());
                Ok(self.alloc_datum(Datum::String(stats)))
            }
            BuiltInSymbol::SystemDate => {
                let date_id = self.allocator.get_free_script_instance_id();
                let date_obj =
                    crate::player::handlers::datum_handlers::date::DateObject::new(date_id);
                self.date_objects.insert(date_id, date_obj);
                Ok(self.alloc_datum(Datum::DateRef(date_id)))
            }
            BuiltInSymbol::Stage => Ok(self.alloc_datum(Datum::Stage)),
            BuiltInSymbol::Globals => {
                // Director 11.5 Scripting Dictionary — `the globals`:
                // a special property list of every current global variable
                // whose value is not VOID, each keyed by its name (symbol)
                // paired with the value. Supports count/getPropAt/getProp/
                // getAProp via the normal PropList handlers. The list always
                // contains #version (Director's running version), so it is
                // never empty even before any global is declared.
                //
                // dirplayer stores the Lingo language constants (PI, EMPTY,
                // QUOTE, TAB, TRUE, FALSE, …) as globals internally for
                // convenience, but Director does NOT expose those as global
                // variables, so they're excluded here to match the spec.
                // actorList is a genuine global and is kept.
                const CONST_GLOBALS: &[&str] = &[
                    "PI",
                    "VOID",
                    "EMPTY",
                    "RETURN",
                    "ENTER",
                    "QUOTE",
                    "TAB",
                    "BACKSPACE",
                    "TRUE",
                    "FALSE",
                ];
                let mut entries = Vec::new();
                for (name, value_ref) in &self.globals {
                    let lower = symbols.lower(name).map_err(|_| SymbolError::Foreign)?;
                    // Constant-name filtering happens before value resolution:
                    // Director does not expose language constants as globals,
                    // even if a stale or foreign value ref is attached to one.
                    if CONST_GLOBALS
                        .iter()
                        .any(|constant| lower.eq_ignore_ascii_case(constant))
                    {
                        continue;
                    }
                    let value = self.checked_movie_datum_ref(symbols, value_ref)?;
                    if matches!(value, Datum::Void) {
                        continue;
                    }
                    entries.push((name.clone(), value_ref.clone()));
                }
                let mut props: VecDeque<(DatumRef, DatumRef)> = entries
                    .into_iter()
                    .map(|(name, value_ref)| (self.alloc_datum(Datum::Symbol(name)), value_ref))
                    .collect();
                let version_key =
                    self.alloc_datum(Datum::Symbol(symbols.intern_authoritative("version")));
                let version_val = self.alloc_datum(Datum::String("11.0".to_string()));
                props.push_back((version_key, version_val));
                Ok(self.alloc_datum(Datum::PropList(props, false)))
            }
            BuiltInSymbol::Time => Ok(self.alloc_datum(Datum::String(
                chrono::Local::now().format("%H:%M %p").to_string(),
            ))),
            BuiltInSymbol::MilliSeconds => {
                // Reading a clock in a loop condition is a time-based busy-wait
                // (`repeat while (the milliSeconds - t0) < N`); mark it so the
                // backward-jump handler yields cooperatively (see input_polled).
                self.input_polled = true;
                // `Utc`, not `Local`: the difference is identical (both are
                // absolute epoch instants) but `Local::now()` re-resolves the
                // timezone offset on every call under WASM. This is polled in
                // busy-wait loops, so it ran constantly.
                Ok(self.alloc_datum(Datum::Int(
                    (chrono::Utc::now().timestamp_millis()
                        - self.system_start_time.timestamp_millis()) as i32,
                )))
            }
            BuiltInSymbol::KeyboardFocusSprite => {
                Ok(self.alloc_datum(Datum::Int(self.keyboard_focus_sprite as i32)))
            }
            BuiltInSymbol::Selection => {
                // Returns the currently selected text in the focused editable
                // Field/Text member, or "" if none.
                let s = if self.keyboard_focus_sprite >= 0 {
                    let sprite_id = self.keyboard_focus_sprite as i16;
                    let sprite = self.movie.score.get_sprite(sprite_id);
                    let member = sprite
                        .and_then(|s| s.member.as_ref())
                        .and_then(|m| self.movie.cast_manager.find_member_by_ref(m));
                    match member.map(|m| &m.member_type) {
                        Some(crate::player::cast_member::CastMemberType::Field(f))
                            if f.editable =>
                        {
                            let len = f.text.len() as i32;
                            let lo = f.sel_start.min(f.sel_end).clamp(0, len);
                            let hi = f.sel_start.max(f.sel_end).clamp(0, len);
                            f.text[lo as usize..hi as usize].to_string()
                        }
                        Some(crate::player::cast_member::CastMemberType::Text(t))
                            if t.info.as_ref().map_or(false, |i| i.editable) =>
                        {
                            let len = t.text.len() as i32;
                            let lo = t.sel_start.min(t.sel_end).clamp(0, len);
                            let hi = t.sel_start.max(t.sel_end).clamp(0, len);
                            t.text[lo as usize..hi as usize].to_string()
                        }
                        _ => String::new(),
                    }
                } else {
                    String::new()
                };
                Ok(self.alloc_datum(Datum::String(s)))
            }
            BuiltInSymbol::ClipBoard => {
                Ok(self.alloc_datum(Datum::String(self.clipboard_mirror.clone())))
            }
            BuiltInSymbol::FrameTempo => {
                // Get tempo from current frame in score, or use default frame_rate
                let frame_tempo = self
                    .movie
                    .score
                    .get_frame_tempo(self.movie.current_frame)
                    .unwrap_or(self.movie.frame_rate as u32);
                Ok(self.alloc_datum(Datum::Int(frame_tempo as i32)))
            }
            BuiltInSymbol::MouseLoc => Ok(self.alloc_datum(Datum::Point(
                [self.mouse_loc.0 as f64, self.mouse_loc.1 as f64],
                0,
            ))),
            BuiltInSymbol::MouseH => Ok(self.alloc_datum(Datum::Int(self.mouse_loc.0 as i32))),
            BuiltInSymbol::MouseV => Ok(self.alloc_datum(Datum::Int(self.mouse_loc.1 as i32))),
            BuiltInSymbol::MouseMember => {
                let datum = mouse_member_datum(self);
                Ok(self.alloc_datum(datum))
            }
            BuiltInSymbol::MouseChar => {
                let val = compute_mouse_char(self);
                Ok(self.alloc_datum(Datum::Int(val)))
            }
            BuiltInSymbol::MouseLine => {
                let val = compute_mouse_line(self);
                Ok(self.alloc_datum(Datum::Int(val)))
            }
            BuiltInSymbol::StillDown => {
                self.input_polled = true;
                Ok(self.alloc_datum(datum_bool(self.movie.mouse_down)))
            }
            BuiltInSymbol::Rollover => {
                let sprite = get_sprite_at(self, self.mouse_loc.0, self.mouse_loc.1, false);
                Ok(self.alloc_datum(Datum::Int(sprite.unwrap_or(0) as i32)))
            }
            BuiltInSymbol::KeyCode => {
                Ok(self.alloc_datum(Datum::Int(self.keyboard_manager.key_code() as i32)))
            }
            BuiltInSymbol::ShiftDown => {
                Ok(self.alloc_datum(datum_bool(self.keyboard_manager.is_shift_down())))
            }
            BuiltInSymbol::OptionDown => {
                Ok(self.alloc_datum(datum_bool(self.keyboard_manager.is_alt_down())))
            }
            BuiltInSymbol::CommandDown => {
                Ok(self.alloc_datum(datum_bool(self.keyboard_manager.is_command_down())))
            }
            BuiltInSymbol::ControlDown => {
                Ok(self.alloc_datum(datum_bool(self.keyboard_manager.is_control_down())))
            }
            BuiltInSymbol::AltDown => {
                Ok(self.alloc_datum(datum_bool(self.keyboard_manager.is_alt_down())))
            }
            BuiltInSymbol::Key => Ok(self.alloc_datum(Datum::String(self.keyboard_manager.key()))),
            BuiltInSymbol::KeyPressed => {
                self.input_polled = true;
                Ok(self.alloc_datum(Datum::String(self.keyboard_manager.key_pressed())))
            }
            BuiltInSymbol::FloatPrecision => {
                Ok(self.alloc_datum(Datum::Int(self.float_precision as i32)))
            }
            BuiltInSymbol::DoubleClick => Ok(self.alloc_datum(datum_bool(self.is_double_click))),
            // Time read in a loop condition = busy-wait throttle; flag it so the
            // backward-jump handler yields cooperatively (see input_polled).
            BuiltInSymbol::Ticks => {
                self.input_polled = true;
                Ok(self.alloc_datum(Datum::Int(get_elapsed_ticks(self.system_start_time))))
            }
            BuiltInSymbol::FrameLabel => {
                let frame_label = self
                    .movie
                    .score
                    .frame_labels
                    .iter()
                    .filter(|&label| label.frame_num <= self.movie.current_frame as i32)
                    .max_by_key(|label| label.frame_num)
                    .map(|label| label.label.clone());
                Ok(self.alloc_datum(Datum::String(
                    frame_label.unwrap_or_else(|| "0".to_string()),
                )))
            }
            BuiltInSymbol::CurrentSpriteNum => {
                // TODO: this can also be called by a static script
                let script_instance_ref = self
                    .scopes
                    .get(self.current_scope_ref())
                    .and_then(|scope| scope.receiver.clone());

                if let Some(script_instance_ref) = script_instance_ref {
                    // Try to get spriteNum from the script instance
                    if let Some(datum_ref) = script_get_prop_opt(
                        self,
                        symbols,
                        &script_instance_ref,
                        Symbol::builtin(BuiltInSymbol::SpriteNum),
                    )? {
                        let datum = self.checked_movie_datum_ref(symbols, &datum_ref)?;
                        // Check if it's Void - if so, return 0 as default
                        if !matches!(datum, Datum::Void) {
                            if let Ok(sprite_num) = datum.int_value() {
                                return Ok(self.alloc_datum(Datum::Int(sprite_num)));
                            }
                        }
                    }
                }

                // A cast member script runs with no receiver, so fall back to
                // the sprite the message was dispatched to.
                if self.member_script_sprite_num != 0 {
                    return Ok(self.alloc_datum(Datum::Int(self.member_script_sprite_num as i32)));
                }

                // Mouse events reach a member script through a different dispatch
                // path than the frame events above, and that path doesn't stamp
                // `member_script_sprite_num` — so `the currentSpriteNum` read 0 in
                // an `on mouseDown` member script. Director answers with the sprite
                // the click went to. dkbarrel's menu buttons are member scripts that
                // identify themselves with
                //     ind = getPos(btnspr, the currentSpriteNum)
                // and 0 made getPos return 0, so `getAt(btnflash, 0)` raised
                // "Index 0 out of bounds" when clicking HELP.
                if self.click_on_sprite > 0 {
                    return Ok(self.alloc_datum(Datum::Int(self.click_on_sprite as i32)));
                }

                // Default: return 0 when no sprite context is available
                Ok(self.alloc_datum(Datum::Int(0)))
            }
            BuiltInSymbol::ActorList => {
                // Return the reference to the global actorList, not a clone of its contents
                let actor_list_ref = self
                    .globals
                    .get(&Symbol::builtin(BuiltInSymbol::ActorList))
                    .unwrap_or(&DatumRef::Void)
                    .clone();
                if !matches!(actor_list_ref, DatumRef::Void) {
                    let actor_list = self.checked_movie_datum_ref(symbols, &actor_list_ref)?;
                    self.validate_movie_datum_shallow(symbols, actor_list)?;
                }
                Ok(actor_list_ref)
            }
            BuiltInSymbol::ClickOn => Ok(self.alloc_datum(Datum::Int(self.click_on_sprite as i32))),
            BuiltInSymbol::Environment | BuiltInSymbol::EnvironmentPropList => {
                // Build the environment property list. Seven of these entries
                // are overridable by the LeechProtectionRemovalHelp Xtra, whose
                // whole purpose is to make an archived movie's environment
                // check see the values it saw on its original host.
                let props = VecDeque::from(vec![
                    (
                        self.alloc_datum(Datum::Symbol(Symbol::builtin(
                            BuiltInSymbol::ShockMachine,
                        ))),
                        self.alloc_datum(Datum::Int(self.env_overrides.shock_machine.unwrap_or(0))),
                    ),
                    (
                        self.alloc_datum(Datum::Symbol(Symbol::builtin(
                            BuiltInSymbol::ShockMachineVersion,
                        ))),
                        self.alloc_datum(Datum::String(
                            self.env_overrides
                                .shock_machine_version
                                .clone()
                                .unwrap_or_default(),
                        )),
                    ),
                    (
                        self.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::Platform))),
                        self.alloc_datum(Datum::String(self.effective_platform())),
                    ),
                    (
                        self.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::RunMode))),
                        self.alloc_datum(Datum::String(self.effective_run_mode())),
                    ),
                    (
                        self.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::ColorDepth))),
                        self.alloc_datum(Datum::Int(32)),
                    ),
                    (
                        self.alloc_datum(Datum::Symbol(Symbol::builtin(
                            BuiltInSymbol::InternetConnected,
                        ))),
                        self.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::Online))),
                    ),
                    (
                        self.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::UILanguage))),
                        self.alloc_datum(Datum::String("English".to_string())),
                    ),
                    (
                        self.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::OSLanguage))),
                        self.alloc_datum(Datum::String("English".to_string())),
                    ),
                    (
                        self.alloc_datum(Datum::Symbol(Symbol::builtin(
                            BuiltInSymbol::ProductBuildVersion,
                        ))),
                        self.alloc_datum(Datum::String(
                            self.env_overrides
                                .product_build_version
                                .clone()
                                .unwrap_or_else(|| "188".to_string()),
                        )),
                    ),
                    (
                        self.alloc_datum(Datum::Symbol(Symbol::builtin(
                            BuiltInSymbol::ProductVersion,
                        ))),
                        self.alloc_datum(Datum::String(self.effective_product_version())),
                    ),
                    (
                        self.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::OSVersion))),
                        self.alloc_datum(Datum::String(
                            self.env_overrides.os_version.clone().unwrap_or_else(|| {
                                "Windows XP,5,1,148,2,Service Pack 3".to_string()
                            }),
                        )),
                    ),
                    (
                        self.alloc_datum(Datum::Symbol(Symbol::builtin(
                            BuiltInSymbol::DirectXVersion,
                        ))),
                        self.alloc_datum(Datum::String("9.0.0".to_string())),
                    ),
                    (
                        self.alloc_datum(Datum::Symbol(Symbol::builtin(
                            BuiltInSymbol::LicenseType,
                        ))),
                        self.alloc_datum(Datum::String("Full".to_string())),
                    ),
                    (
                        self.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::TrialTime))),
                        self.alloc_datum(Datum::Int(0)),
                    ),
                ]);
                Ok(self.alloc_datum(Datum::PropList(props, false)))
            }
            BuiltInSymbol::ClickLoc => Ok(self.alloc_datum(Datum::Point(
                [self.movie.click_loc.0 as f64, self.movie.click_loc.1 as f64],
                0,
            ))),
            // Director 11.5 Scripting Dictionary, `timeoutList` (Movie property,
            // read-only): "a linear list containing all currently active timeout
            // objects", indexable — its own example is
            // `_movie.timeoutList[3].forget()`. Built here rather than in
            // `Movie::get_prop` because the elements need the allocator; the
            // collection-query fallback in `datum_handlers/mod.rs` routes
            // `.count` and `[n]` to this. AreaZero's `[M] Event Manager.DelayEvent`
            // names each pending timeout by `_movie.timeoutList.count + 1`.
            BuiltInSymbol::TimeoutList => {
                let names: Vec<String> = self.timeout_manager.timeouts.keys().cloned().collect();
                let items: VecDeque<DatumRef> = names
                    .into_iter()
                    .map(|name| self.alloc_datum(Datum::TimeoutRef(name)))
                    .collect();
                Ok(self.alloc_datum(Datum::List(DatumType::List, items, false)))
            }
            BuiltInSymbol::MarkerList => {
                // Director's `the markerList` is a property list keyed by FRAME
                // NUMBER with the label as the value: `[23: "ts_i04", 232: "Skelly", …]`
                // (not `["Skelly": 232]`). Keep that order so movie code that reads
                // `the markerList` (frame → label) works.
                let labels: Vec<_> = self
                    .movie
                    .score
                    .frame_labels
                    .iter()
                    .map(|fl| (fl.frame_num, fl.label.clone()))
                    .collect();
                let props: VecDeque<(DatumRef, DatumRef)> = labels
                    .into_iter()
                    .map(|(frame_num, label)| {
                        let frame_num = self.alloc_datum(Datum::Int(frame_num));
                        let label = self.alloc_datum(Datum::String(label));
                        (frame_num, label)
                    })
                    .collect();
                Ok(self.alloc_datum(Datum::PropList(props, false)))
            }
            BuiltInSymbol::XtraList => {
                let xtra_names = xtra::manager::get_registered_xtra_names(self);
                let xtra_list: VecDeque<DatumRef> = xtra_names
                    .iter()
                    .map(|name| {
                        let name_key =
                            self.alloc_datum(Datum::Symbol(Symbol::builtin(BuiltInSymbol::Name)));
                        let name_val = self.alloc_datum(Datum::String(name.to_string()));
                        self.alloc_datum(Datum::PropList(
                            VecDeque::from(vec![(name_key, name_val)]),
                            false,
                        ))
                    })
                    .collect();
                Ok(self.alloc_datum(Datum::List(
                    crate::director::lingo::datum::DatumType::List,
                    xtra_list,
                    false,
                )))
            }
            BuiltInSymbol::RunMode => {
                let mode = self.effective_run_mode();
                Ok(self.alloc_datum(Datum::String(mode)))
            }
            // Faked wholesale by LeechProtectionRemovalHelp; otherwise these
            // are dirplayer's fixed "Windows Shockwave" identity, resolved in
            // `Movie::get_prop`.
            BuiltInSymbol::Platform => {
                let platform = self.effective_platform();
                Ok(self.alloc_datum(Datum::String(platform)))
            }
            BuiltInSymbol::ProductVersion => {
                let version = self.effective_product_version();
                Ok(self.alloc_datum(Datum::String(version)))
            }
            BuiltInSymbol::MachineType => match self.env_overrides.machine_type {
                Some(v) => Ok(self.alloc_datum(Datum::Int(v))),
                None => {
                    return self.alloc_movie_prop(symbols, prop);
                }
            },
            // `the path` / `_movie.path` is the same directory string as
            // `the moviePath`, and `setTheMoviePath` fakes both.
            BuiltInSymbol::Path => {
                match self
                    .env_overrides
                    .movie_path
                    .clone()
                    .filter(|s| !s.is_empty())
                {
                    Some(path) => Ok(self.alloc_datum(Datum::String(dir_part_of_path(&path)))),
                    None => {
                        return self.alloc_movie_prop(symbols, prop);
                    }
                }
            }
            // `forceTheExitLock` pins the value — see `Movie::set_prop`, which
            // drops the movie's own writes while a forced value is installed.
            BuiltInSymbol::ExitLock => match self.env_overrides.forced_exit_lock {
                Some(v) => Ok(self.alloc_datum(datum_bool(v))),
                None => {
                    return self.alloc_movie_prop(symbols, prop);
                }
            },
            BuiltInSymbol::SafePlayer => {
                let value = self.env_overrides.forced_safe_player.unwrap_or(false);
                Ok(self.alloc_datum(datum_bool(value)))
            }
            // `the moviePath` precedence (read at every Lingo access so
            // it stays correct even if external_params change post-load):
            //   1. external_params["_moviePath"]  — declarative label
            //   2. movie_path_label               — JS API label
            //   3. self.movie.base_path           — actual loaded path
            //      (handled by Movie::get_prop in the fallthrough below)
            // Director's `the moviePath` is the directory part of the
            // movie's location — NOT the full file URL. So if the label
            // looks like a full URL (`http://host/dir/movie.dcr`), strip
            // the filename and return `http://host/dir/`. This matches
            // the load-time logic in `load_movie_from_dir`. Label sources
            // do NOT trigger URL rewriting in net handlers — that's
            // reserved for `movie_path_override`. See `set_movie_path_label`.
            BuiltInSymbol::MoviePath => {
                let label = self.movie_path_label_effective();
                if let Some(path) = label {
                    let base_path = dir_part_of_path(&path);
                    Ok(self.alloc_datum(Datum::String(base_path)))
                } else {
                    return self.alloc_movie_prop(symbols, prop);
                }
            }
            // `the movieName` mirrors `the moviePath` — when a label is
            // active, return the FILENAME portion (last path segment).
            // Scripts use `the moviePath & the movieName` to reconstruct
            // the full URL for security/whitelist checks. Without this
            // companion intercept, `movieName` would still come from the
            // actually-loaded file, breaking the concatenation.
            // `the movie` and `_movie.name` are aliases of `the movieName`
            // (see `Movie::get_prop`), and both the label logic below and
            // `setTheMovieName` are documented to cover all three.
            BuiltInSymbol::MovieName | BuiltInSymbol::Movie | BuiltInSymbol::Name => {
                // `setTheMovieName` is a name in its own right — the Xtra takes
                // the path and the name as two separate calls, and its own
                // usage example passes a bare filename ("foo.dcr") to this one.
                // So it wins outright rather than being derived from a path.
                if let Some(name) = self
                    .env_overrides
                    .movie_name
                    .clone()
                    .filter(|s| !s.is_empty())
                {
                    return Ok(self.alloc_datum(Datum::String(name)));
                }
                let label = self.movie_path_label_effective();
                if let Some(path) = label {
                    let file_name = if let Ok(url) = Url::parse(&path) {
                        url.path_segments()
                            .and_then(|s| s.last())
                            .filter(|s| !s.is_empty())
                            .map(|s| s.to_string())
                            .unwrap_or_default()
                    } else if let Some(pos) = path.rfind(|c| c == '/' || c == '\\') {
                        path[pos + 1..].to_string()
                    } else {
                        path.clone()
                    };
                    Ok(self.alloc_datum(Datum::String(file_name)))
                } else {
                    return self.alloc_movie_prop(symbols, prop);
                }
            }
            // Score reads inside a `tell <film loop sprite>` block resolve
            // against that film loop's own playhead, not the movie's. A film
            // loop runs an independent score, and this is the idiom scripts use
            // to notice one has played out:
            //   tell msgSprite
            //     f = the frame
            //   end tell
            //   tell msgSprite
            //     e = the lastFrame
            //   end tell
            //   if f = e then <clear the message>
            // Read against the movie those two are unrelated to the animation
            // and never coincide, so the sprite is never cleared.
            BuiltInSymbol::Frame | BuiltInSymbol::LastFrame => {
                let prop_text = symbols.lower(&prop).map_err(|_| SymbolError::Foreign)?;
                let value = self.tell_film_loop().and_then(|member_ref| {
                    self.movie
                        .cast_manager
                        .find_member_by_ref(&member_ref)
                        .and_then(|m| match &m.member_type {
                            CastMemberType::FilmLoop(film_loop) => {
                                Some(if prop_text.eq_ignore_ascii_case("frame") {
                                    film_loop.current_frame.max(1)
                                } else {
                                    film_loop.score.frame_count.unwrap_or(1).max(1)
                                })
                            }
                            _ => None,
                        })
                });
                match value {
                    Some(v) => Ok(self.alloc_datum(Datum::Int(v as i32))),
                    // Outside a film-loop `tell`, these are ordinary movie props.
                    None => {
                        return self.alloc_movie_prop(symbols, prop);
                    }
                }
            }
            BuiltInSymbol::StageLeft
            | BuiltInSymbol::StageTop
            | BuiltInSymbol::StageRight
            | BuiltInSymbol::StageBottom => {
                let layout = crate::player::stage::stage_layout(self);
                let value = match builtin_prop {
                    BuiltInSymbol::StageLeft => layout.stage_rect[0],
                    BuiltInSymbol::StageTop => layout.stage_rect[1],
                    BuiltInSymbol::StageRight => layout.stage_rect[2],
                    BuiltInSymbol::StageBottom => layout.stage_rect[3],
                    _ => unreachable!(),
                };
                Ok(self.alloc_datum(Datum::Int(value as i32)))
            }
            _ => {
                return self.alloc_movie_prop(symbols, prop);
            }
        }
    }

    fn get_player_prop(
        &mut self,
        symbols: &mut SymbolTable,
        prop: Symbol,
    ) -> Result<DatumRef, ScriptError> {
        match prop.into_builtin() {
            Some(BuiltInSymbol::TraceScript) => Ok(self.alloc_datum(datum_bool(false))), // TODO
            Some(BuiltInSymbol::ProductVersion) => {
                let version = self.effective_product_version();
                Ok(self.alloc_datum(Datum::String(version)))
            }
            Some(BuiltInSymbol::RunMode) => {
                let mode = self.effective_run_mode();
                Ok(self.alloc_datum(Datum::String(mode)))
            }
            Some(BuiltInSymbol::SafePlayer) => {
                let value = self.env_overrides.forced_safe_player.unwrap_or(false);
                Ok(self.alloc_datum(datum_bool(value)))
            }
            // Key state properties (also accessible via _key.optionDown etc.)
            Some(BuiltInSymbol::OptionDown) => {
                Ok(self.alloc_datum(datum_bool(self.keyboard_manager.is_alt_down())))
            }
            Some(BuiltInSymbol::CommandDown) => {
                Ok(self.alloc_datum(datum_bool(self.keyboard_manager.is_command_down())))
            }
            Some(BuiltInSymbol::ControlDown) => {
                Ok(self.alloc_datum(datum_bool(self.keyboard_manager.is_control_down())))
            }
            Some(BuiltInSymbol::ShiftDown) => {
                Ok(self.alloc_datum(datum_bool(self.keyboard_manager.is_shift_down())))
            }
            Some(BuiltInSymbol::AltDown) => {
                Ok(self.alloc_datum(datum_bool(self.keyboard_manager.is_alt_down())))
            }
            Some(BuiltInSymbol::KeyCode) => {
                Ok(self.alloc_datum(Datum::Int(self.keyboard_manager.key_code() as i32)))
            }
            Some(BuiltInSymbol::Key) => {
                Ok(self.alloc_datum(Datum::String(self.keyboard_manager.key())))
            }
            // Several documented Player properties (frontWindow, activeWindow,
            // windowList, memorySize, …) are served from Movie::get_prop, which
            // is where `the <name>` resolves. Fall through so the equivalent
            // `_player.<name>` form reaches the same getter instead of erroring
            // — the Scripting Dictionary gives `_player.frontWindow` as the
            // primary syntax for exactly these.
            _ => self.alloc_movie_prop(symbols, prop),
        }
    }

    fn get_mouse_prop(&mut self, prop: Symbol) -> Result<DatumRef, ScriptError> {
        match prop.into_builtin() {
            Some(BuiltInSymbol::DoubleClick) => {
                Ok(self.alloc_datum(datum_bool(self.is_double_click)))
            }
            Some(BuiltInSymbol::MouseLoc) => Ok(self.alloc_datum(Datum::Point(
                [self.mouse_loc.0 as f64, self.mouse_loc.1 as f64],
                0,
            ))),
            Some(BuiltInSymbol::ClickOn) => {
                Ok(self.alloc_datum(Datum::Int(self.click_on_sprite as i32)))
            }
            // `_mouse.clickLoc` — point where the user last clicked,
            // captured at mouseDown (distinct from mouseLoc which tracks
            // current cursor position). Fugue No.4's Narrative_Float
            // mouseWithin reads `getAt(_mouse.clickLoc, 2)`.
            Some(BuiltInSymbol::ClickLoc) => Ok(self.alloc_datum(Datum::Point(
                [self.movie.click_loc.0 as f64, self.movie.click_loc.1 as f64],
                0,
            ))),
            Some(BuiltInSymbol::MouseH) => {
                Ok(self.alloc_datum(Datum::Int(self.mouse_loc.0 as i32)))
            }
            Some(BuiltInSymbol::MouseV) => {
                Ok(self.alloc_datum(Datum::Int(self.mouse_loc.1 as i32)))
            }
            Some(BuiltInSymbol::MouseMember) => {
                let datum = mouse_member_datum(self);
                Ok(self.alloc_datum(datum))
            }
            Some(BuiltInSymbol::MouseChar) => {
                let val = compute_mouse_char(self);
                Ok(self.alloc_datum(Datum::Int(val)))
            }
            Some(BuiltInSymbol::MouseLine) => {
                let val = compute_mouse_line(self);
                Ok(self.alloc_datum(Datum::Int(val)))
            }
            Some(BuiltInSymbol::MouseDown) => {
                self.input_polled = true;
                Ok(self.alloc_datum(datum_bool(self.movie.mouse_down)))
            }
            Some(BuiltInSymbol::MouseUp) => {
                self.input_polled = true;
                Ok(self.alloc_datum(datum_bool(!self.movie.mouse_down)))
            }
            Some(BuiltInSymbol::StillDown) => {
                self.input_polled = true;
                Ok(self.alloc_datum(datum_bool(self.movie.mouse_down)))
            }
            Some(BuiltInSymbol::RightMouseDown) => {
                Ok(self.alloc_datum(datum_bool(self.movie.right_mouse_down)))
            }
            Some(BuiltInSymbol::RightMouseUp) => {
                Ok(self.alloc_datum(datum_bool(!self.movie.right_mouse_down)))
            }
            _ => Err(ScriptError::new(format!("Unknown _mouse prop {:?}", prop))),
        }
    }

    fn set_mouse_prop(&mut self, prop: Symbol, value_ref: &DatumRef) -> Result<(), ScriptError> {
        match prop.into_builtin() {
            Some(BuiltInSymbol::MouseLoc) => {
                let value = self.get_datum(value_ref).clone();
                match value {
                    Datum::Point(vals, _flags) => {
                        let x = vals[0] as i32;
                        let y = vals[1] as i32;
                        self.mouse_loc = (x, y);
                        // Game is programmatically warping the mouse — this is the FPS mouselook pattern.
                        // Only request pointer lock if cursor is hidden AND 3D content is active.
                        if self.cursor_is_hidden && self.w3d_any_rendered {
                            self.wants_pointer_lock = true;
                        }
                        Ok(())
                    }
                    _ => Err(ScriptError::new(
                        "mouseLoc requires a point value".to_string(),
                    )),
                }
            }
            _ => Err(ScriptError::new(format!(
                "Cannot set _mouse prop {:?}",
                prop
            ))),
        }
    }

    fn set_player_prop(
        &mut self,
        symbols: &SymbolTable,
        prop: Symbol,
        value: &DatumRef,
    ) -> Result<(), ScriptError> {
        match prop.into_builtin() {
            Some(BuiltInSymbol::ItemDelimiter) => {
                let value = self.checked_movie_datum_ref(symbols, value)?;
                self.movie.item_delimiter = (value.string_value(symbols)?).chars().next().unwrap();
                Ok(())
            }
            Some(BuiltInSymbol::TraceScript) => {
                // Accepted, no-op
                Ok(())
            }
            Some(BuiltInSymbol::DebugPlaybackEnabled) => {
                let v = self.checked_movie_datum_ref(symbols, value)?.int_value()? != 0;
                self.movie.debug_playback_enabled = v;
                Ok(())
            }
            // Mirror of `get_player_prop`'s fallthrough: several documented
            // Player properties are served by Movie::set_prop, which is where
            // `the <name> = …` resolves. Without this the `_player.<name> = …`
            // form — the Scripting Dictionary's primary syntax for them —
            // errored while `the <name> = …` worked (Age of Speed 2's Input
            // Manager sets `_player.emulateMultibuttonMouse`).
            _ => {
                // The Movie setter owns ignored/no-op fallback semantics. Resolve the
                // reference here to establish allocator ownership, then leave its
                // payload untouched until Movie::set_prop dispatches it.
                let value = match value {
                    DatumRef::Void => Datum::Void,
                    _ => self
                        .allocator
                        .try_get_datum(value)
                        .ok_or_else(|| {
                            ScriptError::new_code(
                                ScriptErrorCode::InvalidReference,
                                format!("invalid datum reference {value}"),
                            )
                        })?
                        .clone(),
                };
                self.movie.set_prop(symbols, prop, value, &self.allocator)
            }
        }
    }

    fn get_anim_prop(&mut self, prop_id: u16) -> Result<Datum, ScriptError> {
        let prop_name = get_anim_prop_name(prop_id);
        // `the timer` / `the keyPressed` / `the key` / `the keyCode` are the
        // classic-syntax time/input polls. Reading any in a `repeat while`
        // condition (`repeat while the timer < N`, nomiss's Simon delays; or a
        // key spin) is a busy-wait — flag it so the backward-jump handler yields
        // cooperatively to the JS event loop (see input_polled). Mirrors the
        // object-syntax reads in get_movie_prop.
        if matches!(
            prop_name,
            BuiltInSymbol::Timer
                | BuiltInSymbol::KeyPressed
                | BuiltInSymbol::Key
                | BuiltInSymbol::KeyCode
        ) {
            self.input_polled = true;
        }
        match prop_name {
            BuiltInSymbol::ColorDepth => Ok(Datum::Int(32)),
            BuiltInSymbol::FullColorPermit => Ok(Datum::Int(1)), // Full color mode is permitted
            BuiltInSymbol::Timer => Ok(Datum::Int(get_elapsed_ticks(self.start_time))),
            BuiltInSymbol::TimeoutLength => Ok(Datum::Int(self.movie.timeout_length)),
            BuiltInSymbol::TimeoutKeyDown => Ok(datum_bool(self.movie.timeout_keydown)),
            BuiltInSymbol::TimeoutMouse => Ok(datum_bool(self.movie.timeout_mouse)),
            BuiltInSymbol::TimeoutPlay => Ok(Datum::Int(0)),
            BuiltInSymbol::TimeoutLapsed => {
                let lapsed = ((crate::player::testing_shared::now_ms()
                    - self.movie.timeout_last_reset_ms)
                    * 60.0
                    / 1000.0)
                    .max(0.0) as i32;
                Ok(Datum::Int(lapsed))
            }
            BuiltInSymbol::SoundEnabled => Ok(Datum::Int(1)),
            BuiltInSymbol::SoundLevel => Ok(Datum::Int(7)), // max volume
            BuiltInSymbol::BeepOn | BuiltInSymbol::FixStageSize => Ok(Datum::Int(0)),
            BuiltInSymbol::CenterStage => Ok(datum_bool(self.center_stage)),
            BuiltInSymbol::ExitLock => Ok(datum_bool(
                self.env_overrides
                    .forced_exit_lock
                    .unwrap_or(self.movie.exit_lock),
            )),
            BuiltInSymbol::Key => Ok(Datum::String(self.keyboard_manager.key())),
            BuiltInSymbol::KeyPressed => Ok(Datum::String(self.keyboard_manager.key_pressed())),
            BuiltInSymbol::KeyCode => Ok(Datum::Int(self.keyboard_manager.key_code() as i32)),
            BuiltInSymbol::StageColor => Ok(Datum::Int(0)),
            BuiltInSymbol::DoubleClick => Ok(datum_bool(self.is_double_click)),
            BuiltInSymbol::LastClick
            | BuiltInSymbol::LastEvent
            | BuiltInSymbol::LastKey
            | BuiltInSymbol::LastRoll => Ok(Datum::Int(get_elapsed_ticks(self.start_time))),
            BuiltInSymbol::MultiSound => Ok(Datum::Int(1)),
            BuiltInSymbol::PauseState => Ok(datum_bool(self.is_script_paused)),
            BuiltInSymbol::SelStart => Ok(Datum::Int(self.text_selection_start as i32)),
            BuiltInSymbol::SelEnd => Ok(Datum::Int(self.text_selection_end as i32)),
            // `the safePlayer` is 0 (unsafe / full trust) unless
            // LeechProtectionRemovalHelp's `forceTheSafePlayer` pins it —
            // movies test it to decide whether local file access is allowed.
            BuiltInSymbol::SafePlayer => Ok(datum_bool(
                self.env_overrides.forced_safe_player.unwrap_or(false),
            )),
            BuiltInSymbol::SwitchColorDepth
            | BuiltInSymbol::ImageDirect
            | BuiltInSymbol::ColorQD
            | BuiltInSymbol::QuickTimePresent
            | BuiltInSymbol::VideoForWindowsPresent
            | BuiltInSymbol::NetPresent
            | BuiltInSymbol::SoundKeepDevice
            | BuiltInSymbol::SoundMixMedia
            | BuiltInSymbol::PreLoadRAM
            | BuiltInSymbol::ButtonStyle
            | BuiltInSymbol::CheckBoxAccess
            | BuiltInSymbol::CheckBoxType => Ok(Datum::Int(0)),
            _ => Err(ScriptError::new(format!("Unknown anim prop {}", prop_name))),
        }
    }

    fn get_anim2_prop(&self, prop_id: u16) -> Result<Datum, ScriptError> {
        let prop_name = get_anim2_prop_name(prop_id);
        match prop_name {
            BuiltInSymbol::NumberOfCastLibs => {
                Ok(Datum::Int(self.movie.cast_manager.casts.len() as i32))
            }
            BuiltInSymbol::NumberOfCastMembers => Ok(Datum::Int(
                self.movie
                    .cast_manager
                    .casts
                    .iter()
                    .map(|cast_lib| cast_lib.members.len() as i32)
                    .sum(),
            )),
            // `the number of xtras` — Director 11.5 Scripting Dictionary:
            // "returns the number of scripting Xtra extensions available to
            // the movie". Read-only. Movies pair it with `xtra(i).name` to
            // feature-detect an Xtra before `new(xtra "...")` (Hey Arnold!'s
            // enhancerXtra probes for "enhancer" this way). The count must
            // therefore agree with what `xtra(i)` indexes and with the
            // `xtraList` properties — all three read the same registry.
            BuiltInSymbol::NumberOfXtras => Ok(Datum::Int(
                xtra::manager::get_registered_xtra_names(self).len() as i32,
            )),
            _ => Err(ScriptError::new(format!(
                "Unknown anim2 prop {}",
                prop_name
            ))),
        }
    }

    pub(crate) fn set_movie_prop(
        &mut self,
        symbols: &SymbolTable,
        prop: Symbol,
        value: Datum,
    ) -> Result<(), ScriptError> {
        symbols
            .lower(&prop)
            .map_err(|_| crate::player::symbols::symbol::SymbolError::Foreign)?;
        match prop.into_builtin() {
            // LeechProtectionRemovalHelp `forceTheExitLock` — "force", not
            // "set": the write is swallowed so a leech check that re-asserts
            // `the exitLock` before testing it can't undo the fake. Director
            // never errors on this assignment, so neither do we.
            Some(BuiltInSymbol::ExitLock) if self.env_overrides.forced_exit_lock.is_some() => {
                Ok(())
            }
            // Director 11.5 Scripting Dictionary lists `timeoutList` as a
            // READ-ONLY Movie property: "a linear list containing all currently
            // active timeout objects… Use the forget() method to delete a timeout
            // object". Director does not RAISE on assignment though, and movies use
            // `_movie.timeoutList = []` as a clear-all idiom — AreaZero's
            // `[M] Misc Handlers.ClearTimeOutList` is exactly that, invoked from the
            // reset event beside DeleteAllButtons / DeleteAllScripts / stop-all-sound.
            // Erroring there aborted the reset and the game never started.
            //
            // Accept the write and give it the meaning the documented property
            // implies: make the live set match the assigned list, forgetting every
            // timeout it does not name (so `[]` cancels all). forget_timeout also
            // cancels the underlying JS interval, which plain clearing would leak.
            Some(BuiltInSymbol::TimeoutList) => {
                let keep: Vec<String> = match &value {
                    Datum::List(_, items, _) => {
                        let mut keep = Vec::new();
                        for r in items {
                            match self.checked_movie_datum_ref(symbols, r)? {
                                Datum::TimeoutRef(n) => keep.push(n.clone()),
                                _ => {}
                            }
                        }
                        keep
                    }
                    _ => Vec::new(),
                };
                let existing: Vec<String> = self.timeout_manager.timeouts.keys().cloned().collect();
                for name in existing {
                    if !keep.iter().any(|k| k.eq_ignore_ascii_case(&name)) {
                        self.timeout_manager.forget_timeout(&name);
                    }
                }
                Ok(())
            }
            Some(BuiltInSymbol::KeyboardFocusSprite) => {
                // TODO switch focus
                self.keyboard_focus_sprite = value.int_value()? as i16;
                Ok(())
            }
            Some(BuiltInSymbol::SelStart) => {
                self.text_selection_start = value.int_value()? as u16;
                Ok(())
            }
            Some(BuiltInSymbol::SelEnd) => {
                self.text_selection_end = value.int_value()? as u16;
                Ok(())
            }
            Some(BuiltInSymbol::ClipBoard) => {
                self.clipboard_mirror = value.string_value(symbols)?;
                Ok(())
            }
            Some(BuiltInSymbol::FloatPrecision) => {
                self.float_precision = value.int_value()? as u8;
                Ok(())
            }
            Some(BuiltInSymbol::Timer) => {
                // `set the timer = N` resets Director's timer to N ticks (1/60 s);
                // `set the timer = 0` is equivalent to `startTimer`. `the timer`
                // reads elapsed ticks since `start_time`, so back-date start_time
                // by N ticks so it reads N going forward. eds_kart_attack's Kart
                // behavior does `set the timer = 0` in `on wobble`.
                let ticks = value.int_value()?;
                let ms = (ticks as i64) * 1000 / 60;
                self.start_time = chrono::Local::now() - chrono::Duration::milliseconds(ms);
                Ok(())
            }
            Some(BuiltInSymbol::CenterStage) => {
                self.center_stage = value.int_value()? != 0;
                crate::player::stage::apply_stage_draw_rect(self);
                let (w, h) = crate::player::stage::stage_canvas_dims(self);
                crate::js_api::JsApi::dispatch_stage_size_changed(w, h, self.center_stage);
                Ok(())
            }
            Some(BuiltInSymbol::ActorList) => {
                // Setting actorList - update the global variable
                match value {
                    Datum::List(list_type, list_items, sorted) => {
                        for item in &list_items {
                            self.checked_movie_datum_ref(symbols, item)?;
                        }
                        let new_actor_list =
                            self.alloc_datum(Datum::List(list_type, list_items, sorted));
                        self.globals
                            .insert(Symbol::builtin(BuiltInSymbol::ActorList), new_actor_list);
                        self.actor_list_generation = self.actor_list_generation.wrapping_add(1);
                        Ok(())
                    }
                    _ => Err(ScriptError::new("actorList must be a list".to_string())),
                }
            }
            _ => self.movie.set_prop(symbols, prop, value, &self.allocator),
        }
    }

    /// Human-readable dump of the current handler call stack (`script::handler`
    /// per scope, outer→inner). Used to surface a nested `#movie` sub-player's
    /// stack in the console — the Dev UI panels are bound to the main player, so
    /// a sub-player's error is otherwise uninspectable there.
    pub fn call_stack_string(&self) -> String {
        let mut trace = String::new();
        for i in 0..self.scope_count {
            if let Some(scope) = self.scopes.get(i as usize) {
                let handler_info = if let Some(script) =
                    self.movie.cast_manager.get_script_by_ref(&scope.script_ref)
                {
                    let handler_name = script
                        .handlers
                        .iter()
                        .find(|(_, h)| h.name_id == scope.handler_name_id)
                        .map(|(name, _)| format!("{:?}", name))
                        .unwrap_or_else(|| format!("#{}", scope.handler_name_id));
                    format!("{}::{}", script.name, handler_name)
                } else {
                    format!("?::#{}", scope.handler_name_id)
                };
                trace.push_str(&format!(
                    "  {}: {} (pc={})\n",
                    i, handler_info, scope.bytecode_index
                ));
            }
        }
        trace
    }

    fn on_script_error(&mut self, err: &ScriptError) {
        self.on_script_error_with_symbols(err, None);
    }

    pub(crate) fn on_script_error_with_symbols(
        &mut self,
        err: &ScriptError,
        symbols: Option<&SymbolTable>,
    ) {
        // abort is flow control (exits handler chain), not a real error
        if err.code == ScriptErrorCode::Abort {
            return;
        }
        // Native owned-playback tests need to distinguish the one production
        // error-reporting call made by the finalizer from the callback error
        // that it deliberately carries through StopMovie cleanup. Keep this
        // probe test-only; the browser callback remains the source of truth.
        #[cfg(test)]
        TEST_SCRIPT_ERROR_COUNT.with(|count| count.set(count.get().saturating_add(1)));
        // `console_error!`, not `web_sys::console::error_1`: the raw call takes a
        // `JsValue`, and building one on a native target panics inside
        // wasm-bindgen ("cannot access imported statics on non-wasm targets").
        // Since this runs on EVERY script error, it took down every native e2e
        // run the moment a movie hit one. The macro is wasm/native-aware.
        crate::console_error!("[!!] play failed with error: {}", err.message);
        // The message alone rarely says WHICH handler blew up. Scopes are still
        // intact at this point, so dump them.
        crate::console_error!("{}", self.format_scope_stack(5, 25));
        warn!("[!!] play failed with error: {}", err.message);
        self.stop();

        // Dispatch debug update with full call stack (scopes are preserved on
        // error) only when the owning session supplied its symbol table.
        if let Some(symbols) = symbols {
            JsApi::dispatch_debug_update(symbols, self);
        }
        JsApi::dispatch_script_error(self, &err);
    }

    fn get_ctx_current_bytecode<'a>(&'a self, ctx: &'a BytecodeHandlerContext) -> &'a Bytecode {
        let scope = self.scopes.get(ctx.scope_ref()).unwrap();
        let bytecode_index = scope.bytecode_index;
        let handler_def = ctx.code.handler.as_ref();
        handler_def.bytecode_array.get(bytecode_index).unwrap()
    }

    /// Render the live scope stack as `script::handler (bytecode_index=N)` lines.
    /// Shows the BOTTOM `head` frames as well as the top `tail`: the tail alone
    /// can't tell a genuinely infinite recursion from a legitimately deep one,
    /// because you can't see how much is outer context and how much repeats.
    pub fn format_scope_stack(&self, head: u32, tail: u32) -> String {
        let mut out = String::from("Scope stack (outermost frames, then the deepest):\n");
        let indices: Vec<u32> = if self.scope_count <= head + tail {
            (0..self.scope_count).collect()
        } else {
            (0..head)
                .chain((self.scope_count - tail)..self.scope_count)
                .collect()
        };
        let mut last: Option<u32> = None;
        for i in indices {
            if let Some(prev) = last {
                if i != prev + 1 {
                    out.push_str(&format!("  … {} more frames …\n", i - prev - 1));
                }
            }
            last = Some(i);
            if let Some(scope) = self.scopes.get(i as ScopeRef) {
                // Try to get the handler name from the script
                let handler_info = if let Some(script) =
                    self.movie.cast_manager.get_script_by_ref(&scope.script_ref)
                {
                    // Find handler name by looking through the handlers map
                    let handler_name = script
                        .handlers
                        .iter()
                        .find(|(_, h)| h.name_id == scope.handler_name_id)
                        .map(|(name, _)| format!("{:?}", name))
                        .unwrap_or_else(|| format!("handler_name_id#{}", scope.handler_name_id));
                    format!("{}::{}", script.name, handler_name)
                } else {
                    format!("unknown_script::handler_name_id#{}", scope.handler_name_id)
                };
                out.push_str(&format!(
                    "  Scope {}: {} (bytecode_index={})\n",
                    i, handler_info, scope.bytecode_index
                ));
            }
        }
        out
    }

    pub fn push_scope(&mut self) -> ScopeRef {
        if (self.scope_count + 1) as usize >= MAX_STACK_SIZE {
            // Try to get some context about what's on the stack
            let mut stack_trace = String::from(
                "Stack overflow detected - this is likely due to infinite recursion in the movie's Lingo scripts.\n",
            );
            stack_trace.push_str(&self.format_scope_stack(5, 25));
            stack_trace.push_str("\nThis usually indicates a bug in the Director movie's scripts (e.g., a handler calling itself infinitely).\n");
            stack_trace.push_str("Note: If this is happening during frame events, it may be a re-entrant call issue.\n");
            web_sys::console::error_1(&stack_trace.into());
            panic!("Stack overflow - infinite recursion in Lingo scripts");
        }
        let scope_ref = self.scope_count;
        let scope = self.scopes.get_mut(scope_ref as ScopeRef).unwrap();
        scope.reset();
        self.scope_count += 1;
        scope_ref as ScopeRef
    }

    pub fn pop_scope(&mut self) {
        // Guard against underflow. A stale handler resuming from an async JS
        // callback after a test reset (Ruffle init, MP3 decode, fetch) can
        // decrement scope_count on a freshly-reset player. A wrapping subtract
        // would drive `scope_count` to u32::MAX-N and poison every later
        // `scope_count - 1` index computation.
        if self.scope_count > 0 {
            self.scope_count -= 1;
        } else {
            warn!("pop_scope called with scope_count=0 (stale handler?)");
        }
    }

    pub fn current_scope_ref(&self) -> ScopeRef {
        self.scope_count.saturating_sub(1) as ScopeRef
    }

    // Lingo: sound(channelNum)
    pub fn get_sound_channel(&mut self, channel_num: i32) -> Result<DatumRef, ScriptError> {
        // Store the 1-based channel number (conversion to 0-based happens in get_channel_index)
        Ok(self.alloc_datum(Datum::SoundChannel(channel_num as u16)))
    }

    // Lingo: puppetSound channelNum, memberRef
    pub fn puppet_sound(
        &mut self,
        channel_num: i32,
        member_ref: DatumRef,
    ) -> Result<(), ScriptError> {
        let sound_channel = self.get_sound_channel(channel_num)?;

        // Get the loop setting from the cast member
        let loop_count = {
            let member_datum = self.get_datum(&member_ref);
            if let Datum::CastMember(cast_member_ref) = member_datum {
                if let Some(cast_member) =
                    self.movie.cast_manager.find_member_by_ref(cast_member_ref)
                {
                    if let CastMemberType::Sound(sound_member) = &cast_member.member_type {
                        if sound_member.info.loop_enabled {
                            0 // Loop forever
                        } else {
                            1 // Play once
                        }
                    } else {
                        1
                    }
                } else {
                    1
                }
            } else {
                1
            }
        };

        // handle_play_file applies this loop count to the channel right before
        // it starts the sound. (It used to be set here AND reset to 1 inside
        // handle_play_file, which clobbered looping — now the derived value is
        // threaded through.)
        SoundChannelDatumHandlers::handle_play_file(self, &sound_channel, &member_ref, loop_count)
    }

    // Lingo: sound stop channelNum
    pub fn sound_stop(&mut self, channel_num: i32) -> Result<(), ScriptError> {
        let sound_channel = self.get_sound_channel(channel_num)?;
        SoundChannelDatumHandlers::handle_stop(self, &sound_channel)
    }

    pub fn load_sound_member(&self, member_ref: &DatumRef) -> Result<AudioData, ScriptError> {
        // TODO: Get the cast member from your cast storage
        // let cast_member = self.get_cast_member(member_ref)?;

        // Get the raw sound data
        // let sound_data = cast_member.get_sound_data()?;

        // Load and decode it
        // load_director_sound(sound_data)
        //     .map_err(|e| ScriptError::new(format!("Failed to load sound: {}", e)))

        Err(ScriptError::new("Not implemented".to_string()))
    }

    pub fn has_custom_font(&self, font_name: &str) -> bool {
        if font_name.is_empty() || font_name == "System" {
            return false;
        }
        self.font_manager.get_font_immutable(font_name).is_some()
    }

    pub fn list_available_fonts(&self) -> Vec<String> {
        let mut fonts: Vec<String> = self
            .font_manager
            .font_cache
            .keys()
            .map(|k| k.clone())
            .collect();
        fonts.sort();
        fonts
    }

    pub fn is_yield_safe(&self) -> bool {
        !self.is_in_frame_update
            && !self.in_frame_script
            && !self.in_enter_frame
            && !self.in_prepare_frame
            && !self.in_event_dispatch
    }

    /// Process filmloop frame changes and sprite updates
    /// Returns list of (member_ref, old_frame, new_frame) for filmloops that changed
    pub async fn update_filmloop_frames(&mut self) -> Vec<(CastMemberRef, u32, u32)> {
        let mut changed_filmloops = Vec::new();

        // Collect active filmloop refs with their current and next frames
        let active_filmloops: Vec<(CastMemberRef, u32, u32)> = self
            .active_stage_filmloop_member_refs()
            .into_iter()
            .filter_map(|member_ref| {
                self.movie
                    .cast_manager
                    .find_member_by_ref(&member_ref)
                    .and_then(|m| {
                        if let CastMemberType::FilmLoop(film_loop) = &m.member_type {
                            let current = film_loop.current_frame;

                            let frame_count = film_loop.score.frame_count.unwrap_or(1).max(1);

                            let next = current + 1;
                            // `info.loops` is already the DECODED flag (1 = loop,
                            // 0 = play once and hold on the last frame) — see
                            // FilmLoopInfo::from, which does `1 - (flags >> 5 & 1)`.
                            // Re-testing it as the raw 0x20 bit is always false, so
                            // every film loop looped regardless of its member flag
                            // and one-shot intro/outro animations never finished.
                            let should_loop = film_loop.info.loops != 0;

                            let new_frame = if next > frame_count {
                                if should_loop { 1 } else { frame_count }
                            } else {
                                next
                            };

                            Some((member_ref, current, new_frame))
                        } else {
                            None
                        }
                    })
            })
            .collect();

        // Process each filmloop
        for (member_ref, old_frame, new_frame) in active_filmloops {
            // Skip if frame didn't change
            if old_frame == new_frame {
                continue;
            }

            // End sprites that are leaving
            let score_ref = ScoreRef::FilmLoop(member_ref.clone());
            let ended_sprites =
                if let Some(member) = self.movie.cast_manager.find_mut_member_by_ref(&member_ref) {
                    if let CastMemberType::FilmLoop(film_loop) = &mut member.member_type {
                        film_loop
                            .score
                            .end_sprites(score_ref.clone(), old_frame, new_frame)
                            .await
                    } else {
                        vec![]
                    }
                } else {
                    vec![]
                };

            let session_handle = retained_session_handle()
                .expect("filmloop advancement requires the owning runtime session");
            let player_id = active_player_id() as u32;
            session_handle
                .borrow_mut()
                .with_player(player_id, |mut context| {
                    let is_film_loop = if let Some(member) = context
                        .player
                        .movie
                        .cast_manager
                        .find_mut_member_by_ref(&member_ref)
                    {
                        if let CastMemberType::FilmLoop(film_loop) = &mut member.member_type {
                            for sprite_num in &ended_sprites {
                                film_loop.score.get_sprite_mut(*sprite_num as i16).exited = true;
                            }
                            film_loop.current_frame = new_frame;
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    if is_film_loop {
                        context.player.begin_score_sprites(
                            score_ref.clone(),
                            new_frame,
                            context.symbols,
                        );
                        if let Some(member) = context
                            .player
                            .movie
                            .cast_manager
                            .find_mut_member_by_ref(&member_ref)
                        {
                            if let CastMemberType::FilmLoop(film_loop) = &mut member.member_type {
                                film_loop.score.apply_tween_modifiers(new_frame);
                            }
                        }
                    }
                })
                .expect("filmloop advancement player disappeared");

            changed_filmloops.push((member_ref, old_frame, new_frame));
        }

        if !changed_filmloops.is_empty() {
            self.invalidate_behavior_channel_cache();
        }

        changed_filmloops
    }

    /// Just advance frame counters - actual sprite management happens in update_filmloop_frames
    fn advance_filmloop_frames(&mut self) {
        let active_filmloop_refs = self.active_stage_filmloop_member_refs();

        for member_ref in active_filmloop_refs {
            if let Some(member) = self.movie.cast_manager.find_mut_member_by_ref(&member_ref) {
                if let CastMemberType::FilmLoop(film_loop) = &mut member.member_type {
                    let frame_count = film_loop.score.frame_count.unwrap_or(1).max(1);

                    let old_frame = film_loop.current_frame;
                    film_loop.current_frame += 1;

                    if film_loop.current_frame > frame_count {
                        // Decoded flag, not a bitmask — see update_filmloop_frames.
                        let should_loop = film_loop.info.loops != 0;

                        if should_loop {
                            film_loop.current_frame = 1;
                        } else {
                            film_loop.current_frame = frame_count;
                        }
                    }
                }
            }
        }
    }
}

/// Module-level owner-bound loader used by frontend and native harness
/// callers. The session handle, player id, and owner are captured by the
/// caller and remain authoritative across the async boundary.
pub async fn load_movie_from_dir_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
    dir: DirectorFile,
) -> Result<(), ScriptError> {
    DirPlayer::load_movie_from_dir_owned(session, player_id, owner, dir).await
}

/// Fetch and mount a movie through an explicit session owner.  Network
/// scheduling and completion are deliberately outside the `RuntimeSession`
/// borrow; only task creation, result inspection, and the final movie
/// mutation reacquire short owner-checked borrows.
pub(crate) async fn load_movie_from_file_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
    path: String,
) -> Result<(), ScriptError> {
    let (task_id, task_future) = session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            let mut player = context.player;
            let task_id = player.net_manager.preload_net_thing(path.clone());
            let future = player.net_manager.create_task_future(task_id);
            Ok((task_id, future))
        })
        .ok_or_else(cancelled_scope_error)??;
    task_future.await;

    let (data_bytes, file_name, base_url) = session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            let task = context
                .player
                .net_manager
                .get_task(task_id)
                .ok_or_else(|| ScriptError::new(format!("Network task {} disappeared", task_id)))?;
            let bytes = context
                .player
                .net_manager
                .get_task_result(Some(task_id))
                .ok_or_else(|| ScriptError::new(format!("No response received for '{}'", path)))?
                .map_err(|_| ScriptError::new(format!("Network request failed for '{}'", path)))?;
            let file_name = task
                .resolved_url
                .path_segments()
                .and_then(|segments| segments.last())
                .unwrap_or("untitled.dcr")
                .to_owned();
            let base_url = get_base_url(&task.resolved_url).to_string();
            Ok((bytes, file_name, base_url))
        })
        .ok_or_else(cancelled_scope_error)??;

    let movie_file =
        read_director_file_bytes(&data_bytes, &file_name, &base_url).map_err(|error| {
            ScriptError::new(format!("Failed to parse movie file '{}': {}", path, error))
        })?;
    session
        .borrow_mut()
        .with_player(player_id, |context| {
            if owner.same_identity(&context.player.owner) && owner.is_arena_live() {
                context.player.movie_reload_data = Some((data_bytes, file_name, base_url));
                Ok(())
            } else {
                Err(cancelled_scope_error())
            }
        })
        .ok_or_else(cancelled_scope_error)??;
    load_movie_from_dir_owned(session, player_id, owner, movie_file).await
}

/// Browser-facing owner-bound URL loader. The URL is fetched and parsed by
/// the same session-owned path as native file loading; no ambient player or
/// symbol table is selected after the await.
pub async fn load_movie_from_url_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
    url: String,
) -> Result<(), ScriptError> {
    load_movie_from_file_owned(session, player_id, owner, url).await
}

/// Start an owner-bound command evaluation and return its first turn.  The
/// evaluator owns its frame stack and any pending capability in the session;
/// callers must retain the returned id and route a later completion through
/// [`resume_eval_owned`].  Returning the turn keeps suspension as control
/// flow, so a host request is never collapsed into a synthetic error or
/// `Void` value.
pub(crate) fn start_eval_lingo_command_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
    source: String,
) -> Result<(crate::player::eval::EvalId, crate::player::eval::EvalTurn), ScriptError> {
    let mut runtime = session.borrow_mut();
    let valid = runtime
        .with_player(player_id, |context| {
            owner.same_identity(&context.player.owner) && owner.is_arena_live()
        })
        .unwrap_or(false);
    if !valid {
        return Err(cancelled_scope_error());
    }
    let eval_id = crate::player::eval::start_eval_lingo_command(&mut runtime, player_id, source)?;
    let turn = runtime.turn_eval(eval_id.clone());
    Ok((eval_id, turn))
}

/// Drive one owner-bound evaluator turn to completion. Nested value and
/// command evaluators share this pump so a pending child gets a fresh
/// evaluator identity while its parent remains retained for resumption.
pub(crate) async fn drive_eval_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
    eval_id: crate::player::eval::EvalId,
    mut turn: crate::player::eval::EvalTurn,
) -> Result<DatumRef, ScriptError> {
    let mut cancellation = if matches!(&turn, crate::player::eval::EvalTurn::Complete(_)) {
        None
    } else {
        Some(crate::player::eval::EvalCancellationGuard::new(
            session.clone(),
            player_id,
            owner.clone(),
            eval_id.clone(),
        )?)
    };
    loop {
        match turn {
            crate::player::eval::EvalTurn::Complete(result) => {
                if let Some(cancellation) = cancellation.as_mut() {
                    cancellation.disarm();
                }
                return result;
            }
            crate::player::eval::EvalTurn::Pending { request } => {
                let (sender, receiver) = async_std::channel::bounded(1);
                let action = match &request {
                    crate::player::eval::EvalPending::Global { capability, .. }
                    | crate::player::eval::EvalPending::Object { capability, .. }
                    | crate::player::eval::EvalPending::SetProperty { capability, .. } => {
                        capability.clone()
                    }
                };
                let next = session
                    .borrow_mut()
                    .execute_eval_request(eval_id.clone(), request);
                match next {
                    crate::player::session::EvalRequestTurn::ExternalXtra(request) => {
                        turn = match crate::player::commands::execute_eval_external_request(
                            &session,
                            eval_id.clone(),
                            action,
                            player_id,
                            owner.clone(),
                            request,
                        )
                        .await
                        {
                            crate::player::session::EvalRequestTurn::Evaluator(turn) => turn,
                            _ => {
                                return Err(crate::player::cancelled_scope_error());
                            }
                        };
                    }
                    crate::player::session::EvalRequestTurn::ExternalXtraLoad(request) => {
                        turn = match crate::player::commands::execute_eval_external_load_request(
                            &session,
                            eval_id.clone(),
                            action,
                            player_id,
                            owner.clone(),
                            request,
                        )
                        .await
                        {
                            crate::player::session::EvalRequestTurn::Evaluator(turn) => turn,
                            _ => {
                                return Err(crate::player::cancelled_scope_error());
                            }
                        };
                    }
                    crate::player::session::EvalRequestTurn::XtraPending(request) => {
                        let result = crate::player::commands::execute_xtra_pending_request(
                            &session, player_id, request,
                        )
                        .await;
                        turn = session.borrow_mut().resume_eval(
                            eval_id.clone(),
                            &action,
                            &owner,
                            result,
                        );
                    }
                    crate::player::session::EvalRequestTurn::SpriteAsync(request) => {
                        if request.player_id != player_id || !request.owner.same_identity(&owner) {
                            turn = crate::player::eval::EvalTurn::Complete(Err(
                                crate::player::cancelled_scope_error(),
                            ));
                            continue;
                        }
                        let typed_request =
                            crate::player::driver::InternalVmRequest::SpriteAsync(request.clone());
                        let result =
                            crate::player::session::RuntimeSession::execute_sprite_async_request(
                                session.clone(),
                                request,
                            )
                            .await;
                        turn = match session.borrow_mut().resume_typed_async_eval(
                            eval_id.clone(),
                            &action,
                            &typed_request,
                            &owner,
                            result,
                        ) {
                            crate::player::session::EvalRequestTurn::Evaluator(turn) => turn,
                            _ => crate::player::eval::EvalTurn::Complete(Err(
                                crate::player::cancelled_scope_error(),
                            )),
                        };
                    }
                    crate::player::session::EvalRequestTurn::MovieAsync(request) => {
                        let result = Box::pin(crate::player::handlers::movie::execute_movie_async(
                            session.clone(),
                            request,
                        ))
                        .await;
                        turn = session.borrow_mut().resume_eval(
                            eval_id.clone(),
                            &action,
                            &owner,
                            result,
                        );
                    }
                    crate::player::session::EvalRequestTurn::Flash(request) => {
                        let result = crate::player::commands::execute_owned_flash_request(
                            &session, player_id, &owner, request,
                        )
                        .await;
                        turn = session.borrow_mut().resume_eval(
                            eval_id.clone(),
                            &action,
                            &owner,
                            result,
                        );
                    }
                    crate::player::session::EvalRequestTurn::Evaluator(next) => match next {
                        crate::player::eval::EvalTurn::Complete(result) => {
                            if let Some(cancellation) = cancellation.as_mut() {
                                cancellation.disarm();
                            }
                            return result;
                        }
                        crate::player::eval::EvalTurn::Pending { request } => {
                            if let Some((action, internal_request)) =
                                crate::player::commands::eval_pending_internal_request(&request)
                            {
                                if let Some(crate::player::session::EvalRequestTurn::Evaluator(
                                    next,
                                )) = Box::pin(
                                    crate::player::commands::execute_eval_internal_request(
                                        &session,
                                        eval_id.clone(),
                                        action,
                                        owner.clone(),
                                        internal_request,
                                    ),
                                )
                                .await
                                {
                                    turn = next;
                                    continue;
                                }
                            }
                            session.borrow_mut().retain_pending_eval_request(
                                eval_id.clone(),
                                player_id,
                                owner.clone(),
                                request,
                                sender,
                            );
                            return receiver.recv().await.map_err(|_| {
                                ScriptError::new("evaluator completion channel closed".to_owned())
                            })?;
                        }
                    },
                    crate::player::session::EvalRequestTurn::Child(child_turn) => {
                        match child_turn {
                            crate::player::driver::DriverTurn::Waiting => {
                                session.borrow_mut().retain_pending_command(
                                    crate::player::driver::PendingCommand {
                                        player_id,
                                        owner: owner.clone(),
                                        started: false,
                                        action: None,
                                        ticket: None,
                                        completer: None,
                                        event_sender: None,
                                        score_continuation: None,
                                        child_completion: None,
                                        eval_child: Some(eval_id.clone()),
                                        eval_sender: Some(sender),
                                    },
                                );
                                return receiver.recv().await.map_err(|_| {
                                    ScriptError::new("evaluator child channel closed".to_owned())
                                })?;
                            }
                            crate::player::driver::DriverTurn::Pending(action) => {
                                let ticket = action.ticket().clone();
                                session.borrow_mut().retain_pending_command(
                                    crate::player::driver::PendingCommand {
                                        player_id,
                                        owner: owner.clone(),
                                        started: false,
                                        action: Some(action),
                                        ticket: Some(ticket),
                                        completer: None,
                                        event_sender: None,
                                        score_continuation: None,
                                        child_completion: None,
                                        eval_child: Some(eval_id.clone()),
                                        eval_sender: Some(sender),
                                    },
                                );
                                return receiver.recv().await.map_err(|_| {
                                    ScriptError::new("evaluator child channel closed".to_owned())
                                })?;
                            }
                            crate::player::driver::DriverTurn::Complete(_)
                            | crate::player::driver::DriverTurn::Error(_) => {
                                turn = session
                                .borrow_mut()
                                .turn_eval_child(eval_id.clone())
                                .and_then(|child| match child {
                                    crate::player::session::EvalRequestTurn::Evaluator(turn) => Some(turn),
                                    crate::player::session::EvalRequestTurn::Child(_)
                                    | crate::player::session::EvalRequestTurn::SpriteAsync(_)
                                    | crate::player::session::EvalRequestTurn::MovieAsync(_)
                                    | crate::player::session::EvalRequestTurn::Flash(_)
                                    | crate::player::session::EvalRequestTurn::ExternalXtra(_)
                                    | crate::player::session::EvalRequestTurn::ExternalXtraLoad(_)
                                    | crate::player::session::EvalRequestTurn::XtraPending(_) => None,
                                })
                                .unwrap_or_else(|| crate::player::eval::EvalTurn::Complete(Err(cancelled_scope_error())));
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Evaluate a command for a captured session owner. Synchronous turns and
/// prepared evaluator children use the shared owner-bound evaluation pump.
pub(crate) async fn eval_lingo_command_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
    source: String,
) -> Result<DatumRef, ScriptError> {
    let (eval_id, turn) =
        start_eval_lingo_command_owned(session.clone(), player_id, owner.clone(), source)?;
    drive_eval_owned(session, player_id, owner, eval_id, turn).await
}

pub(crate) fn resume_eval_owned(
    session: RuntimeSessionHandle,
    eval_id: crate::player::eval::EvalId,
    action: &crate::player::eval::EvalAction,
    owner: &OwnerToken,
    result: Result<DatumRef, ScriptError>,
) -> crate::player::eval::EvalTurn {
    session
        .borrow_mut()
        .resume_eval(eval_id, action, owner, result)
}

pub fn player_alloc_datum(datum: Datum) -> DatumRef {
    // let mut player_opt = PLAYER_LOCK.try_write().unwrap();
    unsafe {
        let player = crate::player::player_mut();
        player.alloc_datum(datum)
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum ScriptErrorCode {
    HandlerNotFound,
    Generic,
    InvalidReference,
    Abort,
}

#[derive(Debug, Clone)]
pub struct ScriptError {
    pub code: ScriptErrorCode,
    pub message: String,
}

/// A handler turn that is safe to return across the session borrow boundary.
/// `Pending` retains the driver's opaque action, so a host can resume the
/// exact invocation after doing external work. It is deliberately separate
/// from `Result`: suspension is control flow, not a script failure.
pub(crate) enum ScriptHandlerTurn {
    Complete(Result<ScopeResult, ScriptError>),
    Waiting,
    Pending(driver::PendingAction),
}

/// The corresponding turn for global handlers, whose synchronous value is a
/// datum rather than a scope result.
pub(crate) enum GlobalHandlerTurn {
    Complete(Result<DatumRef, ScriptError>),
    Waiting,
    PendingRequest {
        request: driver::InternalVmRequest,
        reason: String,
    },
    PendingAction(driver::PendingAction),
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl From<ScriptError> for String {
    fn from(e: ScriptError) -> String {
        e.message
    }
}

impl ScriptError {
    pub fn new(message: String) -> ScriptError {
        Self::new_code(ScriptErrorCode::Generic, message)
    }

    pub fn new_code(code: ScriptErrorCode, message: String) -> ScriptError {
        ScriptError { code, message }
    }
}

impl From<SymbolError> for ScriptError {
    fn from(error: SymbolError) -> Self {
        match error {
            SymbolError::Foreign => {
                Self::new_code(ScriptErrorCode::InvalidReference, error.to_string())
            }
            SymbolError::NotBuiltin { .. } => Self::new(error.to_string()),
        }
    }
}

pub fn player_handle_scope_return(scope: &ScopeResult) {
    if scope.passed {
        reserve_player_mut(|player| {
            let scope_ref = player.current_scope_ref();
            let last_scope = player.scopes.get_mut(scope_ref);
            if let Some(last_scope) = last_scope {
                last_scope.passed = true;
            }
        });
    }
}

/// Dispatch a global through the retained production RuntimeSession.
///
/// The session owns the player graph and the handler driver. A synchronous
/// result is returned directly; a host request remains owned by the driver and
/// is reported with its retained reason until the caller adopts the request
/// executor. No ambient player lookup occurs in this path.
pub(crate) async fn player_call_global_handler_turn(
    handler_name: Symbol,
    args: &Vec<DatumRef>,
) -> GlobalHandlerTurn {
    let id = active_player_id() as u32;
    let Some(handle) = PLAYER_SESSION_HANDLE.with(|slot| slot.borrow().clone()) else {
        return GlobalHandlerTurn::Complete(Err(ScriptError::new(
            "runtime session is not initialized".to_owned(),
        )));
    };
    let mut session = handle.borrow_mut();
    player_call_global_handler_turn_in_session(&mut session, id, handler_name, args)
}

/// Explicit global turn primitive. The caller owns the session handle and
/// captures `id` before entering an async boundary; this function never
/// consults ambient player state.
pub(crate) fn player_call_global_handler_turn_in_session(
    session: &mut RuntimeSession,
    id: u32,
    handler_name: Symbol,
    args: &Vec<DatumRef>,
) -> GlobalHandlerTurn {
    let dispatch = match session.dispatch_global(id, &handler_name, args) {
        Ok(dispatch) => dispatch,
        Err(error) => return GlobalHandlerTurn::Complete(Err(error)),
    };
    let (receiver, handler_ref, child_args, completion, use_raw_arg_list) = match dispatch {
        driver::GlobalDispatch::Child {
            receiver,
            handler_ref,
        } => (receiver, handler_ref, args.clone(), None, true),
        driver::GlobalDispatch::ChildPrepared {
            receiver,
            handler_ref,
            args,
        } => (receiver, handler_ref, args, None, false),
        driver::GlobalDispatch::ChildWithCompletion {
            receiver,
            handler_ref,
            args,
            completion,
        } => (receiver, handler_ref, args, Some(completion), false),
        driver::GlobalDispatch::AncestorChildren { .. } => {
            return GlobalHandlerTurn::PendingRequest {
                request: driver::InternalVmRequest::Global {
                    name: handler_name,
                    args: args.clone(),
                },
                reason: "ancestor handler dispatch requires the global continuation executor"
                    .to_owned(),
            };
        }
        driver::GlobalDispatch::SyncResult(result) => return GlobalHandlerTurn::Complete(result),
        driver::GlobalDispatch::ChildSequence { .. } => {
            return GlobalHandlerTurn::PendingRequest {
                request: driver::InternalVmRequest::Global {
                    name: handler_name,
                    args: args.clone(),
                },
                reason: "ordered global child sequence requires the global continuation executor"
                    .to_owned(),
            };
        }
        driver::GlobalDispatch::PendingRequest { request, reason } => {
            return GlobalHandlerTurn::PendingRequest { request, reason };
        }
        driver::GlobalDispatch::Pending { reason } => {
            return GlobalHandlerTurn::PendingRequest {
                request: driver::InternalVmRequest::Global {
                    name: handler_name,
                    args: args.clone(),
                },
                reason,
            };
        }
    };
    let started =
        match session.start_handler(id, receiver, handler_ref, &child_args, use_raw_arg_list) {
            Ok(started) => started,
            Err(error) => return GlobalHandlerTurn::Complete(Err(error)),
        };
    match started {
        Some(result) => {
            let result = if let Some(completion) = completion {
                session.apply_child_completion(id, completion, result.return_value)
            } else {
                Ok(result.return_value)
            };
            GlobalHandlerTurn::Complete(result)
        }
        None => loop {
            match session.turn_handler(id) {
                Some(DriverTurn::Complete(result)) => {
                    let result = if let Some(completion) = completion.clone() {
                        session.apply_child_completion(id, completion, result.return_value)
                    } else {
                        Ok(result.return_value)
                    };
                    break GlobalHandlerTurn::Complete(result);
                }
                Some(DriverTurn::Error(error)) => break GlobalHandlerTurn::Complete(Err(error)),
                // Waiting is a cooperative driver increment. Keep advancing
                // this owner-bound continuation until an external action or
                // terminal result is reached; exposing Waiting here would
                // leave a command with no ticket to resume it.
                Some(DriverTurn::Waiting) => continue,
                Some(DriverTurn::Pending(request)) => {
                    break GlobalHandlerTurn::PendingAction(request);
                }
                None => {
                    break GlobalHandlerTurn::Complete(Err(ScriptError::new(
                        "global handler driver disappeared".to_owned(),
                    )));
                }
            }
        },
    }
}

/// Owner-bound global handler turn. Suspension is returned to the caller as
/// control flow; callers must retain the returned action and resume it through
/// `RuntimeSession::complete_handler_action` rather than converting it to a
/// script error.
pub async fn player_call_global_handler(
    handler_name: Symbol,
    args: &Vec<DatumRef>,
) -> GlobalHandlerTurn {
    player_call_global_handler_turn(handler_name, args).await
}

/// Dispatch an object handler through the active retained session. Callers
/// must handle `Pending` as an owned request before awaiting host work.
pub(crate) fn player_call_datum_handler_active(
    receiver: &DatumRef,
    name: Symbol,
    args: &Vec<DatumRef>,
) -> crate::player::handlers::datum_handlers::DatumDispatch {
    let id = active_player_id() as u32;
    let Some(handle) = PLAYER_SESSION_HANDLE.with(|slot| slot.borrow().clone()) else {
        return crate::player::handlers::datum_handlers::DatumDispatch::Sync(Err(
            ScriptError::new("runtime session is not initialized".to_owned()),
        ));
    };
    let mut session = handle.borrow_mut();
    player_call_datum_handler_in_session(&mut session, id, receiver, name, args)
}

/// Explicit datum turn primitive. The player id is captured by the owning
/// caller and remains stable across any later host completion.
pub(crate) fn player_call_datum_handler_in_session(
    session: &mut RuntimeSession,
    id: u32,
    receiver: &DatumRef,
    name: Symbol,
    args: &Vec<DatumRef>,
) -> crate::player::handlers::datum_handlers::DatumDispatch {
    let Some(dispatch) = session.with_player(id, |mut context| {
        crate::player::handlers::datum_handlers::player_call_datum_handler(
            &mut context,
            receiver,
            name,
            args,
        )
    }) else {
        return crate::player::handlers::datum_handlers::DatumDispatch::Sync(Err(
            cancelled_scope_error(),
        ));
    };
    dispatch
}

/// Await-free explicit datum turn. The returned `Pending` variant owns all
/// receiver/argument handles and can be handed to the session host loop after
/// this short `RefCell` borrow ends.
pub(crate) async fn player_call_datum_handler_active_turn(
    receiver: &DatumRef,
    name: Symbol,
    args: &Vec<DatumRef>,
) -> crate::player::handlers::datum_handlers::DatumDispatch {
    player_call_datum_handler_active(receiver, name, args)
}

pub(crate) async fn player_call_datum_handler_turn_in_session(
    session: &mut RuntimeSession,
    id: u32,
    receiver: &DatumRef,
    name: Symbol,
    args: &Vec<DatumRef>,
) -> crate::player::handlers::datum_handlers::DatumDispatch {
    player_call_datum_handler_in_session(session, id, receiver, name, args)
}

/// Explicit datum turn for callers that have not yet adopted a retained
/// `RuntimeSession` handle. Pending is returned to the caller unchanged; it is
/// never represented as a script error.
pub(crate) async fn call_datum_handler_active(
    receiver: &DatumRef,
    name: Symbol,
    args: &Vec<DatumRef>,
) -> crate::player::handlers::datum_handlers::DatumDispatch {
    player_call_datum_handler_active(receiver, name, args)
}

pub(crate) fn retain_datum_pending(request: driver::InternalVmRequest, reason: String) {
    let id = active_player_id() as u32;
    if let Some(handle) = retained_session_handle() {
        handle
            .borrow_mut()
            .retain_deferred_request(id, request, reason);
    }
}

/// Explicit script-handler coordinator used by event and command callers.
/// It drives only turns that complete synchronously; a pending driver remains
/// retained by the session for the host continuation loop.
pub(crate) async fn player_call_script_handler_turn(
    receiver: Option<ScriptInstanceRef>,
    handler_ref: ScriptHandlerRef,
    args: &Vec<DatumRef>,
) -> ScriptHandlerTurn {
    let id = active_player_id() as u32;
    let Some(handle) = PLAYER_SESSION_HANDLE.with(|slot| slot.borrow().clone()) else {
        return ScriptHandlerTurn::Complete(Err(ScriptError::new(
            "runtime session is not initialized".to_owned(),
        )));
    };
    let mut session = handle.borrow_mut();
    player_call_script_handler_turn_in_session(&mut session, id, receiver, handler_ref, args).await
}

/// Explicit script turn primitive for production callers that retain a real
/// session handle. It captures the numeric player id supplied by that caller
/// and never re-resolves the active player after a suspension.
pub(crate) async fn player_call_script_handler_turn_in_session(
    session: &mut RuntimeSession,
    id: u32,
    receiver: Option<ScriptInstanceRef>,
    handler_ref: ScriptHandlerRef,
    args: &Vec<DatumRef>,
) -> ScriptHandlerTurn {
    player_call_script_handler_turn_in_session_sync(session, id, receiver, handler_ref, args)
}

/// Synchronous owner-bound script turn used by command/event coordinators.
/// Keeping the session borrow out of an `await` is required even though the
/// driver itself advances synchronously.
pub(crate) fn player_call_script_handler_turn_in_session_sync(
    session: &mut RuntimeSession,
    id: u32,
    receiver: Option<ScriptInstanceRef>,
    handler_ref: ScriptHandlerRef,
    args: &Vec<DatumRef>,
) -> ScriptHandlerTurn {
    let started = match session.start_handler(id, receiver, handler_ref, args, true) {
        Ok(started) => started,
        Err(error) => return ScriptHandlerTurn::Complete(Err(error)),
    };
    match started {
        Some(result) => ScriptHandlerTurn::Complete(Ok(result)),
        None => loop {
            match session.turn_handler(id) {
                Some(DriverTurn::Complete(result)) => {
                    break ScriptHandlerTurn::Complete(Ok(result));
                }
                Some(DriverTurn::Error(error)) => break ScriptHandlerTurn::Complete(Err(error)),
                // DriverTurn::Waiting is its cooperative one-op increment.
                // Continue the same owner-bound driver rather than exposing
                // an action-less pending command that no host ticket can wake.
                Some(DriverTurn::Waiting) => continue,
                Some(DriverTurn::Pending(action)) => break ScriptHandlerTurn::Pending(action),
                None => {
                    break ScriptHandlerTurn::Complete(Err(ScriptError::new(
                        "script handler driver disappeared".to_owned(),
                    )));
                }
            }
        },
    }
}

/// Owner-bound script handler turn. The returned `Pending` action remains
/// attached to the session driver until the host completes its capability.
pub async fn player_call_script_handler(
    receiver: Option<ScriptInstanceRef>,
    handler_ref: ScriptHandlerRef,
    args: &Vec<DatumRef>,
) -> ScriptHandlerTurn {
    player_call_script_handler_turn(receiver, handler_ref, args).await
}

/// True if an active movie/static script defines a handler named
/// `handler_name`. Used to decide, for the D4 `name(receiver, ..)` call form
/// (ObjCallV4), whether `name` is a movie handler that should take priority
/// over treating the first arg as a method receiver.
pub fn player_global_handler_exists(player: &DirPlayer, handler_name: &str) -> bool {
    get_active_static_script_refs(player)
        .iter()
        .any(|script_ref| {
            player
                .movie
                .cast_manager
                .get_script_by_ref(script_ref)
                .is_some_and(|script| {
                    script
                        .handler_names_raw
                        .iter()
                        .any(|name| name.eq_ignore_ascii_case(handler_name))
                })
        })
}

/// Which player `reserve_player_*` / `player_*` currently resolve to.
/// `0` = the main/host player (`PLAYER_OPT`); `N>0` = `NESTED_PLAYERS[N-1]`, a
/// Linked `#movie` sub-player. A task sets this to run the engine against a
/// specific player; because all ~1300 `reserve_player_*` call sites funnel
/// through the accessors below, switching this one value redirects the whole
/// engine — which is how a nested movie runs its own frame/scripts in the same
/// wasm instance (no second wasm, no cross-realm bridge). Task-scoping this so
/// each async task restores its own id on poll is a later step; today it stays
/// `0`, making these accessors identical to the previous `PLAYER_OPT`-only form.
pub static mut ACTIVE_PLAYER_ID: usize = 0;

#[inline]
pub(crate) fn active_player_id() -> usize {
    unsafe { ACTIVE_PLAYER_ID }
}

/// Clone the retained production session handle at an invocation boundary.
/// A caller keeps this handle together with its captured player id across an
/// async suspension, then borrows the session only for one synchronous turn.
pub(crate) fn retained_session_handle() -> Option<Rc<RefCell<RuntimeSession>>> {
    PLAYER_SESSION_HANDLE.with(|slot| slot.borrow().clone())
}
/// Registry of Linked `#movie` sub-players, indexed by `ACTIVE_PLAYER_ID - 1`.
pub static mut NESTED_PLAYERS: Vec<Option<DirPlayer>> = Vec::new();
/// Parallel to `NESTED_PLAYERS`: which `#movie` member owns each sub-player,
/// so `activate` can skip already-built members and callers can map member→id.
pub static mut NESTED_PLAYER_KEYS: Vec<Option<CastMemberRef>> = Vec::new();
/// Parallel to `NESTED_PLAYERS`: each sub-player's OWN event channel sender.
/// Events (mouseEnter/mouseWithin/mouseLeave, targeted callbacks) must be
/// processed by the sub's own event loop with `ACTIVE_PLAYER_ID` = its id, so
/// script-instance refs resolve against the sub's allocator. Routing them to the
/// host's `PLAYER_EVENT_TX` (as the old single-global path did) processed a sub's
/// instance ids against the HOST allocator → `get_script_instance` unwrap panic.
pub static mut NESTED_EVENT_TX: Vec<Option<Sender<PlayerVMEvent>>> = Vec::new();

/// The event-channel sender for the currently active player (host id 0 →
/// `PLAYER_EVENT_TX`, else the sub's `NESTED_EVENT_TX` slot). Used by the
/// `player_dispatch_*` event helpers so a sub's events reach the sub's loop.
pub fn active_event_tx() -> Option<Sender<PlayerVMEvent>> {
    unsafe {
        if ACTIVE_PLAYER_ID == 0 {
            PLAYER_EVENT_TX.clone()
        } else {
            NESTED_EVENT_TX
                .get(ACTIVE_PLAYER_ID - 1)
                .and_then(|o| o.clone())
        }
    }
}

/// A nested `#movie` sub-player's Flash sprites dispatch to Ruffle under a
/// synthetic positive sprite key `BASE + player_id*STRIDE + local_channel` so
/// they neither collide with the host's channel keys nor look like the negative
/// off-screen-3D-texture keys (which are capture-count-limited). `update_flash_frame`
/// decodes it back to (player_id, channel) and routes the captured frame into the
/// SUB's `flash_frame_buffers`. STRIDE bounds a sub's channel count; BASE keeps
/// the synthetic keys clear of real channels.
pub const NESTED_FLASH_BASE: i32 = 100_000;
pub const NESTED_FLASH_STRIDE: i32 = 1_000;

/// Root players, including nonzero session player IDs, retain their local
/// channel key. Only classified legacy nested players use synthetic keys.
pub(crate) fn flash_host_sprite_key(is_nested_host: bool, player_id: usize, channel: i16) -> i32 {
    if is_nested_host {
        nested_flash_key(player_id, channel)
    } else {
        channel as i32
    }
}

/// Encode a nested sub-player's Flash dispatch key (see `NESTED_FLASH_BASE`).
pub fn nested_flash_key(player_id: usize, channel: i16) -> i32 {
    NESTED_FLASH_BASE + (player_id as i32) * NESTED_FLASH_STRIDE + channel as i32
}

/// Decode a synthetic nested Flash key back to `(player_id, channel)`, or `None`
/// if it's an ordinary host channel key.
pub fn decode_nested_flash_key(key: i32) -> Option<(usize, i16)> {
    if key < NESTED_FLASH_BASE {
        return None;
    }
    let rel = key - NESTED_FLASH_BASE;
    Some((
        (rel / NESTED_FLASH_STRIDE) as usize,
        (rel % NESTED_FLASH_STRIDE) as i16,
    ))
}

/// Active-player id (`>0`) of the sub-player built for `member_ref`, if any.
pub fn nested_player_id(member_ref: &CastMemberRef) -> Option<usize> {
    unsafe {
        NESTED_PLAYER_KEYS
            .iter()
            .position(|k| k.as_ref() == Some(member_ref))
            .map(|i| i + 1)
    }
}

pub(crate) fn render_nested_player_bitmap(
    sub: &mut DirPlayer,
    mut symbols: Option<&mut SymbolTable>,
    id: usize,
) -> Option<bitmap::bitmap::Bitmap> {
    let (rw, rh) = (sub.movie.rect.width(), sub.movie.rect.height());
    if !(rw >= 1 && rh >= 1 && rw <= 4096 && rh <= 4096) {
        warn!("[nested] skip render: bad sub rect {}x{}", rw, rh);
        return None;
    }
    sub.drain_allocator_reclaims();
    let w = rw.max(1) as u16;
    let h = rh.max(1) as u16;
    let webgl_bmp = symbols.as_deref_mut().and_then(|symbols| {
        crate::rendering::with_renderer_mut(|r| match r {
            Some(crate::rendering_gpu::DynamicRenderer::WebGL2(webgl)) => webgl
                .render_player_to_bitmap(sub, symbols, id, w as u32, h as u32)
                .ok(),
            _ => None,
        })
    });
    Some(webgl_bmp.unwrap_or_else(|| {
        let mut bmp = bitmap::bitmap::Bitmap::new(
            w,
            h,
            32,
            32,
            0,
            bitmap::bitmap::PaletteRef::BuiltIn(bitmap::bitmap::get_system_default_palette()),
        );
        crate::rendering::render_stage_to_bitmap(sub, &mut bmp, None);
        bmp
    }))
}

#[inline(always)]
unsafe fn active_player_ptr() -> *mut DirPlayer {
    if let Some(handle) = PLAYER_SESSION_HANDLE.with(|slot| slot.borrow().clone()) {
        // `player_ref`/`player_mut` are legacy raw-reference entrypoints. The
        // production owner is still the thread-local retained Rc handle; this
        // pointer is derived from that RefCell only for callers that have not
        // yet crossed to reserve_player_*.
        let session = (*Rc::as_ptr(&handle)).as_ptr();
        let mut player_ptr: *mut DirPlayer = std::ptr::null_mut();
        let _ = (*session).with_player(ACTIVE_PLAYER_ID as u32, |context| {
            player_ptr = context.player as *mut DirPlayer;
        });
        if !player_ptr.is_null() {
            return player_ptr;
        }
    }
    if ACTIVE_PLAYER_ID == 0 {
        PLAYER_OPT.as_mut().unwrap_unchecked() as *mut DirPlayer
    } else {
        NESTED_PLAYERS
            .get_unchecked_mut(ACTIVE_PLAYER_ID - 1)
            .as_mut()
            .unwrap_unchecked() as *mut DirPlayer
    }
}

pub fn reserve_player_ref<T, F>(callback: F) -> T
where
    F: FnOnce(&DirPlayer) -> T,
{
    if let Some(handle) = PLAYER_SESSION_HANDLE.with(|slot| slot.borrow().clone()) {
        let mut session = handle.borrow_mut();
        return session
            .with_player(active_player_id() as u32, |context| {
                callback(context.player)
            })
            .expect("active production player is missing");
    }
    unsafe {
        let player = &*active_player_ptr();
        callback(player)
    }
}

#[inline(always)]
/// An input handler is about to run: hold the stage redraw until the next
/// exitFrame has settled what it changes (`DirPlayer::draw_hold_since_ms`).
pub fn hold_draw_for_input_handler() {
    reserve_player_mut(|player| {
        if player.draw_hold_since_ms.is_none() {
            player.draw_hold_since_ms = Some(chrono::Utc::now().timestamp_millis());
        }
    });
}

/// Director runs one handler at a time: input that arrives while a frame
/// handler is busy-waiting (`repeat while ... updateStage()`) is queued until
/// the handler returns. The busy-wait yield lets the command and event loops
/// run inside that wait, so a mouseEnter fired mid-animation on Maaneby's map
/// build sequence, where the original ignores the mouse until it is over.
/// Waits for the gap; never drops the input.
pub async fn wait_for_handler_gap() {
    loop {
        let busy = reserve_player_ref(|player| {
            player.is_playing
                && !player.is_yield_safe()
                && !player.in_mouse_command
                && !player.command_handler_yielding
        });
        if !busy {
            return;
        }
        let _ = timeout(Duration::from_millis(4), future::pending::<()>()).await;
    }
}

/// Wait for an input-handler gap on one captured session/player owner.
///
/// The legacy [`wait_for_handler_gap`] path reads the process-wide active
/// player and remains for old browser/bootstrap callers. Session-owned
/// command loops must use this capability-bound variant so a nested player or
/// a replacement after reset cannot block or release input for another owner.
pub async fn wait_for_handler_gap_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
) -> Result<(), ScriptError> {
    loop {
        if !owner.is_arena_live() {
            return Err(cancelled_scope_error());
        }
        let Some((owner_matches, busy)) = session.borrow_mut().with_player(player_id, |context| {
            (
                owner.same_identity(&context.player.owner),
                context.player.is_playing
                    && !context.player.is_yield_safe()
                    && !context.player.in_mouse_command
                    && !context.player.command_handler_yielding,
            )
        }) else {
            return Err(cancelled_scope_error());
        };
        if !owner_matches {
            return Err(cancelled_scope_error());
        }
        if !busy {
            return Ok(());
        }
        let _ = timeout(Duration::from_millis(4), future::pending::<()>()).await;
    }
}

pub fn reserve_player_mut<T, F>(callback: F) -> T
where
    F: FnOnce(&mut DirPlayer) -> T,
{
    if let Some(handle) = PLAYER_SESSION_HANDLE.with(|slot| slot.borrow().clone()) {
        let mut session = handle.borrow_mut();
        return session
            .with_player(active_player_id() as u32, |context| {
                callback(context.player)
            })
            .expect("active production player is missing");
    }
    unsafe {
        let player = &mut *active_player_ptr();
        callback(player)
    }
}

/// Direct reference access without closure overhead.
/// Caller must ensure no mutable references exist.
#[inline(always)]
pub unsafe fn player_ref() -> &'static DirPlayer {
    &*active_player_ptr()
}

/// Direct mutable reference access without closure overhead.
/// Caller must ensure no other references exist.
#[inline(always)]
pub unsafe fn player_mut() -> &'static mut DirPlayer {
    &mut *active_player_ptr()
}

/// Future wrapper that sets `ACTIVE_PLAYER_ID` to a captured id for the duration
/// of every poll, then restores it. This is what makes multi-player safe under
/// the shared async executor: a task spawned for player X always runs against X
/// even when it's polled while another player's frame is on the stack.
struct WithActivePlayer<F> {
    id: usize,
    inner: F,
}
impl<F: std::future::Future> std::future::Future for WithActivePlayer<F> {
    type Output = F::Output;
    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        // SAFETY: standard pin-projection of the single `inner` field; `id` is Copy.
        let this = unsafe { self.get_unchecked_mut() };
        let inner = unsafe { std::pin::Pin::new_unchecked(&mut this.inner) };
        unsafe {
            let prev = ACTIVE_PLAYER_ID;
            ACTIVE_PLAYER_ID = this.id;
            let r = inner.poll(cx);
            ACTIVE_PLAYER_ID = prev;
            r
        }
    }
}

/// Await `fut` with `ACTIVE_PLAYER_ID` pinned to `id` across every poll, so async
/// awaits inside it keep resolving the engine against `id` even after yielding
/// (a plain `ACTIVE_PLAYER_ID = id` before an `.await` is undone by the enclosing
/// task's own `WithActivePlayer` on the next poll). Used by `tellcall` to run a
/// command inside a nested `#movie` sub-player.
pub fn with_active_player<F: std::future::Future>(
    id: usize,
    fut: F,
) -> impl std::future::Future<Output = F::Output> {
    WithActivePlayer { id, inner: fut }
}

/// Spawn a local task bound to the *currently active* player, so its async work
/// always resolves the engine against the player it was spawned for — even when
/// interleaved with another player's frame. Replaces raw `spawn_local` across
/// the engine; the captured id is `0` (main player) everywhere today.
pub fn spawn_player_local<F>(fut: F)
where
    F: std::future::Future<Output = ()> + 'static,
{
    // `use ... as raw` (no trailing `(`) so the bulk `crate::player::spawn_player_local(`
    // → `spawn_player_local(` rewrite doesn't turn this into infinite recursion.
    use async_std::task::spawn_local as raw;
    let id = unsafe { ACTIVE_PLAYER_ID };
    raw(WithActivePlayer { id, inner: fut });
}

fn reserve_player_mut_async<F, R>(callback: F) -> impl Future<Output = R>
where
    F: for<'a> FnOnce(&'a mut DirPlayer) -> Pin<Box<dyn Future<Output = R> + 'a>>,
{
    async move {
        unsafe {
            let player = crate::player::player_mut();
            callback(player).await
        }
    }
}

#[allow(dead_code)]
#[derive(Clone)]
pub enum ScriptReceiver {
    Script(CastMemberRef),
    ScriptInstance(ScriptInstanceRef),
    ScriptText(String),
}

/// One active handler invocation on the trampoline's explicit scope stack.
/// Holds everything the opcode loop needs for that frame, so the driver can
/// suspend a caller (push a callee frame) and resume it without recursion or a
/// boxed future. `_profile` keeps the handler's profiling frame open for the
/// frame's lifetime (dropped on teardown).
/// A frame's register-IR execution state.
///
/// Just the compiled handler now. The dense local file used to live here and be
/// synced to and from `scope.locals` around every escape; both sides share
/// `scope.locals` directly, so there is no second copy to carry or reconcile —
/// and a suspended caller keeps its locals for free, because they were never
/// anywhere but its own scope.
pub(crate) struct IrState {
    compiled: std::rc::Rc<crate::player::compiled::CompiledHandler>,
}

/// Identity and generation capability for one live handler scope.
///
/// The slot is only a locator. Every access must validate the owner, live
/// generation, and (for teardown/return delivery) top-of-stack position first.
#[derive(Clone)]
pub(crate) struct ScopeToken {
    owner: OwnerToken,
    slot: ScopeRef,
    generation: u64,
    epoch: u64,
}

impl ScopeToken {
    #[inline]
    pub(crate) fn slot(&self) -> ScopeRef {
        self.slot
    }

    #[inline]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }

    #[inline]
    pub(crate) fn epoch(&self) -> u64 {
        self.epoch
    }

    #[inline]
    pub(crate) fn validate_active(&self, player: &DirPlayer) -> bool {
        if !self.owner.same_identity(&player.owner)
            || !self.owner.is_arena_live()
            || self.epoch != player.scope_invalidation_epoch
        {
            return false;
        }
        if self.slot >= player.scope_count as usize {
            return false;
        }
        player.scopes.get(self.slot).is_some_and(|scope| {
            scope.scope_ref == self.slot && scope.generation == self.generation
        })
    }

    #[inline]
    pub(crate) fn validate_top(&self, player: &DirPlayer) -> bool {
        self.validate_active(player) && self.slot + 1 == player.scope_count as usize
    }
}

/// Snapshot the stack boundary around a synchronous virtual/JS callback. A
/// callback may re-enter the player, but setup may continue only if its owner,
/// whole-stack epoch, depth, and anchored parent are still the same.
#[derive(Clone)]
pub(crate) struct SetupExpectation {
    owner: OwnerToken,
    epoch: u64,
    depth: u32,
    parent: Option<ScopeToken>,
}

impl SetupExpectation {
    pub(crate) fn capture(player: &DirPlayer, parent: Option<&ScopeToken>) -> Self {
        let parent = parent.cloned().or_else(|| {
            let slot = player.scope_count.checked_sub(1)? as ScopeRef;
            let scope = player.scopes.get(slot)?;
            Some(ScopeToken {
                owner: player.owner.clone(),
                slot,
                generation: scope.generation,
                epoch: player.scope_invalidation_epoch,
            })
        });
        Self {
            owner: player.owner.clone(),
            epoch: player.scope_invalidation_epoch,
            depth: player.scope_count,
            parent,
        }
    }

    pub(crate) fn validate(&self, player: &DirPlayer) -> bool {
        self.owner.same_identity(&player.owner)
            && self.owner.is_arena_live()
            && self.epoch == player.scope_invalidation_epoch
            && self.depth == player.scope_count
            && self
                .parent
                .as_ref()
                .map_or(self.depth == 0, |parent| parent.validate_top(player))
    }

    pub(crate) fn parent_scope(&self) -> Option<ScopeToken> {
        self.parent.clone()
    }
}

pub(crate) struct HandlerFrame {
    pub(crate) ir: Option<IrState>,
    pub(crate) ctx: BytecodeHandlerContext,
    pub(crate) is_frame_script: bool,
    pub(crate) script_member_ref: CastMemberRef,
    pub(crate) handler_name: Symbol,
    pub(crate) push_return: bool,
    pub(crate) _profile: ProfileScopeOwned,
}

pub(crate) fn cancelled_scope_error() -> ScriptError {
    ScriptError::new_code(
        ScriptErrorCode::Abort,
        "handler scope was cancelled before it could resume".to_owned(),
    )
}

/// Finish one frame only while its token still identifies the live top scope.
/// The validation must precede every indexed scope access and every mutation.
fn teardown_handler_frame(
    player: &mut DirPlayer,
    token: &ScopeToken,
    is_frame_script: bool,
) -> Result<ScopeResult, ()> {
    if !token.validate_top(player) {
        return Err(());
    }
    if player.movie.trace_script {
        trace_output(player, "--> end");
    }
    if !token.validate_top(player) {
        return Err(());
    }
    let scope_ref = token.slot();
    let result = {
        let scope = player.scopes.get(scope_ref).ok_or(())?;
        player.last_handler_result = scope.return_value.clone();
        ScopeResult {
            passed: scope.passed,
            return_value: scope.return_value.clone(),
        }
    };
    if !pop_valid_top_frame(player, token, is_frame_script) {
        return Err(());
    }
    Ok(result)
}

#[inline]
fn pop_frame_bookkeeping(player: &mut DirPlayer, is_frame_script: bool) {
    player.pop_scope();
    player.handler_stack_depth = player.handler_stack_depth.saturating_sub(1);
    if is_frame_script {
        player.in_frame_script = false;
    }
}

#[inline]
fn pop_valid_top_frame(player: &mut DirPlayer, token: &ScopeToken, is_frame_script: bool) -> bool {
    if !token.validate_top(player) {
        return false;
    }
    pop_frame_bookkeeping(player, is_frame_script);
    true
}

/// Unwind a current frame and then only consecutive parent frames whose tokens
/// still identify the live top scope. A stale token stops the unwind without
/// touching the replacement scope or player depth/flags.
fn unwind_handler_frames(
    player: &mut DirPlayer,
    token: &ScopeToken,
    is_frame_script: bool,
    parents: &mut Vec<HandlerFrame>,
) -> bool {
    if !pop_valid_top_frame(player, token, is_frame_script) {
        return false;
    }

    while let Some(parent) = parents.last() {
        if !pop_valid_top_frame(player, &parent.ctx.scope, parent.is_frame_script) {
            return false;
        }
        parents.pop();
    }
    true
}

/// Deliver a nested result only to the validated parent top scope. This keeps
/// `passed` propagation and an optional return push atomic with the guard.
fn deliver_scope_return(
    player: &mut DirPlayer,
    token: &ScopeToken,
    result: &ScopeResult,
    push_return: bool,
) -> bool {
    if !token.validate_top(player) {
        return false;
    }
    let scope = player.scopes.get_mut(token.slot()).unwrap();
    if result.passed {
        scope.passed = true;
    }
    if push_return {
        scope.stack.push(result.return_value.clone());
    }
    true
}

pub(crate) enum FrameSetup {
    /// A virtual/JS handler answered synchronously — no frame was pushed.
    Early(ScopeResult),
    Pending(crate::player::driver::SetupCallbackPlan),
    Frame(HandlerFrame),
}

pub(crate) enum FrameTransfer {
    /// The frame finished (Ret/Stop/end-of-array/scope reuse).
    Done,
    /// The frame hit a Lingo-handler call; the driver should push a callee frame.
    Call(PendingCall),
}

#[derive(Clone)]
pub(crate) struct HandlerPlan {
    code: HandlerCode,
    variable_multiplier: u32,
    is_frame_script: bool,
    first_param_is_me: bool,
}

fn capture_handler_plan(
    player: &DirPlayer,
    script_member_ref: &CastMemberRef,
    handler_name: &Symbol,
) -> Result<HandlerPlan, ScriptError> {
    let script_rc = player
        .movie
        .cast_manager
        .get_script_by_ref(script_member_ref)
        .cloned()
        .ok_or_else(|| {
            ScriptError::new_code(
                ScriptErrorCode::HandlerNotFound,
                format!("Script {} not found", script_member_ref.cast_member),
            )
        })?;
    let script = script_rc.as_ref();
    let handler_rc = script
        .get_own_handler(handler_name.clone())
        .cloned()
        .ok_or_else(|| {
            ScriptError::new_code(
                ScriptErrorCode::HandlerNotFound,
                format!(
                    "Handler {:?} not found for script {}",
                    handler_name, script.name
                ),
            )
        })?;
    let cast = player
        .movie
        .cast_manager
        .get_cast(script.member_ref.cast_lib as u32)
        .unwrap();
    let names = cast.name_symbols.clone();
    let first_param_is_me = handler_rc
        .argument_name_ids
        .first()
        .and_then(|id| cast.lctx.as_ref()?.names.get(*id as usize))
        .is_some_and(|name| name.eq_ignore_ascii_case("me"));
    let is_frame_script = player
        .movie
        .score
        .get_script_in_frame(player.movie.current_frame)
        .is_some_and(|fs| {
            script_member_ref.cast_lib == fs.cast_lib as i32
                && script_member_ref.cast_member == fs.cast_member as i32
        });
    let variable_multiplier =
        crate::director::file::get_variable_multiplier(cast.capital_x, cast.dir_version);
    Ok(HandlerPlan {
        code: HandlerCode {
            script: script_rc,
            handler: handler_rc,
            names,
        },
        variable_multiplier,
        is_frame_script,
        first_param_is_me,
    })
}

/// Set up a handler call's scope: virtual/JS early-outs, profiling frame,
/// is_frame_script, handler-depth++, push_scope + args, ctx, and trace entry.
/// Returns a ready [`HandlerFrame`] for the driver, or an `Early` result when a
/// virtual/JS handler answered synchronously. This is the setup half of the old
/// inline body of `player_call_script_handler_raw_args`, made reusable so the
/// driver can set up nested frames without recursion.
pub(crate) fn setup_handler_frame(
    session: &mut RuntimeSession,
    player_id: u32,
    receiver: Option<ScriptInstanceRef>,
    handler_ref: ScriptHandlerRef,
    arg_list: &Vec<DatumRef>,
    use_raw_arg_list: bool,
    push_return: bool,
    expectation: SetupExpectation,
) -> Result<FrameSetup, ScriptError> {
    let (script_member_ref, handler_name) = handler_ref;

    if !session
        .with_player(player_id, |ctx| expectation.validate(ctx.player))
        .unwrap_or(false)
    {
        return Err(cancelled_scope_error());
    }
    // Resolve the old cast's owned handler before callbacks can mount another
    // movie. If it is absent, defer that exact error until overrides decline.
    let plan_result = session
        .with_player(player_id, |ctx| {
            capture_handler_plan(ctx.player, &script_member_ref, &handler_name)
        })
        .unwrap_or_else(|| Err(cancelled_scope_error()));

    // Virtual script handler.
    let virtual_result = session
        .with_player(player_id, |mut ctx| {
            ctx.with_player_and_symbols(|player, symbols| {
                virtual_scripts::VirtualScriptRegistry::try_call_handler(
                    player,
                    symbols,
                    &script_member_ref,
                    receiver.as_ref(),
                    handler_name.clone(),
                    arg_list,
                )
            })
        })
        .unwrap_or_else(|| Err(cancelled_scope_error()));
    if !session
        .with_player(player_id, |ctx| expectation.validate(ctx.player))
        .unwrap_or(false)
    {
        return Err(cancelled_scope_error());
    }
    match virtual_result {
        Ok(Some(return_value)) => {
            return Ok(FrameSetup::Early(ScopeResult {
                return_value,
                passed: false,
            }));
        }
        Ok(None) => {}
        Err(e) => return Err(e),
    }

    // JS Lingo handler (XDR JSScript literal area). Classification is done
    // against the session-owned registry before any scope is allocated. The
    // executor receives owned values and invokes JS after releasing this VM
    // borrow; a completion then resumes the saved setup expectation.
    let handler_name_text = session
        .with_player(player_id, |ctx| {
            ctx.symbols
                .display(&handler_name)
                .map(str::to_owned)
                .map_err(|_| {
                    ScriptError::new_code(
                        ScriptErrorCode::InvalidReference,
                        "foreign or stale setup handler symbol".to_owned(),
                    )
                })
        })
        .ok_or_else(cancelled_scope_error)??;
    let js_handler = session
        .with_player(player_id, |ctx| ctx.player.owner.clone())
        .map(|owner| {
            js_lingo_loader::has_js_handler_explicit(
                session,
                player_id,
                &owner,
                &script_member_ref,
                &handler_name_text,
            )
        })
        .unwrap_or(false);
    if js_handler {
        let owner = session
            .with_player(player_id, |ctx| ctx.player.owner.clone())
            .ok_or_else(cancelled_scope_error)?;
        let args = session
            .with_player(player_id, |ctx| {
                arg_list
                    .iter()
                    .map(|value| {
                        crate::player::driver::checked_internal_datum(
                            ctx.player,
                            ctx.symbols,
                            value,
                        )
                        .cloned()
                    })
                    .collect::<Result<Vec<_>, _>>()
            })
            .ok_or_else(cancelled_scope_error)??;
        return Ok(FrameSetup::Pending(
            crate::player::driver::SetupCallbackPlan {
                player_id,
                owner,
                kind: crate::player::driver::SetupCallbackKind::JavaScript,
                script_ref: script_member_ref,
                handler_name: handler_name_text,
                receiver: receiver.map(Datum::ScriptInstanceRef),
                args,
                retained_plan: plan_result.ok(),
                expectation,
                child_policy: None,
                push_return,
            },
        ));
    }

    if !session
        .with_player(player_id, |ctx| expectation.validate(ctx.player))
        .unwrap_or(false)
    {
        return Err(cancelled_scope_error());
    }
    let plan = match plan_result {
        Ok(plan) => plan,
        Err(err) => return Err(err),
    };

    let _handler_scope = ProfileScopeOwned::new(format!("{:?}", handler_name));

    let is_frame_script = plan.is_frame_script;

    let (scope_ref, scope_owner, scope_epoch) = session
        .with_player(player_id, |ctx| {
            let player = ctx.player;
            let handler_name_id = plan.code.handler.name_id;
            let first_param_is_me = plan.first_param_is_me;

            let is_instance_receiver = receiver.is_some();
            let receiver_arg = if let Some(script_instance_ref) = receiver.as_ref() {
                Some(Datum::ScriptInstanceRef(script_instance_ref.clone()))
            } else {
                // No explicit instance receiver: `me` is the script itself (a
                // ScriptRef). Director binds `me` to the script for
                // `script("x").handler()` calls — for MOVIE scripts too, whose
                // sibling-handler calls rely on it (Neopets DGS `secure` movie
                // script does `me.sub(...)` inside `decrypt_pc927634892`, which
                // errored with `me`=VOID).
                Some(Datum::ScriptRef(script_member_ref.clone()))
            };

            // Whether the handler declares `me` as its first parameter. The receiver
            // is only PREPENDED as arg0 (filling that param) when it does. Instance
            // receivers (behaviors/parents) always declare `me`, so they always
            // prepend. Movie scripts are the subtle case: `script("x").handler()`
            // calls whose handler declares `me` (DGS `decrypt_pc927634892`) prepend,
            // but a plain global/event handler like `on streamStatus url, state,
            // bytesSoFar, bytesTotal` must NOT — prepending shifted every param by
            // one (bogey_nights got `bytesSoFar` = the "Complete" state string).
            let scope_ref = player.push_scope();
            player.handler_stack_depth += 1;
            if is_frame_script {
                player.in_frame_script = true;
            }
            {
                let scope = player.scopes.get_mut(scope_ref).unwrap();
                scope.script_ref = script_member_ref.clone();
                scope.receiver = receiver;
                scope.handler_name_id = handler_name_id;
            };

            if let Some(receiver_arg) = receiver_arg {
                if !use_raw_arg_list && (is_instance_receiver || first_param_is_me) {
                    let arg_ref = player.alloc_datum(receiver_arg);
                    let scope = player.scopes.get_mut(scope_ref).unwrap();
                    scope.args.push(arg_ref);
                }
            }

            let scope = player.scopes.get_mut(scope_ref).unwrap();
            scope.args.extend_from_slice(arg_list);

            Ok::<_, ScriptError>((
                scope_ref,
                player.owner.clone(),
                player.scope_invalidation_epoch,
            ))
        })
        .ok_or_else(cancelled_scope_error)??;

    let ctx = BytecodeHandlerContext {
        scope: ScopeToken {
            owner: scope_owner,
            slot: scope_ref,
            generation: session
                .with_player(player_id, |ctx| {
                    ctx.player.scopes.get(scope_ref).unwrap().generation
                })
                .ok_or_else(cancelled_scope_error)?,
            epoch: scope_epoch,
        },
        code: plan.code,
        multiplier: plan.variable_multiplier,
    };

    // Size the dense local file for this handler. The scope pool retains
    // capacity across reuse, so after warmup this is a memset rather than an
    // allocation.
    {
        let n_locals = ctx.code.handler.local_name_ids.len();
        session
            .with_player(player_id, |runtime| {
                runtime
                    .player
                    .scopes
                    .get_mut(scope_ref)
                    .unwrap()
                    .ensure_locals(n_locals);
            })
            .ok_or_else(cancelled_scope_error)?;
    }

    // Trace handler entry if traceScript is enabled. The trace dispatch can
    // re-enter/reset the player, so do not clear any handler state until a
    // fresh token validation succeeds after that callback.
    let trace_message = session
        .with_player(player_id, |ctx| {
            ctx.player.movie.trace_script.then(|| {
                let (cast_lib, cast_member) =
                    (script_member_ref.cast_lib, script_member_ref.cast_member);
                format!(
                    "== Script: (member {} of castLib {}) Handler: {:?}",
                    cast_member, cast_lib, handler_name
                )
            })
        })
        .ok_or_else(cancelled_scope_error)?;
    if let Some(message) = trace_message {
        session
            .with_player(player_id, |ctx| trace_output(ctx.player, &message))
            .ok_or_else(cancelled_scope_error)?;
        if !session
            .with_player(player_id, |runtime| ctx.scope.validate_top(runtime.player))
            .unwrap_or(false)
        {
            return Err(cancelled_scope_error());
        }
        use crate::player::bytecode::handler_manager::EXPRESSION_TRACKER;
        EXPRESSION_TRACKER.with(|tracker| {
            tracker.borrow_mut().clear();
        });
    }

    // Compile to register IR on first call and cache it on the HandlerDef.
    // `bp_generation` rides along in the same read: the cached IR has the
    // breakpoint list baked into it as escapes, so it is only valid for the
    // generation it was built under. Untouched by a session with no
    // breakpoints, where the generation stays 0 and the cache never misses.
    let (ir_enabled, bp_generation) = session
        .with_player(player_id, |ctx| {
            (
                ctx.player.ir_enabled,
                ctx.player.breakpoint_manager.generation,
            )
        })
        .ok_or_else(cancelled_scope_error)?;
    let ir = if ir_enabled {
        let handler_def = ctx.code.handler.as_ref();
        let cached = {
            let slot = handler_def.compiled_ir.borrow();
            slot.clone()
        };
        let compiled = match cached {
            Some((cached_gen, c)) if cached_gen == bp_generation => c,
            _ => {
                let c = crate::player::compiled::compile(handler_def, ctx.multiplier)
                    .map(|mut c| {
                        // Before `is_worth_compiling`, so the escape ceiling is
                        // judged on the op stream that will actually run.
                        let breakpoints = session
                            .with_player(player_id, |ctx| {
                                ctx.player.breakpoint_manager.breakpoints.clone()
                            })
                            .unwrap_or_default();
                        crate::player::compiled::apply_breakpoints(
                            &mut c,
                            &breakpoints,
                            &ctx.code.script.name,
                            handler_name_text.as_str(),
                        );
                        c
                    })
                    .filter(crate::player::compiled::is_worth_compiling)
                    .map(std::rc::Rc::new);
                // Counted here, not inside `is_worth_compiling`, so the result
                // is per DISTINCT handler: the outcome is cached on the
                // HandlerDef, so this arm runs at most once per handler (or
                // once per breakpoint-list change, which is interactive).
                crate::player::interp_stats::record_compile_outcome(c.is_some());
                *handler_def.compiled_ir.borrow_mut() = Some((bp_generation, c.clone()));
                c
            }
        };
        compiled.map(|c| IrState { compiled: c })
    } else {
        None
    };
    // Call-weighted, unlike the compile outcome above: one hot handler the IR
    // declined matters more than a hundred cold ones.
    crate::player::interp_stats::record_handler_call(ir.is_some(), arg_list.len());

    Ok(FrameSetup::Frame(HandlerFrame {
        ir,
        ctx,
        is_frame_script,
        script_member_ref,
        handler_name,
        push_return,
        _profile: _handler_scope,
    }))
}

/// Dispatch stopMovie events and end all sprites. Used by both `run_frame_loop`
/// (when `is_playing` becomes false) and `transition_to_net_movie`.
async fn stop_movie_sequence() {
    dispatch_system_event_to_timeouts(BuiltInSymbol::StopMovie, &vec![]).await;

    reserve_player_mut(|player| {
        player.timeout_manager.clear();
    });

    player_wait_available().await;

    if let Err(err) =
        player_invoke_global_event(Symbol::builtin(BuiltInSymbol::StopMovie), &vec![]).await
    {
        if err.code != ScriptErrorCode::Abort {
            reserve_player_mut(|player| player.on_script_error(&err));
        }
    }

    player_wait_available().await;

    let ended_sprite_nums =
        reserve_player_mut_async(|player| Box::pin(async move { player.end_all_sprites().await }))
            .await;

    player_wait_available().await;

    reserve_player_mut(|player| {
        for (score_source, sprite_num) in ended_sprite_nums.iter() {
            if let Some(sprite) =
                get_score_sprite_mut(&mut player.movie, score_source, *sprite_num as i16)
            {
                sprite.exited = true;
            }
        }
    });

    player_wait_available().await;
}

/// Normalise a projector `--do` payload to Lingo's own string quoting.
///
/// Lingo has exactly one string delimiter, `"` (the grammar's `quote` rule),
/// and so does Director. Launcher command lines nonetheless write the payload
/// with `'`, because the whole `--do` argument is itself inside double quotes
/// and nesting them would need escaping:
///
/// ```text
/// --do "member('gameUrl').text = 'http://…/agent_freeride.dcr'"
/// ```
///
/// The projector undoes that before evaluating. Only rewrite when the payload
/// contains NO double quote of its own — otherwise it is already Lingo-quoted
/// and an apostrophe in it is a literal apostrophe, not a delimiter.
fn normalize_startup_do_quotes(code: &str) -> String {
    if code.contains('"') || !code.contains('\'') {
        return code.to_string();
    }
    code.replace('\'', "\"")
}

/// Evaluate the pending `--do` payload, once, before `prepareMovie`.
///
/// Failure is reported but never fatal: the projector runs the movie either
/// way, and a movie that does not depend on the payload must still start.
pub(crate) async fn run_startup_do_before() {
    let code = reserve_player_mut(|player| player.startup_do_before.take());
    eval_startup_payload(code, "--doBefore").await;
}

async fn run_startup_do() {
    let code = reserve_player_mut(|player| player.startup_do.take());
    eval_startup_payload(code, "--do").await;
}

/// `--go N`: jump to a frame once the movie is running. Consumed on use.
///
/// Routed through the same evaluator as `--do` rather than poking
/// `movie.current_frame`: SPR is itself a Director movie issuing a plain
/// `go`, and the real handler is what performs the frame jump properly
/// (async, marker-aware).
async fn run_startup_go() {
    let frame = reserve_player_mut(|player| player.startup_go.take());
    let Some(frame) = frame else { return };
    eval_startup_payload(Some(format!("go({})", frame)), "--go").await;
}

async fn eval_startup_payload(code: Option<String>, flag: &str) {
    let Some(code) = code else { return };
    let code = normalize_startup_do_quotes(code.trim());
    if code.is_empty() {
        return;
    }
    debug!(">>> startup {}: {}", flag, code);
    let Some(session_handle) = retained_session_handle() else {
        crate::console_error!(
            "startup {} FAILED ({}): runtime session is not initialized",
            flag,
            code
        );
        return;
    };
    let player_id = active_player_id() as u32;
    let turn = {
        let mut session = session_handle.borrow_mut();
        let eval_id = match crate::player::eval::start_eval_lingo_command(
            &mut session,
            player_id,
            code.clone(),
        ) {
            Ok(eval_id) => eval_id,
            Err(err) => {
                crate::console_error!("startup {} FAILED ({}): {}", flag, code, err.message);
                return;
            }
        };
        session.turn_eval(eval_id)
    };
    match turn {
        crate::player::eval::EvalTurn::Complete(Ok(_)) => debug!(">>> startup {} completed", flag),
        crate::player::eval::EvalTurn::Complete(Err(err)) => {
            crate::console_error!("startup {} FAILED ({}): {}", flag, code, err.message);
        }
        crate::player::eval::EvalTurn::Pending { .. } => {
            debug!(
                ">>> startup {} suspended; session retained continuation",
                flag
            );
        }
    }
}

async fn eval_startup_payload_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
    code: Option<String>,
    flag: &str,
) -> Result<(), ScriptError> {
    let Some(code) = code else {
        return Ok(());
    };
    let code = normalize_startup_do_quotes(code.trim());
    if code.is_empty() {
        return Ok(());
    }
    debug!(">>> startup {}: {}", flag, code);
    if !session
        .borrow_mut()
        .with_player(player_id, |context| {
            owner.same_identity(&context.player.owner) && owner.is_arena_live()
        })
        .unwrap_or(false)
    {
        return Err(cancelled_scope_error());
    }
    match eval_lingo_command_owned(session.clone(), player_id, owner.clone(), code.clone()).await {
        Ok(_) => debug!(">>> startup {} completed", flag),
        Err(error) => {
            if !session
                .borrow_mut()
                .with_player(player_id, |context| {
                    owner.same_identity(&context.player.owner) && owner.is_arena_live()
                })
                .unwrap_or(false)
            {
                return Err(cancelled_scope_error());
            }
            // Startup payloads are explicitly best-effort in the projector:
            // a bad --do must not prevent the movie from mounting.
            crate::console_error!("startup {} FAILED ({}): {}", flag, code, error.message);
        }
    }
    Ok(())
}

async fn run_startup_do_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
    before: bool,
) -> Result<(), ScriptError> {
    let code = session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            Ok(if before {
                context.player.startup_do_before.take()
            } else {
                context.player.startup_do.take()
            })
        })
        .ok_or_else(cancelled_scope_error)??;
    eval_startup_payload_owned(
        session,
        player_id,
        owner,
        code,
        if before { "--doBefore" } else { "--do" },
    )
    .await
}

async fn run_startup_go_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
) -> Result<(), ScriptError> {
    let frame = session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            Ok(context.player.startup_go.take())
        })
        .ok_or_else(cancelled_scope_error)??;
    eval_startup_payload_owned(
        session,
        player_id,
        owner,
        frame.map(|frame| format!("go({frame})")),
        "--go",
    )
    .await
}

async fn dispatch_pending_stream_status_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
) -> Result<(), ScriptError> {
    let events = session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            if !context.player.enable_stream_status_handler {
                return Ok(Vec::new());
            }
            let mut result = Vec::new();
            let mut task_ids: Vec<u32> = context.player.net_manager.tasks.keys().copied().collect();
            task_ids.sort_unstable();
            for task_id in task_ids {
                let last_phase = context.player.stream_status_reported.get(&task_id).copied();
                let Some(task) = context.player.net_manager.get_task(task_id) else {
                    continue;
                };
                let Some(task_state) = context.player.net_manager.get_task_state(Some(task_id))
                else {
                    continue;
                };
                let is_done = task_state.result.is_some();
                if last_phase.is_none() {
                    context.player.stream_status_reported.insert(
                        task_id,
                        crate::player::net_task::StreamStatusPhase::Connecting,
                    );
                    if !is_done {
                        result.push((task.url.clone(), "Connecting", 0, 0, 0));
                    }
                }
                if !is_done
                    && task_state.bytes_loaded > 0
                    && last_phase.map_or(true, |phase| {
                        phase < crate::player::net_task::StreamStatusPhase::Final
                    })
                {
                    context.player.stream_status_reported.insert(
                        task_id,
                        crate::player::net_task::StreamStatusPhase::InProgress,
                    );
                    result.push((
                        task.url.clone(),
                        "InProgress",
                        task_state.bytes_loaded as i32,
                        task_state.bytes_total as i32,
                        0,
                    ));
                }
                if is_done
                    && last_phase.map_or(true, |phase| {
                        phase < crate::player::net_task::StreamStatusPhase::Final
                    })
                {
                    match &task_state.result {
                        Some(Ok(bytes)) => {
                            let len = bytes.len() as i32;
                            result.push((task.url.clone(), "Complete", len, len, 0));
                        }
                        Some(Err(error)) => {
                            result.push((task.url.clone(), "Error", 0, 0, *error));
                        }
                        None => {}
                    }
                    context
                        .player
                        .stream_status_reported
                        .insert(task_id, crate::player::net_task::StreamStatusPhase::Final);
                }
            }
            Ok(result)
        })
        .ok_or_else(cancelled_scope_error)??;

    for (url, state, bytes_so_far, bytes_total, error) in events {
        let args = session
            .borrow_mut()
            .with_player(player_id, |context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(cancelled_scope_error());
                }
                Ok(vec![
                    context.player.alloc_datum(Datum::String(url)),
                    context.player.alloc_datum(Datum::String(state.to_owned())),
                    context.player.alloc_datum(Datum::Int(bytes_so_far)),
                    context.player.alloc_datum(Datum::Int(bytes_total)),
                    context.player.alloc_datum(Datum::Int(error)),
                ])
            })
            .ok_or_else(cancelled_scope_error)??;
        match crate::player::events::player_invoke_global_event_owned(
            session.clone(),
            player_id,
            owner.clone(),
            Symbol::builtin(BuiltInSymbol::StreamStatus),
            args,
        )
        .await
        {
            Ok(_)
            | Err(ScriptError {
                code: ScriptErrorCode::HandlerNotFound,
                ..
            }) => {}
            Err(error) if error.code == ScriptErrorCode::Abort => return Ok(()),
            Err(error) => warn!("streamStatus callback failed: {}", error.message),
        }
    }
    Ok(())
}

/// Run the movie initialization sequence: prepareMovie, beginSprite, behavior init,
/// stepFrame, prepareFrame, startMovie, enterFrame, exitFrame.
/// Shared by `play()` and `transition_to_net_movie`.
async fn run_movie_init_sequence() {
    // Animated GIF members were decoded while the cast was built, before the
    // player could be borrowed; hand them over now so the frame loop can run
    // them. See player::gif.
    crate::player::gif::install_pending();
    let flash_actions = reserve_player_mut(|player| {
        player
            .pre_dispatch_flash_members()
            .map(|_| player.take_flash_host_actions())
    });
    if let Err(error) = flash_actions.and_then(emit_flash_host_actions) {
        warn!("Flash host preparation failed: {}", error.message);
    }
    // The projector's `--do` argument, evaluated before the movie's own code
    // gets a turn. See `DirPlayer::startup_do`.
    run_startup_do().await;

    // prepareMovie
    debug!(">>> Dispatching prepareMovie");
    dispatch_system_event_to_timeouts(BuiltInSymbol::PrepareMovie, &vec![]).await;

    if let Err(err) =
        player_invoke_global_event(Symbol::builtin(BuiltInSymbol::PrepareMovie), &vec![]).await
    {
        web_sys::console::error_1(&format!("prepareMovie FAILED: {}", err.message).into());
        if err.code != ScriptErrorCode::Abort {
            reserve_player_mut(|player| player.on_script_error(&err));
        }
        return;
    }
    debug!(">>> prepareMovie completed successfully");

    // Log bPreloadCasts state after prepareMovie
    reserve_player_ref(|player| {
        let val = player
            .globals
            .get(&Symbol::builtin(BuiltInSymbol::BPreloadCasts));
        let desc = match val {
            Some(r) => format!("{}", player.get_datum(r).type_str()),
            None => "NOT SET".to_string(),
        };
        debug!(">>> bPreloadCasts after prepareMovie: {}", desc);
    });

    player_wait_available().await;

    // Dispatch streamStatus for any resources already loaded by the time prepareMovie
    // enables the handler (movie file, external casts, etc.)
    stream_status::dispatch_pending_stream_status().await;

    // Initialize sprites
    let session_handle = retained_session_handle()
        .expect("movie initialization requires the owning runtime session");
    let player_id = active_player_id() as u32;
    let owner = session_handle
        .borrow_mut()
        .with_player(player_id, |context| context.player.owner.clone())
        .expect("movie initialization player disappeared");
    session_handle
        .borrow_mut()
        .with_player(player_id, |mut context| {
            context.player.movie.frame_script_instance = None;
            context.player.begin_all_sprites(context.symbols);
            context
                .player
                .movie
                .score
                .apply_tween_modifiers(context.player.movie.current_frame);
        })
        .expect("movie initialization player disappeared");

    player_wait_available().await;

    // Collect behaviors that need initialization
    let behaviors_to_init: Vec<(ScriptInstanceRef, u32)> = reserve_player_mut(|player| {
        let mut behaviors = Vec::new();
        for channel_number in player.active_stage_behavior_channels() {
            let Some((sprite_num, fallback)) =
                player
                    .movie
                    .score
                    .channels
                    .get(channel_number)
                    .map(|channel| {
                        (
                            channel.sprite.number as u32,
                            channel.sprite.script_instance_list.clone(),
                        )
                    })
            else {
                continue;
            };

            for behavior_ref in
                player.get_sprite_script_instance_ids(sprite_num as i16, fallback.as_slice())
            {
                if player
                    .allocator
                    .get_script_instance_entry(behavior_ref.id())
                    .is_some_and(|entry| !entry.script_instance.begin_sprite_called)
                {
                    behaviors.push((behavior_ref, sprite_num));
                }
            }
        }
        behaviors
    });

    for (behavior_ref, sprite_num) in &behaviors_to_init {
        if let Err(err) = Score::initialize_behavior_defaults_async(
            session_handle.clone(),
            player_id,
            owner.clone(),
            behavior_ref.clone(),
            *sprite_num,
        )
        .await
        {
            log::warn!("Failed to initialize behavior defaults: {}", err.message);
        }
    }

    player_wait_available().await;

    reserve_player_mut(|player| {
        player.is_in_frame_update = true;
    });

    let begin_sprite_nums =
        player_dispatch_event_beginsprite(Symbol::builtin(BuiltInSymbol::BeginSprite), &vec![])
            .await;

    player_wait_available().await;

    reserve_player_mut(|player| {
        for sprite_list in begin_sprite_nums.iter() {
            for (score_source, sprite_num) in sprite_list.iter() {
                if let Some(sprite) =
                    get_score_sprite_mut(&mut player.movie, score_source, *sprite_num as i16)
                {
                    for script_ref in &sprite.script_instance_list {
                        if let Some(entry) = player
                            .allocator
                            .get_script_instance_entry_mut(script_ref.id())
                        {
                            entry.script_instance.begin_sprite_called = true;
                        }
                    }
                }
            }
        }
    });

    // Dispatch beginSprite to remaining behaviors not handled above
    let remaining_behaviors: Vec<ScriptInstanceRef> = reserve_player_mut(|player| {
        behaviors_to_init
            .iter()
            .filter(|(behavior_ref, _)| {
                player
                    .allocator
                    .get_script_instance_entry(behavior_ref.id())
                    .map_or(false, |entry| !entry.script_instance.begin_sprite_called)
            })
            .map(|(behavior_ref, _)| behavior_ref.clone())
            .collect()
    });

    for behavior_ref in &remaining_behaviors {
        let receivers = vec![behavior_ref.clone()];
        let _ = player_invoke_targeted_event(
            Symbol::builtin(BuiltInSymbol::BeginSprite),
            &vec![],
            Some(&receivers),
        )
        .await;
    }

    if !remaining_behaviors.is_empty() {
        reserve_player_mut(|player| {
            for behavior_ref in &remaining_behaviors {
                if let Some(entry) = player
                    .allocator
                    .get_script_instance_entry_mut(behavior_ref.id())
                {
                    entry.script_instance.begin_sprite_called = true;
                }
            }
        });
    }

    player_wait_available().await;

    // stepFrame to actorList — gate on in_step_frame to prevent re-entry.
    let step_frame_entered = reserve_player_mut(|player| {
        if player.in_step_frame {
            return true;
        }
        player.in_step_frame = true;
        false
    });
    if !step_frame_entered {
        let (actor_list_snapshot, mut active_actor_ids, mut actor_list_generation) =
            reserve_player_ref(|player| player.actor_list_stepframe_snapshot());

        for (idx, actor_ref) in actor_list_snapshot.iter().enumerate() {
            let still_active = active_actor_ids.contains(&actor_ref.unwrap());

            if still_active {
                let result = call_datum_handler_active(
                    &actor_ref,
                    Symbol::builtin(BuiltInSymbol::StepFrame),
                    &vec![],
                )
                .await;

                if let crate::player::handlers::datum_handlers::DatumDispatch::Pending {
                    request,
                    reason,
                } = result
                {
                    retain_datum_pending(request, reason);
                    return;
                }
                if let crate::player::handlers::datum_handlers::DatumDispatch::Sync(Err(err)) =
                    result
                {
                    if err.code == ScriptErrorCode::Abort {
                        reserve_player_mut(|player| {
                            player.is_in_frame_update = false;
                            player.in_step_frame = false;
                        });
                        return;
                    }
                    error!("⚠ stepFrame[{}] error: {}", idx, err.message);
                    reserve_player_mut(|player| {
                        player.on_script_error(&err);
                        player.is_in_frame_update = false;
                        player.in_step_frame = false;
                    });
                    return;
                }

                let refreshed_active_ids = reserve_player_ref(|player| {
                    if player.actor_list_generation != actor_list_generation {
                        Some(player.actor_list_active_ids())
                    } else {
                        None
                    }
                });

                if let Some((next_active_actor_ids, next_actor_list_generation)) =
                    refreshed_active_ids
                {
                    active_actor_ids = next_active_actor_ids;
                    actor_list_generation = next_actor_list_generation;
                }
            }
        }
        reserve_player_mut(|player| {
            player.in_step_frame = false;
        });
    }

    reserve_player_mut(|player| {
        player.in_prepare_frame = true;
        player.drain_allocator_reclaims();
    });

    dispatch_system_event_to_timeouts(BuiltInSymbol::PrepareFrame, &vec![]).await;
    let _ = dispatch_event_to_all_behaviors(Symbol::builtin(BuiltInSymbol::PrepareFrame), &vec![])
        .await;

    let now_ms = js_sys::Date::now().max(0.0);
    // Prepare timer callbacks under the owner-bound context, then release the
    // borrow before invoking handlers.  This preserves the old ordering where
    // #timeMS runs before animation/particle advancement.
    let timer_callbacks = match session_handle
        .borrow_mut()
        .with_player(player_id, |mut context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            crate::player::events::prepare_w3d_timer_events(&mut context, now_ms)
        }) {
        Some(Ok(callbacks)) => callbacks,
        _ => return,
    };
    for callback in timer_callbacks {
        if !owner.same_identity(&callback.owner) || !owner.is_arena_live() {
            return;
        }
        match callback.receiver {
            crate::player::events::W3dCallbackReceiver::Instance(instance) => {
                let handler = match session_handle
                    .borrow_mut()
                    .with_player(player_id, |context| {
                        crate::player::handlers::datum_handlers::script_instance::ScriptInstanceUtils::get_script_instance_handler(
                            callback.handler_name.clone(), &instance, context.player)
                    }) {
                    Some(Ok(handler)) => handler,
                    _ => return,
                };
                if let Some(handler) = handler {
                    if let Err(error) = crate::player::eval::invoke_script_callback_owned(
                        session_handle.clone(),
                        player_id,
                        owner.clone(),
                        Some(instance),
                        handler,
                        callback.args,
                        false,
                    )
                    .await
                    {
                        if error.code != ScriptErrorCode::Abort {
                            log::error!("W3D callback failed: {}", error.message);
                        }
                        return;
                    }
                }
            }
            crate::player::events::W3dCallbackReceiver::Static { .. }
            | crate::player::events::W3dCallbackReceiver::Global => {
                if let Err(error) = crate::player::events::player_invoke_global_event_owned(
                    session_handle.clone(),
                    player_id,
                    owner.clone(),
                    callback.handler_name,
                    callback.args,
                )
                .await
                {
                    if error.code != ScriptErrorCode::Abort {
                        log::error!("W3D global callback failed: {}", error.message);
                    }
                    return;
                }
            }
        }
    }

    // Advance animation and particles, then prepare collision callbacks from
    // the same short context.  Each callback is awaited only after the borrow
    // has ended, so a callback can safely mutate the owner or suspend again.
    let (animation_dt, particle_dt) = match session_handle
        .borrow_mut()
        .w3d_deltas(player_id, &owner, now_ms)
    {
        Ok(deltas) => deltas,
        Err(_) => return,
    };
    let collision_callbacks =
        match session_handle
            .borrow_mut()
            .with_player(player_id, |mut context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(cancelled_scope_error());
                }
                crate::player::events::tick_w3d_animations(&mut context, animation_dt)?;
                crate::player::events::tick_w3d_particles(&mut context, particle_dt)?;
                let dirty = crate::player::events::W3dDirtyTransformInput::new(
                    owner.clone(),
                    context.player.w3d_dirty_transform_ids.clone(),
                );
                let mut callbacks =
                    crate::player::events::prepare_w3d_collision_callbacks(&mut context, &dirty)?;
                callbacks.extend(crate::player::events::prepare_physx_collision_callbacks(
                    &mut context,
                )?);
                context.player.w3d_dirty_transform_ids.clear();
                Ok(callbacks)
            }) {
            Some(Ok(callbacks)) => callbacks,
            _ => return,
        };
    for callback in collision_callbacks {
        if !owner.same_identity(&callback.owner) || !owner.is_arena_live() {
            return;
        }
        match callback.receiver {
            crate::player::events::W3dCallbackReceiver::Instance(instance) => {
                let handler = match session_handle
                    .borrow_mut()
                    .with_player(player_id, |context| {
                        crate::player::handlers::datum_handlers::script_instance::ScriptInstanceUtils::get_script_instance_handler(
                            callback.handler_name.clone(), &instance, context.player)
                    }) {
                    Some(Ok(handler)) => handler,
                    _ => return,
                };
                if let Some(handler) = handler {
                    if let Err(error) = crate::player::eval::invoke_script_callback_owned(
                        session_handle.clone(),
                        player_id,
                        owner.clone(),
                        Some(instance),
                        handler,
                        callback.args,
                        false,
                    )
                    .await
                    {
                        if error.code != ScriptErrorCode::Abort {
                            log::error!("W3D callback failed: {}", error.message);
                        }
                        return;
                    }
                }
            }
            crate::player::events::W3dCallbackReceiver::Static { .. }
            | crate::player::events::W3dCallbackReceiver::Global => {
                if let Err(error) = crate::player::events::player_invoke_global_event_owned(
                    session_handle.clone(),
                    player_id,
                    owner.clone(),
                    callback.handler_name,
                    callback.args,
                )
                .await
                {
                    if error.code != ScriptErrorCode::Abort {
                        log::error!("W3D global callback failed: {}", error.message);
                    }
                    return;
                }
            }
        }
    }

    reserve_player_mut(|player| {
        player.in_prepare_frame = false;
    });

    // startMovie
    dispatch_system_event_to_timeouts(BuiltInSymbol::StartMovie, &vec![]).await;

    if let Err(err) =
        player_invoke_global_event(Symbol::builtin(BuiltInSymbol::StartMovie), &vec![]).await
    {
        if err.code != ScriptErrorCode::Abort {
            reserve_player_mut(|player| player.on_script_error(&err));
        }
        return;
    }

    player_wait_available().await;

    // enterFrame
    reserve_player_mut(|player| {
        player.in_enter_frame = true;
    });

    let _ =
        dispatch_event_to_all_behaviors(Symbol::builtin(BuiltInSymbol::EnterFrame), &vec![]).await;

    reserve_player_mut(|player| {
        player.in_enter_frame = false;
    });

    player_wait_available().await;

    // exitFrame
    dispatch_system_event_to_timeouts(BuiltInSymbol::ExitFrame, &vec![]).await;
    let _ =
        dispatch_event_to_all_behaviors(Symbol::builtin(BuiltInSymbol::ExitFrame), &vec![]).await;

    player_wait_available().await;

    reserve_player_mut(|player| {
        player.is_in_frame_update = false;
    });

    // `--go N`, last: the movie's own prepareMovie / startMovie / exitFrame have
    // had their turn, so a frame the curator picked is not immediately
    // overwritten by the movie's own opening navigation.
    run_startup_go().await;
}

/// Clears the frame-update flag even if an owned initialization future is
/// dropped while suspended. Drop deliberately uses a non-blocking borrow: a
/// replacement owner must never be mutated by a stale future's cleanup.
struct FrameUpdateFlagGuard {
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
    armed: bool,
}

impl FrameUpdateFlagGuard {
    fn new(session: RuntimeSessionHandle, player_id: u32, owner: OwnerToken) -> Self {
        Self {
            session,
            player_id,
            owner,
            armed: true,
        }
    }

    fn clear_now(&mut self) -> Result<(), ScriptError> {
        let cleared = self
            .session
            .borrow_mut()
            .with_player(self.player_id, |context| {
                if !self.owner.same_identity(&context.player.owner) || !self.owner.is_arena_live() {
                    return false;
                }
                context.player.is_in_frame_update = false;
                true
            })
            .unwrap_or(false);
        self.armed = false;
        if cleared {
            Ok(())
        } else {
            Err(cancelled_scope_error())
        }
    }
}

impl Drop for FrameUpdateFlagGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // Every owned phase releases its `RefMut` before awaiting. A failed
        // borrow here would otherwise silently leave the player stuck in the
        // frame-update state, so treat the ownership boundary as an invariant
        // and perform the cleanup synchronously.
        let mut session = self.session.borrow_mut();
        let _ = session.with_player(self.player_id, |context| {
            if self.owner.same_identity(&context.player.owner) && self.owner.is_arena_live() {
                context.player.is_in_frame_update = false;
            }
        });
    }
}

/// Owner-bound movie initialization entrypoint with an explicit frame-clock
/// sample captured by the caller before entering the async phase sequence.
/// Every phase reacquires the session only for its synchronous mutation and
/// carries the captured owner through event/default-property suspension.
pub async fn run_movie_init_owned_at(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: crate::player::ownership::OwnerToken,
    frame_now_ms: f64,
) -> Result<(), ScriptError> {
    let valid = session
        .borrow_mut()
        .with_player(player_id, |context| {
            owner.same_identity(&context.player.owner) && owner.is_arena_live()
        })
        .unwrap_or(false);
    if !valid {
        return Err(cancelled_scope_error());
    }
    crate::player::gif::install_pending();
    let flash_actions = session
        .borrow_mut()
        .with_player(player_id, |context| {
            context
                .player
                .pre_dispatch_flash_members()
                .map(|_| context.player.take_flash_host_actions())
        })
        .ok_or_else(cancelled_scope_error)??;
    let flash_actions = bind_flash_host_actions(flash_actions, session.clone(), player_id);
    emit_flash_host_actions(flash_actions)?;
    run_startup_do_owned(session.clone(), player_id, owner.clone(), false).await?;
    // prepareMovie runs against the old score state. Mount callers set
    // pending_movie_init only after the new cast is installed, so this event
    // must precede beginSprite/default-property work.
    dispatch_system_event_to_timeouts_owned(
        session.clone(),
        player_id,
        owner.clone(),
        BuiltInSymbol::PrepareMovie,
        Vec::new(),
    )
    .await?;
    crate::player::events::player_invoke_global_event_owned(
        session.clone(),
        player_id,
        owner.clone(),
        Symbol::builtin(BuiltInSymbol::PrepareMovie),
        Vec::new(),
    )
    .await?;
    dispatch_pending_stream_status_owned(session.clone(), player_id, owner.clone()).await?;
    session
        .borrow_mut()
        .with_player(player_id, |mut context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            context.player.movie.frame_script_instance = None;
            context.player.begin_all_sprites(context.symbols);
            context
                .player
                .movie
                .score
                .apply_tween_modifiers(context.player.movie.current_frame);
            Ok::<(), ScriptError>(())
        })
        .ok_or_else(cancelled_scope_error)??;
    // Defaults and beginSprite belong between prepareMovie and startMovie,
    // matching Director's initialization order.
    let behaviors = session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            let player = &mut *context.player;
            let mut result = Vec::new();
            for channel in player.active_stage_behavior_channels() {
                let Some((sprite_num, fallback)) =
                    player.movie.score.channels.get(channel).map(|channel| {
                        (
                            channel.sprite.number as u32,
                            channel.sprite.script_instance_list.clone(),
                        )
                    })
                else {
                    continue;
                };
                for behavior in
                    player.get_sprite_script_instance_ids(sprite_num as i16, fallback.as_slice())
                {
                    if player
                        .allocator
                        .get_script_instance_entry(behavior.id())
                        .is_some_and(|entry| !entry.script_instance.begin_sprite_called)
                    {
                        result.push((behavior, sprite_num));
                    }
                }
            }
            Ok::<_, ScriptError>(result)
        })
        .ok_or_else(cancelled_scope_error)??;
    for (behavior, sprite_num) in behaviors {
        Score::initialize_behavior_defaults_async(
            session.clone(),
            player_id,
            owner.clone(),
            behavior,
            sprite_num,
        )
        .await?;
    }
    // Preserve Director's ordered frame/movie, stage-sprite, and film-loop
    // dispatch. The owner-bound dispatcher returns only the channels whose
    // callback phase completed; this caller owns the final state mutation.
    let begin_sprite = crate::player::events::player_dispatch_event_beginsprite_owned(
        session.clone(),
        player_id,
        owner.clone(),
        Symbol::builtin(BuiltInSymbol::BeginSprite),
        Vec::new(),
    )
    .await?;
    session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            for (score_source, sprite_num) in &begin_sprite.initialized_channels {
                if let Some(sprite) = get_score_sprite_mut(
                    &mut context.player.movie,
                    score_source,
                    *sprite_num as i16,
                ) {
                    for script_ref in &sprite.script_instance_list {
                        if let Some(entry) = context
                            .player
                            .allocator
                            .get_script_instance_entry_mut(script_ref.id())
                        {
                            entry.script_instance.begin_sprite_called = true;
                        }
                    }
                }
            }
            Ok::<(), ScriptError>(())
        })
        .ok_or_else(cancelled_scope_error)??;

    // Preserve the legacy targeted remainder for active behaviors that had no
    // handler during the ordered dispatcher pass, while keeping the callback
    // owner-bound across the await.
    for behavior_ref in begin_sprite.unhandled_behaviors {
        let handler = session
            .borrow_mut()
            .with_player(player_id, |context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(cancelled_scope_error());
                }
                ScriptInstanceUtils::get_script_instance_handler(
                    Symbol::builtin(BuiltInSymbol::BeginSprite),
                    &behavior_ref,
                    context.player,
                )
            })
            .ok_or_else(cancelled_scope_error)??;
        if let Some(handler) = handler {
            crate::player::eval::invoke_script_callback_owned(
                session.clone(),
                player_id,
                owner.clone(),
                Some(behavior_ref.clone()),
                handler,
                Vec::new(),
                false,
            )
            .await?;
        }
        session
            .borrow_mut()
            .with_player(player_id, |context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(cancelled_scope_error());
                }
                if let Some(entry) = context
                    .player
                    .allocator
                    .get_script_instance_entry_mut(behavior_ref.id())
                {
                    entry.script_instance.begin_sprite_called = true;
                }
                Ok::<(), ScriptError>(())
            })
            .ok_or_else(cancelled_scope_error)??;
    }
    // Initialization keeps the frame-update guard across the split phases:
    // stepFrame/PrepareFrame/W3D run before StartMovie, while EnterFrame and
    // ExitFrame run afterward, matching the legacy sequence.
    session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            context.player.is_in_frame_update = true;
            context
                .player
                .movie
                .score
                .apply_tween_modifiers(context.player.movie.current_frame);
            Ok(())
        })
        .ok_or_else(cancelled_scope_error)??;
    let mut frame_guard = FrameUpdateFlagGuard::new(session.clone(), player_id, owner.clone());
    let frame_result = async {
        crate::player::handlers::movie::execute_movie_async(
            session.clone(),
            crate::player::handlers::movie::MovieAsyncRequest {
                player_id,
                owner: owner.clone(),
                kind: crate::player::handlers::movie::MovieAsyncKind::InitPrepareFrame {
                    now_ms: frame_now_ms,
                },
                args: Vec::new(),
            },
        )
        .await?;
        dispatch_system_event_to_timeouts_owned(
            session.clone(),
            player_id,
            owner.clone(),
            BuiltInSymbol::StartMovie,
            Vec::new(),
        )
        .await?;
        crate::player::events::player_invoke_global_event_owned(
            session.clone(),
            player_id,
            owner.clone(),
            Symbol::builtin(BuiltInSymbol::StartMovie),
            Vec::new(),
        )
        .await?;
        crate::player::handlers::movie::execute_movie_async(
            session.clone(),
            crate::player::handlers::movie::MovieAsyncRequest {
                player_id,
                owner: owner.clone(),
                kind: crate::player::handlers::movie::MovieAsyncKind::InitEnterFrame,
                args: Vec::new(),
            },
        )
        .await?;
        dispatch_system_event_to_timeouts_owned(
            session.clone(),
            player_id,
            owner.clone(),
            BuiltInSymbol::ExitFrame,
            Vec::new(),
        )
        .await?;
        crate::player::handlers::movie::execute_movie_async(
            session.clone(),
            crate::player::handlers::movie::MovieAsyncRequest {
                player_id,
                owner: owner.clone(),
                kind: crate::player::handlers::movie::MovieAsyncKind::InitExitFrame,
                args: Vec::new(),
            },
        )
        .await?;
        Ok::<(), ScriptError>(())
    }
    .await;
    if let Err(error) = frame_result {
        return Err(error);
    }
    frame_guard.clear_now()?;
    run_startup_go_owned(session.clone(), player_id, owner.clone()).await?;
    session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            // Mark initialization only after every startup phase, including
            // startup Go, has completed successfully.  A failed or canceled
            // initializer must be retried by the next owned playback loop.
            context.player.last_initialized_frame = Some(context.player.movie.current_frame);
            context.player.playback_init_count =
                context.player.playback_init_count.saturating_add(1);
            Ok::<(), ScriptError>(())
        })
        .ok_or_else(cancelled_scope_error)??;
    Ok(())
}

/// Compatibility boundary for callers that have not yet been migrated to
/// supply the frame timestamp. The host sample is taken before the owned
/// phase begins; the core initializer itself never samples a clock.
pub async fn run_movie_init_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: crate::player::ownership::OwnerToken,
) -> Result<(), ScriptError> {
    run_movie_init_owned_at(
        session,
        player_id,
        owner,
        crate::player::testing_shared::now_ms().max(0.0),
    )
    .await
}

/// Finish one owner-bound playback loop and decide whether a same-owner replay
/// may be restarted. Cleanup failures are reported once and consume the replay
/// request, so a partially torn-down movie cannot be mounted again.
pub(crate) fn finish_playback_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
    epoch: u64,
    result: Result<(), ScriptError>,
) {
    let result_ok = result.is_ok();
    if let Err(error) = result {
        // Abort is cancellation/control flow. It still consumes a queued
        // replay below, but must not be surfaced as a script failure while
        // the captured owner remains live.
        if error.code != ScriptErrorCode::Abort {
            let _ = session.borrow_mut().with_player(player_id, |context| {
                if owner.same_identity(&context.player.owner) && owner.is_arena_live() {
                    context
                        .player
                        .on_script_error_with_symbols(&error, Some(context.symbols));
                }
            });
        }
    }
    let finished = session
        .borrow_mut()
        .finish_playback_loop(player_id, &owner, epoch);
    let replay = if result_ok {
        session
            .borrow_mut()
            .take_playback_replay_request(player_id, &owner, epoch)
    } else {
        session
            .borrow_mut()
            .discard_playback_replay_request(player_id, &owner, epoch);
        false
    };
    // A loop that ends naturally owns the playing flag; a replacement owner
    // must never be touched by this finalizer.
    if finished {
        let _ = session.borrow_mut().with_player(player_id, |context| {
            if owner.same_identity(&context.player.owner) {
                context.player.is_playing = false;
            }
        });
    }
    if replay && result_ok {
        // A same-owner play issued while StopMovie/endSprite cleanup was still
        // running resumes only after the old loop has retired.
        let _ = start_playback_owned(session, player_id, owner);
    }
}

/// Start the canonical playback loop for one captured session owner.
///
/// This is the only production entrypoint that creates a movie frame loop for
/// a handle.  It deliberately uses `spawn_local` directly because the task is
/// already bound to `(session, player_id, owner)`; it never consults
/// `ACTIVE_PLAYER_ID`, `PLAYER_OPT`, or `spawn_player_local`.
pub(crate) fn start_playback_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
) -> Result<(), ScriptError> {
    let Some((epoch, cancel_rx)) = session
        .borrow_mut()
        .begin_playback_loop(player_id, &owner)?
    else {
        return Ok(());
    };
    session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            context.player.is_playing = true;
            context.player.is_script_paused = false;
            Ok::<(), ScriptError>(())
        })
        .ok_or_else(cancelled_scope_error)??;

    spawn_local(async move {
        let result =
            run_playback_loop_owned(session.clone(), player_id, owner.clone(), cancel_rx, epoch)
                .await;
        finish_playback_owned(session, player_id, owner, epoch, result);
    });
    Ok(())
}

/// Stop only the loop belonging to the captured owner.  Sending on the
/// session-held channel wakes pacing, Flash waits, and handler-gap waits; the
/// owner check prevents a stale stop from cancelling a replacement.
pub(crate) fn stop_playback_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: &OwnerToken,
) -> Result<(), ScriptError> {
    let mut runtime = session.borrow_mut();
    let valid = runtime
        .with_player(player_id, |context| {
            owner.same_identity(&context.player.owner) && owner.is_arena_live()
        })
        .unwrap_or(false);
    if !valid {
        return Err(cancelled_scope_error());
    }
    runtime.cancel_playback_loop(player_id, owner, true);
    runtime
        .with_player(player_id, |context| {
            context.player.stop();
            context.player.playback_stop_count =
                context.player.playback_stop_count.saturating_add(1);
        })
        .ok_or_else(cancelled_scope_error)?;
    Ok(())
}

/// Select one owned playback operation against the loop's cancellation
/// channel.  Dropping the losing operation is intentional: the owned frame
/// and callback guards release their session state on cancellation.
async fn await_playback_or_cancel<'a, T, F>(cancel_rx: &'a Receiver<()>, operation: F) -> Option<T>
where
    F: Future<Output = T> + 'a,
{
    match select(cancel_rx.recv().boxed_local(), operation.boxed_local()).await {
        Either::Left((_cancelled, _operation)) => None,
        Either::Right((result, _cancelled)) => Some(result),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PlaybackTransitionStopPhase {
    NotStarted,
    InProgress,
    Complete,
}

struct PlaybackTransitionAwait<T> {
    result: Option<T>,
    cancelled: bool,
}

/// A transition may be cancelled until it enters StopMovie. Once lifecycle
/// teardown starts, retain the future until that phase has finished so a
/// cancellation cannot replay already-attempted StopMovie/endSprite callbacks.
async fn await_playback_transition_or_cancel<'a, T, F>(
    cancel_rx: &'a Receiver<()>,
    operation: F,
    stop_phase: Rc<Cell<PlaybackTransitionStopPhase>>,
) -> PlaybackTransitionAwait<T>
where
    F: Future<Output = T> + 'a,
{
    match select(cancel_rx.recv().boxed_local(), operation.boxed_local()).await {
        Either::Left((_cancelled, operation)) => {
            if stop_phase.get() == PlaybackTransitionStopPhase::NotStarted {
                PlaybackTransitionAwait {
                    result: None,
                    cancelled: true,
                }
            } else {
                PlaybackTransitionAwait {
                    result: Some(operation.await),
                    cancelled: true,
                }
            }
        }
        Either::Right((result, _cancelled)) => PlaybackTransitionAwait {
            result: Some(result),
            cancelled: false,
        },
    }
}

fn flash_loading_owned(
    session: &RuntimeSessionHandle,
    player_id: u32,
    owner: &OwnerToken,
) -> Result<bool, ScriptError> {
    session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.is_arena_live() || !owner.same_identity(&context.player.owner) {
                return Err(cancelled_scope_error());
            }
            Ok(context.player.flash_scripted_access_pending.get())
        })
        .ok_or_else(cancelled_scope_error)?
}

fn tick_sound_manager_owned(
    session: &RuntimeSessionHandle,
    player_id: u32,
    owner: &OwnerToken,
    delta: f64,
) -> Result<(), ScriptError> {
    session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            // Move the manager out for the duration of the update.  This
            // gives SoundManager its required mutable player reference without
            // manufacturing overlapping `&mut SoundManager`/`&mut DirPlayer`
            // aliases through a raw pointer.
            let mut sound_manager =
                std::mem::replace(&mut context.player.sound_manager, SoundManager::empty());
            let result = sound_manager.update(delta, context.player);
            context.player.sound_manager = sound_manager;
            result
        })
        .ok_or_else(cancelled_scope_error)?
}

async fn cleanup_after_playback_cancel(
    session: &RuntimeSessionHandle,
    player_id: u32,
    owner: &OwnerToken,
    epoch: u64,
) -> Result<(), ScriptError> {
    let stop_sequence = session
        .borrow_mut()
        .take_playback_stop_cleanup(player_id, owner, epoch);
    if stop_sequence {
        // Stop is a lifecycle operation, unlike reset/remove cancellation:
        // finish StopMovie/endSprite against the old owner before its replay
        // can claim a new loop. Every await remains owner checked.
        stop_movie_sequence_owned(session.clone(), player_id, owner.clone()).await?;
    }
    Ok(())
}

/// Restore transition bookkeeping when cancellation drops an owned movie
/// restart/network operation. The operation may have set these flags before
/// its next await, so its ordinary error path cannot run after `select!`
/// drops it. The owner fence keeps cancellation from touching a replacement
/// runtime in the same player slot.
async fn restore_playback_transition_flags_owned(
    session: &RuntimeSessionHandle,
    player_id: u32,
    owner: &OwnerToken,
    is_in_transition: bool,
    is_dispatching_events: bool,
) {
    let _ = session.borrow_mut().with_player(player_id, |context| {
        if owner.same_identity(&context.player.owner) && owner.is_arena_live() {
            context.player.is_in_transition = is_in_transition;
            context.player.is_dispatching_events = is_dispatching_events;
        }
    });
}

async fn run_playback_loop_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
    cancel_rx: Receiver<()>,
    epoch: u64,
) -> Result<(), ScriptError> {
    let mut next_deadline_ms = bench_now_ms();
    let needs_init = session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            Ok(context.player.last_initialized_frame.is_none())
        })
        .ok_or_else(cancelled_scope_error)??;
    if needs_init {
        let initial_now_ms = bench_now_ms();
        match await_playback_or_cancel(
            &cancel_rx,
            run_movie_init_owned_at(session.clone(), player_id, owner.clone(), initial_now_ms),
        )
        .await
        {
            Some(Ok(())) => {}
            Some(Err(error)) => return Err(error),
            None => {
                cleanup_after_playback_cancel(&session, player_id, &owner, epoch).await?;
                return Ok(());
            }
        }
    }

    loop {
        let state = session
            .borrow_mut()
            .with_player(player_id, |context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(cancelled_scope_error());
                }
                Ok((
                    context.player.is_playing,
                    context.player.is_script_paused,
                    context.player.pending_restart,
                    context.player.pending_movie_init && context.player.handler_stack_depth == 0,
                    context
                        .player
                        .pending_goto_net_movie
                        .as_ref()
                        .and_then(|(task_id, target)| {
                            context
                                .player
                                .net_manager
                                .is_task_done(Some(*task_id))
                                .then(|| (*task_id, target.clone()))
                        }),
                    context.player.current_frame_tempo,
                ))
            })
            .ok_or_else(cancelled_scope_error)??;
        if !state.0 {
            stop_movie_sequence_owned(session.clone(), player_id, owner.clone()).await?;
            return Ok(());
        }

        if state.2 {
            let transition_flags = session
                .borrow_mut()
                .with_player(player_id, |context| {
                    if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                        return Err(cancelled_scope_error());
                    }
                    Ok((
                        context.player.is_in_transition,
                        context.player.is_dispatching_events,
                    ))
                })
                .ok_or_else(cancelled_scope_error)??;
            let stop_phase = Rc::new(Cell::new(PlaybackTransitionStopPhase::NotStarted));
            let outcome = await_playback_transition_or_cancel(
                &cancel_rx,
                restart_current_movie_owned(
                    session.clone(),
                    player_id,
                    owner.clone(),
                    stop_phase.clone(),
                ),
                stop_phase,
            )
            .await;
            match (outcome.result, outcome.cancelled) {
                (Some(result), false) => result?,
                (None, true) => {
                    let cleanup_result =
                        cleanup_after_playback_cancel(&session, player_id, &owner, epoch).await;
                    restore_playback_transition_flags_owned(
                        &session,
                        player_id,
                        &owner,
                        transition_flags.0,
                        transition_flags.1,
                    )
                    .await;
                    cleanup_result?;
                    return Ok(());
                }
                (Some(result), true) => {
                    if result.is_ok() {
                        let cleanup_result =
                            cleanup_after_playback_cancel(&session, player_id, &owner, epoch).await;
                        restore_playback_transition_flags_owned(
                            &session,
                            player_id,
                            &owner,
                            transition_flags.0,
                            transition_flags.1,
                        )
                        .await;
                        cleanup_result?;
                    } else {
                        restore_playback_transition_flags_owned(
                            &session,
                            player_id,
                            &owner,
                            transition_flags.0,
                            transition_flags.1,
                        )
                        .await;
                    }
                    result?;
                    return Ok(());
                }
                (None, false) => unreachable!("transition await completed without result"),
            }
            continue;
        }

        if state.3 {
            session
                .borrow_mut()
                .with_player(player_id, |context| {
                    if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                        return Err(cancelled_scope_error());
                    }
                    context.player.pending_movie_init = false;
                    context.player.is_in_transition = false;
                    context.player.retired_cast_libs.clear();
                    Ok::<(), ScriptError>(())
                })
                .ok_or_else(cancelled_scope_error)??;
            let now_ms = bench_now_ms();
            match await_playback_or_cancel(
                &cancel_rx,
                run_movie_init_owned_at(session.clone(), player_id, owner.clone(), now_ms),
            )
            .await
            {
                Some(result) => result?,
                None => {
                    cleanup_after_playback_cancel(&session, player_id, &owner, epoch).await?;
                    return Ok(());
                }
            }
            continue;
        }

        if let Some((task_id, target)) = state.4 {
            let transition_flags = session
                .borrow_mut()
                .with_player(player_id, |context| {
                    if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                        return Err(cancelled_scope_error());
                    }
                    Ok((
                        context.player.is_in_transition,
                        context.player.is_dispatching_events,
                    ))
                })
                .ok_or_else(cancelled_scope_error)??;
            let stop_phase = Rc::new(Cell::new(PlaybackTransitionStopPhase::NotStarted));
            let outcome = await_playback_transition_or_cancel(
                &cancel_rx,
                transition_to_net_movie_owned(
                    session.clone(),
                    player_id,
                    owner.clone(),
                    task_id,
                    target,
                    stop_phase.clone(),
                ),
                stop_phase,
            )
            .await;
            match (outcome.result, outcome.cancelled) {
                (Some(result), false) => result?,
                (None, true) => {
                    let cleanup_result =
                        cleanup_after_playback_cancel(&session, player_id, &owner, epoch).await;
                    restore_playback_transition_flags_owned(
                        &session,
                        player_id,
                        &owner,
                        transition_flags.0,
                        transition_flags.1,
                    )
                    .await;
                    cleanup_result?;
                    return Ok(());
                }
                (Some(result), true) => {
                    if result.is_ok() {
                        let cleanup_result =
                            cleanup_after_playback_cancel(&session, player_id, &owner, epoch).await;
                        restore_playback_transition_flags_owned(
                            &session,
                            player_id,
                            &owner,
                            transition_flags.0,
                            transition_flags.1,
                        )
                        .await;
                        cleanup_result?;
                    } else {
                        restore_playback_transition_flags_owned(
                            &session,
                            player_id,
                            &owner,
                            transition_flags.0,
                            transition_flags.1,
                        )
                        .await;
                    }
                    result?;
                    return Ok(());
                }
                (None, false) => unreachable!("transition await completed without result"),
            }
            continue;
        }

        if flash_loading_owned(&session, player_id, &owner)? {
            for _ in 0..150 {
                if !flash_loading_owned(&session, player_id, &owner)? {
                    break;
                }
                let wait = timeout(Duration::from_millis(100), future::pending::<()>()).map(|_| ());
                if await_playback_or_cancel(&cancel_rx, wait).await.is_none() {
                    cleanup_after_playback_cancel(&session, player_id, &owner, epoch).await?;
                    return Ok(());
                }
                tick_sound_manager_owned(&session, player_id, &owner, 0.1)?;
            }
        }

        let frame_now_ms = bench_now_ms();
        let frame = await_playback_or_cancel(
            &cancel_rx,
            run_single_frame_owned_at(session.clone(), player_id, owner.clone(), frame_now_ms),
        )
        .await;
        let (playing, paused) = match frame {
            Some(result) => result?,
            None => {
                cleanup_after_playback_cancel(&session, player_id, &owner, epoch).await?;
                return Ok(());
            }
        };
        if !playing {
            continue;
        }
        if !paused {
            match await_playback_or_cancel(
                &cancel_rx,
                crate::player::events::player_invoke_global_event_owned(
                    session.clone(),
                    player_id,
                    owner.clone(),
                    Symbol::builtin(BuiltInSymbol::Idle),
                    Vec::new(),
                ),
            )
            .await
            {
                Some(result) => {
                    result?;
                }
                None => {
                    cleanup_after_playback_cancel(&session, player_id, &owner, epoch).await?;
                    return Ok(());
                }
            }
        }

        let tempo = session
            .borrow_mut()
            .with_player(player_id, |context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(cancelled_scope_error());
                }
                Ok(context.player.current_frame_tempo)
            })
            .ok_or_else(cancelled_scope_error)??;
        let delta = if tempo == 0 {
            1.0 / 30.0
        } else {
            1.0 / tempo as f64
        };
        tick_sound_manager_owned(&session, player_id, &owner, delta)?;

        let cue_events = session
            .borrow_mut()
            .with_player(player_id, |context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(cancelled_scope_error());
                }
                Ok(std::mem::take(&mut context.player.pending_cue_events))
            })
            .ok_or_else(cancelled_scope_error)??;
        for (channel_num, cue_number, cue_name) in cue_events {
            let (args, cue_symbol) = session
                .borrow_mut()
                .with_player(player_id, |context| {
                    if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                        return Err(cancelled_scope_error());
                    }
                    let channel = context.symbols.intern(&format!("sound{}", channel_num));
                    let channel_ref = context.player.alloc_datum(Datum::Symbol(channel));
                    let number_ref = context.player.alloc_datum(Datum::Int(cue_number));
                    let name_ref = context.player.alloc_datum(Datum::String(cue_name));
                    Ok((
                        vec![channel_ref, number_ref, name_ref],
                        context.symbols.intern("cuePassed"),
                    ))
                })
                .ok_or_else(cancelled_scope_error)??;
            match await_playback_or_cancel(
                &cancel_rx,
                crate::player::events::player_invoke_global_event_owned(
                    session.clone(),
                    player_id,
                    owner.clone(),
                    cue_symbol,
                    args,
                ),
            )
            .await
            {
                Some(result) => {
                    result?;
                }
                None => {
                    cleanup_after_playback_cancel(&session, player_id, &owner, epoch).await?;
                    return Ok(());
                }
            }
        }

        let target_delay_ms = if tempo == 0 {
            1000.0 / 30.0
        } else {
            1000.0 / tempo as f64
        };
        next_deadline_ms += target_delay_ms;
        let now_ms = bench_now_ms();
        if next_deadline_ms < now_ms {
            next_deadline_ms = now_ms;
        }
        let wait = wait_until_deadline(next_deadline_ms);
        if await_playback_or_cancel(&cancel_rx, wait).await.is_none() {
            cleanup_after_playback_cancel(&session, player_id, &owner, epoch).await?;
            return Ok(());
        }
        match await_playback_or_cancel(
            &cancel_rx,
            wait_for_handler_gap_owned(session.clone(), player_id, owner.clone()),
        )
        .await
        {
            Some(result) => {
                result?;
            }
            None => {
                cleanup_after_playback_cancel(&session, player_id, &owner, epoch).await?;
                return Ok(());
            }
        }
    }
}

/// Advance one owner-bound frame through the canonical movie executor. The
/// executor owns frame callbacks and host waits; this function only performs
/// the short state transition after that work has completed.
pub async fn run_single_frame_owned_at(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: crate::player::ownership::OwnerToken,
    frame_now_ms: f64,
) -> Result<(bool, bool), ScriptError> {
    let (playing, paused, pending_init, stack_depth) = session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            Ok::<_, ScriptError>((
                context.player.is_playing,
                context.player.is_script_paused,
                context.player.pending_movie_init,
                context.player.handler_stack_depth,
            ))
        })
        .ok_or_else(cancelled_scope_error)??;
    if !playing {
        return Ok((false, paused));
    }
    if pending_init && stack_depth == 0 {
        session
            .borrow_mut()
            .with_player(player_id, |context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(cancelled_scope_error());
                }
                context.player.pending_movie_init = false;
                Ok::<(), ScriptError>(())
            })
            .ok_or_else(cancelled_scope_error)??;
        run_movie_init_owned_at(session.clone(), player_id, owner.clone(), frame_now_ms).await?;
        session
            .borrow_mut()
            .with_player(player_id, |context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(cancelled_scope_error());
                }
                context.player.is_in_transition = false;
                context.player.retired_cast_libs.clear();
                Ok::<(), ScriptError>(())
            })
            .ok_or_else(cancelled_scope_error)??;
        return Ok((playing, paused));
    }
    if paused {
        return Ok((playing, paused));
    }
    // A puppet transition owns the playhead until its deadline. Keep the
    // stage dirty for the renderer, but do not advance scripts or the frame.
    let transition_hold = session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            Ok(context.player.transition_hold_active())
        })
        .ok_or_else(cancelled_scope_error)??;
    if transition_hold {
        session
            .borrow_mut()
            .with_player(player_id, |context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(cancelled_scope_error());
                }
                context.player.stage_dirty = true;
                Ok::<(), ScriptError>(())
            })
            .ok_or_else(cancelled_scope_error)??;
        return Ok((playing, paused));
    }
    // Global timeout and stream-status work happen once per frame, before
    // frame script dispatch, matching the recovered frame-loop ordering.
    crate::player::fire_pending_timeouts_owned_at(
        session.clone(),
        player_id,
        owner.clone(),
        frame_now_ms,
    )
    .await?;
    crate::player::dispatch_pending_stream_status_owned(session.clone(), player_id, owner.clone())
        .await?;
    let unpuppet_changed = session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            let frame = context.player.movie.current_frame;
            Ok(context
                .player
                .movie
                .score
                .process_pending_unpuppet_reverts(frame))
        })
        .ok_or_else(cancelled_scope_error)??;
    if unpuppet_changed {
        session
            .borrow_mut()
            .with_player(player_id, |context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(cancelled_scope_error());
                }
                context.player.movie.score.invalidate_render_channel_cache();
                context.player.stage_dirty = true;
                Ok::<(), ScriptError>(())
            })
            .ok_or_else(cancelled_scope_error)??;
    }
    let skip_frame = session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            Ok(context.player.command_handler_yielding || context.player.in_mouse_command)
        })
        .ok_or_else(cancelled_scope_error)??;
    if skip_frame {
        return Ok((playing, paused));
    }
    crate::player::handlers::movie::execute_movie_async(
        session.clone(),
        crate::player::handlers::movie::MovieAsyncRequest {
            player_id,
            owner: owner.clone(),
            kind: crate::player::handlers::movie::MovieAsyncKind::FrameUpdate {
                now_ms: frame_now_ms,
            },
            args: Vec::new(),
        },
    )
    .await?;
    session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            context.player.playback_frame_count =
                context.player.playback_frame_count.saturating_add(1);
            Ok::<(), ScriptError>(())
        })
        .ok_or_else(cancelled_scope_error)??;
    // The timeout-target ExitFrame dispatch is a separate phase in the
    // legacy loop. It follows the frame/movie ExitFrame callbacks and must
    // complete before any go/advance decision is observed.
    dispatch_system_event_to_timeouts_owned(
        session.clone(),
        player_id,
        owner.clone(),
        BuiltInSymbol::ExitFrame,
        Vec::new(),
    )
    .await?;
    // An eager goto can replace the movie while frame callbacks are pending;
    // the replacement owns the next init sequence and must not be advanced by
    // this stale frame.
    let pending_after_callbacks = session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            Ok(context.player.pending_movie_init)
        })
        .ok_or_else(cancelled_scope_error)??;
    if pending_after_callbacks {
        return Ok((playing, paused));
    }
    let (
        is_delayed,
        has_player_frame_changed,
        has_frame_changed_in_go,
        go_same_frame,
        go_direction,
    ) = session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            let delayed = context.player.delay_until.map_or(false, |until| {
                if chrono::Local::now() < until {
                    true
                } else {
                    context.player.delay_until = None;
                    false
                }
            });
            Ok((
                delayed,
                context.player.has_player_frame_changed,
                context.player.has_frame_changed_in_go,
                context.player.go_same_frame,
                context.player.go_direction,
            ))
        })
        .ok_or_else(cancelled_scope_error)??;
    if is_delayed {
        return Ok((playing, paused));
    }
    let (previous_frame, next_frame) = session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            Ok((
                context.player.movie.current_frame,
                context.player.get_next_frame(),
            ))
        })
        .ok_or_else(cancelled_scope_error)??;
    // Exit-frame handlers have already completed in the owned MovieAsync
    // phase. Preserve go()'s same-frame and explicit-frame state before the
    // ordinary advance, and consume these flags only after owner validation.
    if has_player_frame_changed {
        session
            .borrow_mut()
            .with_player(player_id, |context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(cancelled_scope_error());
                }
                context.player.has_player_frame_changed = false;
                context.player.has_frame_changed_in_go = false;
                context.player.go_same_frame = false;
                if context.player.go_direction > 0 {
                    context.player.go_direction = 0;
                }
                Ok::<(), ScriptError>(())
            })
            .ok_or_else(cancelled_scope_error)??;
    } else if go_same_frame {
        session
            .borrow_mut()
            .with_player(player_id, |context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(cancelled_scope_error());
                }
                context.player.go_same_frame = false;
                context.player.next_frame = None;
                Ok::<(), ScriptError>(())
            })
            .ok_or_else(cancelled_scope_error)??;
    } else if has_frame_changed_in_go {
        session
            .borrow_mut()
            .with_player(player_id, |context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(cancelled_scope_error());
                }
                context.player.has_frame_changed_in_go = false;
                context.player.has_player_frame_changed = false;
                Ok::<(), ScriptError>(())
            })
            .ok_or_else(cancelled_scope_error)??;
    } else {
        let ended = end_score_sprites_owned(
            session.clone(),
            player_id,
            owner.clone(),
            ScoreRef::Stage,
            previous_frame,
            next_frame,
            false,
        )
        .await?;
        session
            .borrow_mut()
            .with_player(player_id, |context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(cancelled_scope_error());
                }
                for sprite_num in ended {
                    if let Some(sprite) = get_score_sprite_mut(
                        &mut context.player.movie,
                        &ScoreRef::Stage,
                        sprite_num as i16,
                    ) {
                        sprite.exited = true;
                    }
                }
                Ok::<(), ScriptError>(())
            })
            .ok_or_else(cancelled_scope_error)??;
    }
    let result = session
        .borrow_mut()
        .with_player(player_id, |mut context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            let ordinary_advance =
                !has_player_frame_changed && !go_same_frame && !has_frame_changed_in_go;
            if !context.player.is_script_paused && ordinary_advance {
                context.player.advance_frame();
                context.player.begin_all_sprites(context.symbols);
                context
                    .player
                    .movie
                    .score
                    .apply_tween_modifiers(context.player.movie.current_frame);
            }
            Ok((context.player.is_playing, context.player.is_script_paused))
        })
        .ok_or_else(cancelled_scope_error)??;
    // The legacy frame loop returns before film-loop advancement while paused or
    // stopped. Keep that gate here so a retained playhead does not mutate a
    // paused/replacement score after callbacks have completed.
    if result.0 && !result.1 {
        advance_filmloops_owned(session.clone(), player_id, owner.clone()).await?;
        // Linked `#movie` sprites are owner-bound children. Start them only
        // after parent callbacks and filmloops have settled, then advance and
        // composite each child before the frontend draws the parent.
        nested::activate_nested_players_owned(session.clone(), player_id, owner.clone()).await?;
        nested::advance_nested_players_owned(
            session.clone(),
            player_id,
            owner.clone(),
            frame_now_ms,
        )
        .await?;
        let _ = nested::render_nested_players_owned(&session, player_id, &owner)?;
    }
    // `go_direction` is consumed with the same owner validation as the frame
    // transition; retain it above for diagnostics and ordering.
    let _ = go_direction;
    Ok(result)
}

/// Owner-bound counterpart of the score portion of `DirPlayer::end_all_sprites`.
/// Score dispatch itself can suspend in a script callback, so the callback list
/// is snapshotted before every await and the owner is revalidated before each
/// subsequent mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum EndSpriteErrorPolicy {
    /// Preserve ordinary frame advancement behavior: report a live callback
    /// error and continue the remaining EndSprite callbacks.
    ReportAndContinue,
    /// StopMovie collects the first live callback error and returns it only
    /// after all callbacks and score-exit bookkeeping have been attempted.
    CollectForStop,
}

struct EndScoreSpritesResult {
    ended_channels: Vec<u32>,
    first_error: Option<ScriptError>,
}

async fn end_score_sprites_owned_with_policy(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
    score_ref: ScoreRef,
    previous_frame: u32,
    next_frame: u32,
    end_all_active: bool,
    error_policy: EndSpriteErrorPolicy,
) -> Result<EndScoreSpritesResult, ScriptError> {
    let (callbacks, frame_script_ended, ended_channels) = session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            let score = match &score_ref {
                ScoreRef::Stage => Some(&context.player.movie.score),
                ScoreRef::FilmLoop(member_ref) => context
                    .player
                    .movie
                    .cast_manager
                    .find_member_by_ref(member_ref)
                    .and_then(|member| match &member.member_type {
                        CastMemberType::FilmLoop(film_loop) => Some(&film_loop.score),
                        _ => None,
                    }),
            }
            .ok_or_else(cancelled_scope_error)?;
            let mut ended: Vec<u32> = if end_all_active {
                Vec::new()
            } else {
                score
                    .sprite_spans
                    .iter()
                    .filter(|span| {
                        Score::is_span_in_frame(span, previous_frame)
                            && !Score::is_span_in_frame(span, next_frame)
                    })
                    .map(|span| span.channel_number)
                    .fold(Vec::new(), |mut channels, channel| {
                        if !channels.contains(&channel) {
                            channels.push(channel);
                        }
                        channels
                    })
            };
            if end_all_active {
                // StopMovie ends every entered/persistent channel in this
                // score, including D6+ channels synthesized from
                // channel_initialization_data that have no sprite span.
                for channel in &score.channels {
                    if channel.sprite.entered && !channel.sprite.exited {
                        let channel_number = channel.number as u32;
                        if !ended.contains(&channel_number) {
                            ended.push(channel_number);
                        }
                    }
                }
            }
            let mut callbacks: Vec<Vec<ScriptInstanceRef>> = Vec::new();
            for channel_number in &ended {
                if *channel_number == 0 {
                    continue;
                }
                if let Some(channel) = score.channels.get(*channel_number as usize) {
                    if !channel.sprite.script_instance_list.is_empty() {
                        callbacks.push(channel.sprite.script_instance_list.clone());
                    }
                }
            }
            let frame_script_ended = matches!(&score_ref, ScoreRef::Stage)
                && ended.iter().any(|channel| {
                    *channel == 0
                        && score
                            .channels
                            .get(0)
                            .is_some_and(|channel| !channel.sprite.script_instance_list.is_empty())
                });
            Ok((callbacks, frame_script_ended, ended))
        })
        .ok_or_else(cancelled_scope_error)??;

    let _score_context = crate::player::events::OwnedScoreContextScope::enter(
        &session,
        player_id,
        &owner,
        score_ref.clone(),
    )?;
    let mut first_error = None;
    if frame_script_ended {
        let static_result = crate::player::events::player_invoke_static_event_owned(
            &session,
            player_id,
            &owner,
            Symbol::builtin(BuiltInSymbol::EndSprite),
            &[],
        )
        .await;
        if let Err(error) = static_result {
            if error.code != ScriptErrorCode::Abort
                && error_policy == EndSpriteErrorPolicy::CollectForStop
            {
                first_error = Some(error);
            }
        }
    }
    for behavior_group in callbacks {
        for behavior in behavior_group {
            let handler = session
                .borrow_mut()
                .with_player(player_id, |context| {
                    if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                        return Err(cancelled_scope_error());
                    }
                    ScriptInstanceUtils::get_script_instance_handler(
                        Symbol::builtin(BuiltInSymbol::EndSprite),
                        &behavior,
                        context.player,
                    )
                })
                .ok_or_else(cancelled_scope_error)??;
            if let Some(handler) = handler {
                let callback_result = crate::player::eval::invoke_script_callback_owned(
                    session.clone(),
                    player_id,
                    owner.clone(),
                    Some(behavior),
                    handler,
                    Vec::new(),
                    false,
                )
                .await;
                match callback_result {
                    Ok(_)
                    | Err(ScriptError {
                        code: ScriptErrorCode::HandlerNotFound,
                        ..
                    }) => {}
                    Err(ScriptError {
                        code: ScriptErrorCode::Abort,
                        ..
                    }) => break,
                    Err(error) => {
                        if error_policy == EndSpriteErrorPolicy::ReportAndContinue {
                            let _ = session.borrow_mut().with_player(player_id, |context| {
                                if owner.same_identity(&context.player.owner)
                                    && owner.is_arena_live()
                                {
                                    context.player.on_script_error_with_symbols(
                                        &error,
                                        Some(context.symbols),
                                    );
                                }
                            });
                        } else if first_error.is_none() {
                            first_error = Some(error);
                        }
                    }
                }
            }
        }
    }
    Ok(EndScoreSpritesResult {
        ended_channels,
        first_error,
    })
}

async fn end_score_sprites_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
    score_ref: ScoreRef,
    previous_frame: u32,
    next_frame: u32,
    end_all_active: bool,
) -> Result<Vec<u32>, ScriptError> {
    Ok(end_score_sprites_owned_with_policy(
        session,
        player_id,
        owner,
        score_ref,
        previous_frame,
        next_frame,
        end_all_active,
        EndSpriteErrorPolicy::ReportAndContinue,
    )
    .await?
    .ended_channels)
}

/// Advance active film-loop scores after the stage playhead has settled. The
/// old helper borrowed a global player across each async EndSprite callback;
/// this split snapshots one score at a time and reacquires the captured owner
/// only for the synchronous frame mutation.
async fn advance_filmloops_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
) -> Result<(), ScriptError> {
    let plans = session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            let refs = context.player.active_stage_filmloop_member_refs();
            let mut plans = Vec::new();
            for member_ref in refs {
                let Some(member) = context
                    .player
                    .movie
                    .cast_manager
                    .find_member_by_ref(&member_ref)
                else {
                    continue;
                };
                let CastMemberType::FilmLoop(film_loop) = &member.member_type else {
                    continue;
                };
                let old_frame = film_loop.current_frame;
                let frame_count = film_loop.score.frame_count.unwrap_or(1).max(1);
                let next = old_frame + 1;
                let next_frame = if next > frame_count {
                    if film_loop.info.loops != 0 {
                        1
                    } else {
                        frame_count
                    }
                } else {
                    next
                };
                if old_frame != next_frame {
                    plans.push((member_ref, old_frame, next_frame));
                }
            }
            Ok(plans)
        })
        .ok_or_else(cancelled_scope_error)??;

    for (member_ref, old_frame, next_frame) in plans {
        let score_ref = ScoreRef::FilmLoop(member_ref.clone());
        let ended = end_score_sprites_owned(
            session.clone(),
            player_id,
            owner.clone(),
            score_ref.clone(),
            old_frame,
            next_frame,
            false,
        )
        .await?;
        let changed = session
            .borrow_mut()
            .with_player(player_id, |context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(cancelled_scope_error());
                }
                let mut changed = false;
                if let Some(member) = context
                    .player
                    .movie
                    .cast_manager
                    .find_mut_member_by_ref(&member_ref)
                {
                    if let CastMemberType::FilmLoop(film_loop) = &mut member.member_type {
                        for sprite_num in ended {
                            film_loop.score.get_sprite_mut(sprite_num as i16).exited = true;
                        }
                        film_loop.current_frame = next_frame;
                        changed = true;
                    }
                }
                if changed {
                    context.player.begin_score_sprites(
                        score_ref.clone(),
                        next_frame,
                        context.symbols,
                    );
                    if let Some(member) = context
                        .player
                        .movie
                        .cast_manager
                        .find_mut_member_by_ref(&member_ref)
                    {
                        if let CastMemberType::FilmLoop(film_loop) = &mut member.member_type {
                            film_loop.score.apply_tween_modifiers(next_frame);
                        }
                    }
                }
                if changed {
                    context.player.invalidate_behavior_channel_cache();
                    context.player.invalidate_active_stage_filmloop_cache();
                    context.player.stage_dirty = true;
                }
                Ok::<bool, ScriptError>(changed)
            })
            .ok_or_else(cancelled_scope_error)??;
        let _ = changed;
    }
    Ok(())
}

/// Compatibility boundary for native callers that still let the harness
/// choose the sample. The owned frame path receives the resulting value as a
/// parameter and never samples a host clock after it starts.
pub async fn run_single_frame_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: crate::player::ownership::OwnerToken,
) -> Result<(bool, bool), ScriptError> {
    run_single_frame_owned_at(
        session,
        player_id,
        owner,
        crate::player::testing_shared::now_ms().max(0.0),
    )
    .await
}

/// Perform the movie transition for gotoNetMovie.
/// Called from within the frame loop when the pending fetch is complete.

/// Owner-bound StopMovie/endSprite sequence used by the detached playback
/// loop. Every callback is dispatched through the captured session owner and
/// every subsequent mutation revalidates that owner before touching the score.
pub(crate) async fn stop_movie_sequence_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
) -> Result<(), ScriptError> {
    let mut first_error = None;
    match dispatch_system_event_to_timeouts_owned(
        session.clone(),
        player_id,
        owner.clone(),
        BuiltInSymbol::StopMovie,
        Vec::new(),
    )
    .await
    {
        Ok(()) => {}
        Err(error) if error.code == ScriptErrorCode::Abort => return Err(error),
        Err(error) => first_error = Some(error),
    }
    session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            context.player.timeout_manager.clear();
            Ok::<(), ScriptError>(())
        })
        .ok_or_else(cancelled_scope_error)??;

    if let Err(error) = crate::player::events::player_invoke_global_event_owned(
        session.clone(),
        player_id,
        owner.clone(),
        Symbol::builtin(BuiltInSymbol::StopMovie),
        Vec::new(),
    )
    .await
    {
        if error.code == ScriptErrorCode::Abort {
            return Err(error);
        }
        if first_error.is_none() {
            first_error = Some(error);
        }
    }

    let (current_frame, next_frame, filmloops) = session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            let current = context.player.movie.current_frame;
            let next = context.player.get_next_frame();
            let loops = context.player.active_stage_filmloop_member_refs();
            Ok((current, next, loops))
        })
        .ok_or_else(cancelled_scope_error)??;
    let stage_result = end_score_sprites_owned_with_policy(
        session.clone(),
        player_id,
        owner.clone(),
        ScoreRef::Stage,
        current_frame,
        next_frame,
        true,
        EndSpriteErrorPolicy::CollectForStop,
    )
    .await?;
    if first_error.is_none() {
        first_error = stage_result.first_error;
    }
    let mut ended_scores = vec![(ScoreRef::Stage, stage_result.ended_channels)];
    for member_ref in filmloops {
        let frame_result = session
            .borrow_mut()
            .with_player(player_id, |context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(cancelled_scope_error());
                }
                let film_loop = context
                    .player
                    .movie
                    .cast_manager
                    .find_member_by_ref(&member_ref)
                    .and_then(|member| member.member_type.as_film_loop())
                    .ok_or_else(cancelled_scope_error)?;
                Ok((film_loop.current_frame, film_loop.current_frame + 1))
            })
            .ok_or_else(cancelled_scope_error)?;
        let (old_frame, next_frame) = match frame_result {
            Ok(frames) => frames,
            Err(error) if error.code == ScriptErrorCode::Abort => return Err(error),
            Err(error) => {
                if first_error.is_none() {
                    first_error = Some(error);
                }
                continue;
            }
        };
        let score_ref = ScoreRef::FilmLoop(member_ref);
        let film_result = end_score_sprites_owned_with_policy(
            session.clone(),
            player_id,
            owner.clone(),
            score_ref.clone(),
            old_frame,
            next_frame,
            true,
            EndSpriteErrorPolicy::CollectForStop,
        )
        .await?;
        if first_error.is_none() {
            first_error = film_result.first_error;
        }
        ended_scores.push((score_ref, film_result.ended_channels));
    }
    session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            for (score_ref, sprite_nums) in ended_scores {
                for sprite_num in sprite_nums {
                    if let Some(sprite) = get_score_sprite_mut(
                        &mut context.player.movie,
                        &score_ref,
                        sprite_num as i16,
                    ) {
                        sprite.exited = true;
                    }
                }
            }
            Ok::<(), ScriptError>(())
        })
        .ok_or_else(cancelled_scope_error)??;
    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// Owner-bound network movie transition. The fetched bytes, mounted movie,
/// target frame, and initialization sequence all remain fenced to one player
/// generation across every await.
async fn transition_to_net_movie_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
    task_id: u32,
    target: MovieFrameTarget,
    stop_phase: Rc<Cell<PlaybackTransitionStopPhase>>,
) -> Result<(), ScriptError> {
    let (data_bytes, file_name, base_url) = session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            let task = context
                .player
                .net_manager
                .get_task(task_id)
                .ok_or_else(|| ScriptError::new(format!("Network task {task_id} disappeared")))?;
            let bytes = context
                .player
                .net_manager
                .get_task_result(Some(task_id))
                .ok_or_else(|| {
                    ScriptError::new(format!("No response received for task {task_id}"))
                })?
                .map_err(|_| {
                    ScriptError::new(format!("Network request failed for task {task_id}"))
                })?;
            let file_name = task
                .resolved_url
                .path_segments()
                .and_then(|segments| segments.last())
                .unwrap_or("untitled.dcr")
                .to_owned();
            let base_url = get_base_url(&task.resolved_url).to_string();
            Ok((bytes, file_name, base_url))
        })
        .ok_or_else(cancelled_scope_error)??;
    let dir_file =
        read_director_file_bytes(&data_bytes, &file_name, &base_url).map_err(|error| {
            ScriptError::new(format!("Failed to parse movie file '{file_name}': {error}"))
        })?;

    let previous_dispatching = session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            context.player.is_playing = false;
            context.player.is_in_transition = true;
            Ok(std::mem::replace(
                &mut context.player.is_dispatching_events,
                false,
            ))
        })
        .ok_or_else(cancelled_scope_error)??;
    stop_phase.set(PlaybackTransitionStopPhase::InProgress);
    if let Err(error) = stop_movie_sequence_owned(session.clone(), player_id, owner.clone()).await {
        let _ = session.borrow_mut().with_player(player_id, |context| {
            if owner.same_identity(&context.player.owner) && owner.is_arena_live() {
                context.player.is_dispatching_events = previous_dispatching;
                context.player.is_in_transition = false;
            }
        });
        return Err(error);
    }
    stop_phase.set(PlaybackTransitionStopPhase::Complete);
    session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            context.player.is_dispatching_events = previous_dispatching;
            context.player.movie.score.reset();
            context
                .player
                .queue_player_notification(PlayerNotificationKind::ScoreChanged);
            context.player.clear_script_instance_list_caches();
            context.player.movie.frame_script_instance = None;
            context.player.movie.frame_script_member = None;
            context.player.movie.current_frame = 1;
            context.player.last_initialized_frame = None;
            let old_casts = std::mem::take(&mut context.player.movie.cast_manager.casts);
            context.player.retired_cast_libs.push(old_casts);
            context.player.is_playing = false;
            Ok::<(), ScriptError>(())
        })
        .ok_or_else(cancelled_scope_error)??;
    if let Err(error) =
        DirPlayer::load_movie_from_dir_owned(session.clone(), player_id, owner.clone(), dir_file)
            .await
    {
        let _ = session.borrow_mut().with_player(player_id, |context| {
            if owner.same_identity(&context.player.owner) && owner.is_arena_live() {
                context.player.is_dispatching_events = previous_dispatching;
                context.player.is_in_transition = false;
            }
        });
        return Err(error);
    }
    session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            match target {
                MovieFrameTarget::Label(label) => {
                    if let Some(frame) = context
                        .player
                        .movie
                        .score
                        .frame_labels
                        .iter()
                        .find(|entry| entry.label.eq_ignore_ascii_case(&label))
                        .map(|entry| entry.frame_num as u32)
                    {
                        context.player.movie.current_frame = frame;
                    }
                }
                MovieFrameTarget::Frame(frame) => context.player.movie.current_frame = frame,
                MovieFrameTarget::Default => {}
            }
            context.player.pending_goto_net_movie = None;
            context.player.is_playing = true;
            context.player.pending_movie_init = false;
            context.player.movie_mount_generation =
                context.player.movie_mount_generation.wrapping_add(1);
            Ok::<(), ScriptError>(())
        })
        .ok_or_else(cancelled_scope_error)??;
    let result =
        run_movie_init_owned_at(session.clone(), player_id, owner.clone(), bench_now_ms()).await;
    let _ = session.borrow_mut().with_player(player_id, |context| {
        if owner.same_identity(&context.player.owner) && owner.is_arena_live() {
            context.player.is_in_transition = false;
        }
    });
    result
}

/// Stop the current movie, parse the fetched bytes and MOUNT the new movie —
/// everything a `go(frame, movie)` transition does EXCEPT running the new
/// movie's init sequence. Returns false if the movie could not be loaded.
///
/// Split out so `go()` can mount eagerly. Director's `go()` (11.5 Scripting
/// Dictionary, Movie method) "loads frame 1 of the movie", and "if go() is
/// called from within a handler, the handler in which it is placed continues
/// executing" — so the REST of the calling handler already sees the new
/// movie's cast libraries. Miniclip's `wrapper_silentbaystudios.dcr` depends
/// on exactly that: it calls `go(1, "gameloader.dcr")` and then, still in the
/// same `exitFrame`, does `move(hsController, member(1, 2))` and
/// `new(script("MiniclipServices script"))` — cast lib 2 and those parent
/// scripts belong to GAMELOADER, not to the wrapper.
async fn transition_to_net_movie(task_id: u32, target: MovieFrameTarget) {
    if mount_net_movie(task_id, target, false).await {
        // 4. Run new movie initialization sequence
        run_movie_init_sequence().await;

        reserve_player_mut(|player| {
            player.is_in_transition = false;
        });
    }
}

/// `eager = true`: called from INSIDE `go()` while the calling Lingo handler is
/// still on the stack (Director semantics — the handler continues and must see
/// the new movie's casts). In that mode the live scope stack is left alone, the
/// old movie's cast libraries are RETIRED instead of dropped (suspended
/// trampoline frames hold raw pointers into them), and the init sequence is
/// deferred to the frame loop via `pending_movie_init`.
///
/// `eager = false`: the classic frame-loop transition (gotoNetMovie), no Lingo
/// on the stack; resets scopes and drops the old movie outright.
///
/// Returns true if the new movie was mounted.
pub(crate) async fn mount_net_movie(task_id: u32, target: MovieFrameTarget, eager: bool) -> bool {
    let Some(session) = retained_session_handle() else {
        log::warn!("mount_net_movie requires an owner-bound runtime session");
        return false;
    };
    let player_id = active_player_id() as u32;
    let Some(owner) = session
        .borrow_mut()
        .with_player(player_id, |context| context.player.owner.clone())
    else {
        return false;
    };
    // 1. Parse the fetched movie data
    let dir_file = reserve_player_mut(|player| {
        let task = player.net_manager.get_task(task_id);
        let data_result = player.net_manager.get_task_result(Some(task_id));

        match (task, data_result) {
            (Some(task), Some(Ok(data_bytes))) => {
                let file_name = task
                    .resolved_url
                    .path_segments()
                    .and_then(|segments| segments.last())
                    .unwrap_or("untitled.dcr")
                    .to_string();
                let base_url = get_base_url(&task.resolved_url).to_string();
                read_director_file_bytes(&data_bytes, &file_name, &base_url).ok()
            }
            _ => None,
        }
    });

    let dir_file = match dir_file {
        Some(f) => f,
        None => {
            log::warn!(
                "gotoNetMovie: failed to parse movie data for task {}",
                task_id
            );
            reserve_player_mut(|player| {
                player.pending_goto_net_movie = None;
            });
            return false;
        }
    };

    // 2. Shutdown current movie
    // Block the event loop for the entire transition: set is_playing = false and
    // is_in_transition = true. Direct calls to player_invoke_global_event (for
    // stopMovie, prepareMovie, beginSprite, etc.) are unaffected because they
    // execute inline from the frame loop task, not through the event loop channel.
    //
    // Eager mode: `go()` runs inside `dispatch_event_to_all_behaviors`, whose
    // re-entrancy guard (`is_dispatching_events`) would silently swallow the
    // endSprite dispatch inside `stop_movie_sequence`. Lift the guard for the
    // stop sequence — this nesting is deliberate — and restore it after, so the
    // suspended outer dispatch keeps its own bookkeeping intact.
    let prev_dispatching = reserve_player_mut(|player| {
        player.is_playing = false;
        player.is_in_transition = true;
        std::mem::replace(&mut player.is_dispatching_events, false)
    });

    stop_movie_sequence().await;

    // 3. Load the new movie data (preserving globals and allocator)
    reserve_player_mut(|player| {
        player.is_dispatching_events = prev_dispatching;
        player.movie.score.reset();
        player.queue_player_notification(PlayerNotificationKind::ScoreChanged);
        player.clear_script_instance_list_caches();
        player.movie.frame_script_instance = None;
        player.movie.frame_script_member = None;
        player.movie.current_frame = 1;
        player.last_initialized_frame = None;

        if eager {
            // The calling handler (and its whole trampoline chain) is still
            // suspended on the scope stack and will CONTINUE after go()
            // returns — Director's documented behaviour. Leave the scopes
            // alone, and keep the old cast libraries alive: the suspended
            // frames' BytecodeHandlerContexts retain Rc snapshots from them
            // (Scripts, HandlerDefs, and cast `name_symbols`).
            let old_casts = std::mem::take(&mut player.movie.cast_manager.casts);
            player.retired_cast_libs.push(old_casts);
        } else {
            player.bump_scope_invalidation_epoch();
            for scope in player.scopes.iter_mut() {
                scope.reset();
            }
            player.scope_count = 0;
        }

        // Temporarily set is_playing = false so that begin_all_sprites (called inside
        // load_movie_from_dir) resets entered flags and clears script instance lists,
        // matching the behavior of the initial movie load path.
        player.is_playing = false;
    });

    DirPlayer::load_movie_from_dir_owned(session.clone(), player_id, owner.clone(), dir_file)
        .await
        .is_ok()
        || return false;

    // Apply frame target if specified, restore is_playing before init sequence
    reserve_player_mut(|player| {
        match &target {
            MovieFrameTarget::Label(label) => {
                let target_frame = player
                    .movie
                    .score
                    .frame_labels
                    .iter()
                    .find(|fl| fl.label.eq_ignore_ascii_case(label))
                    .map(|fl| fl.frame_num as u32);
                if let Some(frame) = target_frame {
                    player.movie.current_frame = frame;
                }
            }
            MovieFrameTarget::Frame(frame) => {
                player.movie.current_frame = *frame;
            }
            MovieFrameTarget::Default => {}
        }
        player.pending_goto_net_movie = None;
        player.is_playing = true;
        if eager {
            // The frame loop runs the init sequence once the handler stack
            // unwinds; until then every dispatch loop must stand down.
            player.pending_movie_init = true;
            player.movie_mount_generation = player.movie_mount_generation.wrapping_add(1);
        }
    });
    true
}

/// Restart the current movie in place (`play movie <the current movie>`). Re-parses
/// the retained movie bytes and runs the same load+init flow as a movie transition —
/// rebuilding the cast (fresh W3D scenes, no stale clones) while PRESERVING globals
/// and external params (which belong to the projector/embedding, not the movie file),
/// matching Director's `play movie`. Used because the net loader often can't re-fetch
/// the movie by name once it's been loaded.
async fn restart_current_movie_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
    stop_phase: Rc<Cell<PlaybackTransitionStopPhase>>,
) -> Result<(), ScriptError> {
    let (bytes, file_name, base_url) =
        session
            .borrow_mut()
            .with_player(player_id, |context| {
                if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                    return Err(cancelled_scope_error());
                }
                context.player.movie_reload_data.clone().ok_or_else(|| {
                    ScriptError::new("current movie bytes are unavailable".to_owned())
                })
            })
            .ok_or_else(cancelled_scope_error)??;
    let dir_file = read_director_file_bytes(&bytes, &file_name, &base_url)
        .map_err(|error| ScriptError::new(format!("Failed to re-parse movie bytes: {error}")))?;
    session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            context.player.pending_restart = false;
            context.player.bump_scope_invalidation_epoch();
            context.player.is_playing = false;
            context.player.is_in_transition = true;
            Ok::<(), ScriptError>(())
        })
        .ok_or_else(cancelled_scope_error)??;
    stop_phase.set(PlaybackTransitionStopPhase::InProgress);
    if let Err(error) = stop_movie_sequence_owned(session.clone(), player_id, owner.clone()).await {
        let _ = session.borrow_mut().with_player(player_id, |context| {
            if owner.same_identity(&context.player.owner) && owner.is_arena_live() {
                context.player.is_in_transition = false;
            }
        });
        return Err(error);
    }
    stop_phase.set(PlaybackTransitionStopPhase::Complete);
    session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            context.player.movie.score.reset();
            context
                .player
                .queue_player_notification(PlayerNotificationKind::ScoreChanged);
            context.player.clear_script_instance_list_caches();
            context.player.movie.frame_script_instance = None;
            context.player.movie.frame_script_member = None;
            context.player.movie.current_frame = 1;
            context.player.last_initialized_frame = None;
            for scope in context.player.scopes.iter_mut() {
                scope.reset();
            }
            context.player.scope_count = 0;
            context.player.is_playing = false;
            Ok::<(), ScriptError>(())
        })
        .ok_or_else(cancelled_scope_error)??;
    if let Err(error) =
        DirPlayer::load_movie_from_dir_owned(session.clone(), player_id, owner.clone(), dir_file)
            .await
    {
        let _ = session.borrow_mut().with_player(player_id, |context| {
            if owner.same_identity(&context.player.owner) && owner.is_arena_live() {
                context.player.is_in_transition = false;
            }
        });
        return Err(error);
    }
    session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            context.player.movie.current_frame = 1;
            context.player.pending_restart = false;
            context.player.is_playing = true;
            Ok::<(), ScriptError>(())
        })
        .ok_or_else(cancelled_scope_error)??;
    let result =
        run_movie_init_owned_at(session.clone(), player_id, owner.clone(), bench_now_ms()).await;
    let _ = session.borrow_mut().with_player(player_id, |context| {
        if owner.same_identity(&context.player.owner) && owner.is_arena_live() {
            context.player.is_in_transition = false;
        }
    });
    result
}

async fn restart_current_movie() {
    let Some(session) = retained_session_handle() else {
        log::warn!("restart_current_movie requires an owner-bound runtime session");
        return;
    };
    let player_id = active_player_id() as u32;
    let Some(owner) = session
        .borrow_mut()
        .with_player(player_id, |context| context.player.owner.clone())
    else {
        return;
    };
    let reload = reserve_player_ref(|player| player.movie_reload_data.clone());
    let (bytes, file_name, base_url) = match reload {
        Some(x) => x,
        None => {
            // No retained bytes — best-effort soft reset so we don't hang.
            reserve_player_mut(|player| {
                player.pending_restart = false;
                player.reset();
                player.play();
            });
            return;
        }
    };

    let dir_file = match read_director_file_bytes(&bytes, &file_name, &base_url) {
        Ok(f) => f,
        Err(e) => {
            log::warn!("restart: failed to re-parse movie bytes: {}", e);
            reserve_player_mut(|player| player.pending_restart = false);
            return;
        }
    };

    // Shut the current movie down (block the event loop during the transition).
    reserve_player_mut(|player| {
        player.pending_restart = false;
        player.bump_scope_invalidation_epoch();
        player.is_playing = false;
        player.is_in_transition = true;
    });
    stop_movie_sequence().await;

    // Rebuild from frame 1, preserving globals + the allocator (like a transition).
    reserve_player_mut(|player| {
        player.movie.score.reset();
        player.queue_player_notification(PlayerNotificationKind::ScoreChanged);
        player.clear_script_instance_list_caches();
        player.movie.frame_script_instance = None;
        player.movie.frame_script_member = None;
        player.movie.current_frame = 1;
        player.last_initialized_frame = None;
        for scope in player.scopes.iter_mut() {
            scope.reset();
        }
        player.scope_count = 0;
        player.is_playing = false;
    });

    if let Err(error) =
        DirPlayer::load_movie_from_dir_owned(session.clone(), player_id, owner.clone(), dir_file)
            .await
    {
        log::warn!("restart: owner-bound movie load failed: {}", error.message);
        return;
    }

    reserve_player_mut(|player| {
        player.movie.current_frame = 1;
        player.pending_restart = false;
        player.is_playing = true;
    });

    run_movie_init_sequence().await;

    reserve_player_mut(|player| {
        player.is_in_transition = false;
    });
}

// JS bridge name uses the `dirplayer_` prefix so this fork's globals don't
// collide with stock Ruffle / other libraries on the same page.
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = "dirplayer_isFlashLoading", catch)]
    pub(crate) fn is_flash_loading() -> Result<bool, wasm_bindgen::JsValue>;

    /// Resize a live Ruffle instance so it re-renders the vector sharp at the
    /// sprite's current on-stage size (splashes grow, arm swaps dims).
    ///
    /// `catch`: these four are called from the per-frame loop, so a page that
    /// hasn't installed the Flash bridge (`initFlashBridge`) — e.g. the e2e test
    /// harness, or the dev app before startup wiring runs — would otherwise throw
    /// a `ReferenceError` (`dirplayer_ruffleSetSize is not defined`) that aborts
    /// the whole frame loop. With `catch` the missing global degrades to a no-op.
    #[wasm_bindgen(js_name = "dirplayer_ruffleSetSize", catch)]
    fn ruffle_set_size(sprite_num: i32, w: i32, h: i32) -> Result<(), wasm_bindgen::JsValue>;

    // Used to halt a `loop = false` Flash sprite at end-of-timeline.
    #[wasm_bindgen(js_name = "dirplayer_ruffleIsPlaying", catch)]
    fn ruffle_is_playing(sprite_num: i32) -> Result<bool, wasm_bindgen::JsValue>;
    #[wasm_bindgen(js_name = "dirplayer_ruffleGetCurrentFrame", catch)]
    fn ruffle_get_current_frame(sprite_num: i32) -> Result<i32, wasm_bindgen::JsValue>;
    #[wasm_bindgen(js_name = "dirplayer_ruffleGoToFrameAndStop", catch)]
    fn ruffle_goto_frame_and_stop(
        sprite_num: i32,
        frame_or_label: &str,
    ) -> Result<(), wasm_bindgen::JsValue>;
    #[wasm_bindgen(js_name = "dirplayer_ruffleGoToFrameAndStopOwned", catch)]
    fn ruffle_goto_frame_and_stop_owned(
        owner_key: &str,
        sprite_num: i32,
        frame_or_label: &str,
    ) -> Result<(), wasm_bindgen::JsValue>;
    /// Halt the root timeline. The seek above only PINS for `pausedAtStart`
    /// members, so the park needs this to actually stop an animated one.
    #[wasm_bindgen(js_name = "dirplayer_ruffleStop")]
    fn ruffle_stop(sprite_num: i32);
}
/// Execute one complete frame cycle: run frame scripts, then advance to the next frame.
/// Returns (is_playing, is_script_paused) so callers can check if the movie is still running.
///
/// This contains the core per-frame logic extracted from `run_frame_loop`, minus the
/// timing/delay logic and gotoNetMovie handling which are loop-level concerns.
/// Fire all scheduled timeouts immediately. In the real player, timeouts are
/// driven by JS `setInterval`, but in tests there is no JS event loop (native)
/// or frames run too fast for intervals to fire (browser). Call this each frame
/// in test harnesses before `run_single_frame`.
/// Fire scheduled timeouts whose period has elapsed according to wall-clock time.
/// Each timeout that fires is rescheduled to `now + period`. The time is captured
/// once at the start so handlers that take time don't cause cascading re-fires.
pub async fn fire_pending_timeouts() {
    let now = testing_shared::now_ms();
    let pending_timeouts: Vec<(DatumRef, Symbol, String)> = reserve_player_mut(|player| {
        let mut ready = Vec::new();
        for t in player.timeout_manager.timeouts.values_mut() {
            if t.is_scheduled {
                let remaining = t.next_fire_ms - now;
                if remaining <= 0.0 {
                    t.next_fire_ms = now + t.period as f64;
                    ready.push((t.target_ref.clone(), t.handler.clone(), t.name.clone()));
                }
            }
        }
        ready
    });
    for (target_ref, handler_name, timeout_name) in pending_timeouts {
        let ref_datum = player_alloc_datum(Datum::TimeoutRef(timeout_name.clone()));
        let args = vec![ref_datum];
        if target_ref != DatumRef::Void {
            let result = call_datum_handler_active(&target_ref, handler_name.clone(), &args).await;
            match result {
                crate::player::handlers::datum_handlers::DatumDispatch::Pending {
                    request,
                    reason,
                } => {
                    retain_datum_pending(request, reason);
                }
                crate::player::handlers::datum_handlers::DatumDispatch::Sync(Err(err))
                    if err.code != ScriptErrorCode::HandlerNotFound =>
                {
                    warn!(
                        "Timeout '{}' handler '{:?}' error: {}",
                        timeout_name, handler_name, err.message
                    );
                }
                _ => {}
            }
        } else if let Err(err) =
            player_invoke_global_event(Symbol::builtin(BuiltInSymbol::Timeout), &args).await
        {
            if err.code != ScriptErrorCode::HandlerNotFound {
                warn!(
                    "Timeout '{}' handler '{:?}' error: {}",
                    timeout_name, handler_name, err.message
                );
            }
        }
    }
}

/// Fire elapsed timeouts for one captured runtime owner.  This is the
/// session-owned counterpart to the legacy active-player entrypoint: timeout
/// records and datum allocation are read with one short context borrow, and
/// every handler turn is resumed or retained before the next timeout is
/// visited.  A pending request remains in the session queue with its owner;
/// it is never reported as a completed timeout merely because the host must
/// supply a result later.
async fn dispatch_timeout_target_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
    target_ref: DatumRef,
    handler_name: Symbol,
    args: Vec<DatumRef>,
    timeout_name: &str,
) -> Result<(), ScriptError> {
    let dispatch = session
        .borrow_mut()
        .with_player(player_id, |mut context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            context
                .player
                .checked_movie_datum_ref(context.symbols, &target_ref)?;
            Ok(
                crate::player::handlers::datum_handlers::player_call_datum_handler(
                    &mut context,
                    &target_ref,
                    handler_name.clone(),
                    &args,
                ),
            )
        })
        .ok_or_else(cancelled_scope_error)??;

    match dispatch {
        crate::player::handlers::datum_handlers::DatumDispatch::Child {
            receiver,
            handler_ref,
            args,
            ..
        } => {
            match crate::player::eval::invoke_script_callback_owned(
                session,
                player_id,
                owner,
                receiver,
                handler_ref,
                args,
                false,
            )
            .await
            {
                Ok(_)
                | Err(ScriptError {
                    code: ScriptErrorCode::HandlerNotFound,
                    ..
                }) => {}
                Err(error) if error.code == ScriptErrorCode::Abort => return Ok(()),
                Err(error) => warn!(
                    "Timeout '{}' child handler error: {}",
                    timeout_name, error.message
                ),
            }
        }
        crate::player::handlers::datum_handlers::DatumDispatch::ChildWithCompletion {
            receiver,
            handler_ref,
            args,
            completion,
        } => {
            match crate::player::eval::invoke_script_callback_owned(
                session.clone(),
                player_id,
                owner.clone(),
                receiver,
                handler_ref,
                args,
                false,
            )
            .await
            {
                Ok(scope) => {
                    if !session
                        .borrow_mut()
                        .with_player(player_id, |context| {
                            owner.same_identity(&context.player.owner) && owner.is_arena_live()
                        })
                        .unwrap_or(false)
                    {
                        return Err(cancelled_scope_error());
                    }
                    session.borrow_mut().apply_child_completion(
                        player_id,
                        completion,
                        scope.return_value,
                    )?;
                }
                Err(ScriptError {
                    code: ScriptErrorCode::HandlerNotFound,
                    ..
                }) => {}
                Err(error) if error.code == ScriptErrorCode::Abort => return Ok(()),
                Err(error) => warn!(
                    "Timeout '{}' child handler error: {}",
                    timeout_name, error.message
                ),
            }
        }
        crate::player::handlers::datum_handlers::DatumDispatch::Sync(Err(error))
            if error.code != ScriptErrorCode::HandlerNotFound =>
        {
            warn!(
                "Timeout '{}' handler '{:?}' error: {}",
                timeout_name, handler_name, error.message
            );
        }
        crate::player::handlers::datum_handlers::DatumDispatch::Pending { request, reason } => {
            session
                .borrow_mut()
                .retain_deferred_request(player_id, request, reason);
        }
        _ => {}
    }
    Ok(())
}

async fn dispatch_system_event_to_timeouts_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
    handler_name: BuiltInSymbol,
    args: Vec<DatumRef>,
) -> Result<(), ScriptError> {
    let targets = session
        .borrow_mut()
        .with_player(player_id, |context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            Ok(context
                .player
                .timeout_manager
                .timeouts
                .values()
                .filter(|timeout| timeout.is_scheduled)
                .map(|timeout| timeout.target_ref.clone())
                .collect::<Vec<_>>())
        })
        .ok_or_else(cancelled_scope_error)??;

    for target_ref in targets {
        if !session
            .borrow_mut()
            .with_player(player_id, |context| {
                owner.same_identity(&context.player.owner) && owner.is_arena_live()
            })
            .unwrap_or(false)
        {
            return Err(cancelled_scope_error());
        }
        if target_ref == DatumRef::Void {
            match crate::player::events::player_invoke_global_event_owned(
                session.clone(),
                player_id,
                owner.clone(),
                Symbol::builtin(handler_name),
                args.clone(),
            )
            .await
            {
                Ok(_)
                | Err(ScriptError {
                    code: ScriptErrorCode::HandlerNotFound,
                    ..
                }) => {}
                Err(error) if error.code == ScriptErrorCode::Abort => return Ok(()),
                Err(error) => warn!(
                    "Timeout system event {:?} error: {}",
                    handler_name, error.message
                ),
            }
        } else {
            dispatch_timeout_target_owned(
                session.clone(),
                player_id,
                owner.clone(),
                target_ref,
                Symbol::builtin(handler_name),
                args.clone(),
                &format!("system {:?}", handler_name),
            )
            .await?;
        }
    }
    Ok(())
}

pub(crate) async fn fire_pending_timeouts_owned_at(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
    now: f64,
) -> Result<(), ScriptError> {
    let pending = session
        .borrow_mut()
        .with_player(player_id, |mut context| {
            if !owner.same_identity(&context.player.owner) || !owner.is_arena_live() {
                return Err(cancelled_scope_error());
            }
            let mut ready: Vec<(DatumRef, Symbol, String)> = Vec::new();
            for timeout in context.player.timeout_manager.timeouts.values_mut() {
                if timeout.is_scheduled && timeout.next_fire_ms - now <= 0.0 {
                    timeout.next_fire_ms = now + timeout.period as f64;
                    ready.push((
                        timeout.target_ref.clone(),
                        timeout.handler.clone(),
                        timeout.name.clone(),
                    ));
                }
            }
            let ready: Vec<(DatumRef, Symbol, String, DatumRef)> = ready
                .into_iter()
                .map(|(target_ref, handler, name)| {
                    let timeout_ref = context.player.alloc_datum(Datum::TimeoutRef(name.clone()));
                    (target_ref, handler, name, timeout_ref)
                })
                .collect();
            Ok(ready)
        })
        .ok_or_else(cancelled_scope_error)??;

    for (target_ref, handler_name, timeout_name, timeout_ref) in pending {
        if !session
            .borrow_mut()
            .with_player(player_id, |context| {
                owner.same_identity(&context.player.owner) && owner.is_arena_live()
            })
            .unwrap_or(false)
        {
            return Err(cancelled_scope_error());
        }
        let args = vec![timeout_ref];
        if target_ref != DatumRef::Void {
            dispatch_timeout_target_owned(
                session.clone(),
                player_id,
                owner.clone(),
                target_ref,
                handler_name,
                args,
                &timeout_name,
            )
            .await?;
        } else {
            match crate::player::events::player_invoke_global_event_owned(
                session.clone(),
                player_id,
                owner.clone(),
                Symbol::builtin(BuiltInSymbol::Timeout),
                args,
            )
            .await
            {
                Ok(_) => {}
                Err(error) if error.code == ScriptErrorCode::HandlerNotFound => {}
                Err(error) => warn!(
                    "Timeout '{}' global handler error: {}",
                    timeout_name, error.message
                ),
            }
        }
    }
    Ok(())
}

/// Compatibility boundary for harness callers that have not yet supplied the
/// frame timestamp. The owner-bound timeout phase itself receives the sample
/// as an argument and does not access a host clock.
pub(crate) async fn fire_pending_timeouts_owned(
    session: RuntimeSessionHandle,
    player_id: u32,
    owner: OwnerToken,
) -> Result<(), ScriptError> {
    fire_pending_timeouts_owned_at(session, player_id, owner, testing_shared::now_ms().max(0.0))
        .await
}

pub async fn run_single_frame() -> (bool, bool) {
    let (mut is_playing, mut is_script_paused) =
        reserve_player_ref(|player| (player.is_playing, player.is_script_paused));
    if !is_playing {
        return (false, is_script_paused);
    }
    let session_handle =
        retained_session_handle().expect("frame advancement requires the owning runtime session");
    let player_id = active_player_id() as u32;
    let owner = session_handle
        .borrow_mut()
        .with_player(player_id, |context| context.player.owner.clone())
        .expect("frame advancement player disappeared");

    // A score/puppet transition is animating: hold ALL frame processing (scripts +
    // advance) until it finishes, so the movie doesn't run ahead of the visible
    // effect (and so a per-frame `puppetTransition` in a `go the frame` loop can't
    // reset it every tick). The hold is time-based and self-clearing, so it can
    // never permanently freeze the movie. The ~24fps draw loop keeps rendering the
    // transition. Matches Director, which blocks the playhead during a transition.
    if reserve_player_mut(|player| player.transition_hold_active()) {
        reserve_player_mut(|player| {
            player.stage_dirty = true;
        });
        return (is_playing, is_script_paused);
    }

    // An eager go(frame, movie) swapped the movie while a handler was on the
    // stack; nothing may run against the new movie until its init sequence
    // (prepareMovie/startMovie) has run. The browser frame loop normally
    // handles this before calling us, but the NATIVE test harness drives
    // run_single_frame directly — so run the deferred init here too once the
    // handler stack has unwound.
    if reserve_player_ref(|player| player.pending_movie_init) {
        if reserve_player_ref(|player| player.handler_stack_depth == 0) {
            reserve_player_mut(|player| {
                player.pending_movie_init = false;
            });
            run_movie_init_sequence().await;
            reserve_player_mut(|player| {
                player.is_in_transition = false;
                player.retired_cast_libs.clear();
            });
        }
        return (is_playing, is_script_paused);
    }

    // Global idle timeout (`the timeoutScript` / `the timeoutLength`): fire the
    // timeOut event + primary script once the idle period lapses. Runs every tick.
    crate::player::events::check_global_timeout().await;

    // Deferred puppetSprite(N,FALSE) revert: a sprite unpuppeted on a prior tick
    // and not re-puppeted / re-membered since reverts to the Score now (Director
    // defers the revert to the next frame update). Runs every tick so it fires
    // even for single-frame movies — Coke Studios' WallItems unpuppet furniture
    // WITHOUT setting visible=0 and relied on the old immediate wipe to clear it.
    reserve_player_mut(|player| {
        let frame = player.movie.current_frame;
        if player.movie.score.process_pending_unpuppet_reverts(frame) {
            player.movie.score.invalidate_render_channel_cache();
            player.stage_dirty = true;
        }
    });

    // Dispatch streamStatus for any net tasks that completed since last check
    stream_status::dispatch_pending_stream_status().await;

    // --- Phase 1: Execute frame scripts ---
    if !is_script_paused {
        player_wait_available().await;

        let skip_frame =
            reserve_player_ref(|player| player.command_handler_yielding || player.in_mouse_command);
        if !skip_frame {
            let update_result = MovieHandlers::execute_frame_update().await;

            reserve_player_mut(|player| {
                if let Err(err) = update_result {
                    if err.code != ScriptErrorCode::Abort {
                        player.on_script_error(&err);
                    }
                }
            });
        }
    }

    // Eager movie mount during the frame scripts: stop this frame cycle.
    if reserve_player_ref(|player| player.pending_movie_init) {
        return (is_playing, is_script_paused);
    }

    // --- Phase 2: Advance to next frame ---
    let mut prev_frame = 0;
    let mut new_frame = 0;
    reserve_player_mut(|player| {
        is_playing = player.is_playing;
        is_script_paused = player.is_script_paused;
        if !player.is_playing {
            return;
        }
        prev_frame = player.movie.current_frame;
        if !player.is_script_paused {
            new_frame = player.get_next_frame();
        } else {
            new_frame = prev_frame;
        }
    });
    if !is_playing {
        return (false, is_script_paused);
    }
    if new_frame > 1 && prev_frame <= 1 {
        unsafe {
            let player = crate::player::player_mut();
            let _requests = player.movie.cast_manager.prepare_preload_requests(
                CastPreloadReason::AfterFrameOne,
                player.owner.key(),
                &player.dir_cache,
                player.net_manager.base_path.as_ref(),
                player.net_manager.override_base_path.as_deref(),
            );
        }
    }

    if is_script_paused {
        return (is_playing, is_script_paused);
    }

    // Check if delay() is in effect
    let is_delayed = reserve_player_mut(|player| {
        if let Some(until) = player.delay_until {
            if chrono::Local::now() < until {
                true
            } else {
                player.delay_until = None;
                false
            }
        } else {
            false
        }
    });

    let skip_frame =
        reserve_player_ref(|player| player.command_handler_yielding || player.in_mouse_command);
    if skip_frame || is_delayed {
        return (is_playing, is_script_paused);
    }

    let (has_player_frame_changed, has_frame_changed_in_go, go_direction) =
        reserve_player_ref(|player| {
            (
                player.has_player_frame_changed,
                player.has_frame_changed_in_go,
                player.go_direction,
            )
        });

    player_wait_available().await;

    // Relay exitFrame to timeout targets
    dispatch_system_event_to_timeouts(BuiltInSymbol::ExitFrame, &vec![]).await;

    // Eager movie mount during a timeout's exitFrame: stop this frame cycle.
    if reserve_player_ref(|player| player.pending_movie_init) {
        return (is_playing, is_script_paused);
    }

    let mut stayed_on_same_frame = false;

    // Clear stale go_same_frame from previous frame's processing
    reserve_player_mut(|player| {
        player.go_same_frame = false;
    });

    if has_player_frame_changed {
        player_wait_available().await;

        if has_frame_changed_in_go && go_direction == 1 {
            // backwards
            dispatch_event_to_all_behaviors(Symbol::builtin(BuiltInSymbol::ExitFrame), &vec![])
                .await;
        } else {
            // Forward advance/go: the arriving (or looping-in-place) frame's sprite
            // BEHAVIORS were never sent exitFrame here — only the frame+movie scripts ran —
            // so a sprite's FIRST exitFrame on the frame it lands on was lost (Director fires
            // beginSprite -> enterFrame -> exitFrame in one frame visit; e.g. a RaycastCar's
            // updateWheelModels ran a frame late, leaving its wheels in the hover-ray path).
            // Dispatch the behaviors' exitFrame first (matching the backwards-go branch and
            // the stayed-on-frame else branch below), then the frame+movie script exitFrames.
            dispatch_event_to_all_behaviors(Symbol::builtin(BuiltInSymbol::ExitFrame), &vec![])
                .await;
            if let Err(err) = player_invoke_frame_and_movie_scripts(
                Symbol::builtin(BuiltInSymbol::ExitFrame),
                &vec![],
            )
            .await
            {
                if err.code != ScriptErrorCode::Abort {
                    reserve_player_mut(|player| player.on_script_error(&err));
                }
            }
        }

        player_wait_available().await;

        if has_frame_changed_in_go {
            reserve_player_mut(|player| {
                player.has_frame_changed_in_go = false;

                if player.go_direction > 0 {
                    player.go_direction = 0;
                }
            });
        }

        // go() already performed end_all_sprites + advance_frame + begin_all_sprites,
        // so we only clear the flag here.
        (is_playing, is_script_paused) = reserve_player_mut(|player| {
            player.has_player_frame_changed = false;
            player.go_same_frame = false;
            player.has_frame_changed_in_go = false;
            (player.is_playing, player.is_script_paused)
        });
    } else {
        player_wait_available().await;

        dispatch_event_to_all_behaviors(Symbol::builtin(BuiltInSymbol::ExitFrame), &vec![]).await;

        player_wait_available().await;

        // Eager movie mount inside an exitFrame behavior (the Miniclip wrapper
        // path): the playhead state now belongs to the NEW movie — do not
        // advance it, do not end/begin sprites; the frame loop takes over.
        if reserve_player_ref(|player| player.pending_movie_init) {
            return (is_playing, is_script_paused);
        }

        let (has_frame_changed, go_same_frame) =
            reserve_player_ref(|player| (player.has_frame_changed_in_go, player.go_same_frame));

        if go_same_frame {
            // go(the frame) — stay on current frame, no advancement.
            // Clear next_frame too: the go() handler sets it to Some(current)
            // when it sets go_same_frame, and if we don't clear it here the
            // stale value leaks into the next tick. On a subsequent tick where
            // no go() is called the main loop would otherwise hit the "normal
            // advance" path and feed the stale next_frame into advance_frame,
            // pinning the playhead to the previously-go'd frame even though
            // the script wanted to fall through.
            reserve_player_mut(|player| {
                player.go_same_frame = false;
                player.next_frame = None;
            });
            stayed_on_same_frame = true;
        } else if has_frame_changed {
            // go(differentFrame) — go() already did end/begin/advance
            reserve_player_mut(|player| {
                player.has_frame_changed_in_go = false;
                player.has_player_frame_changed = false;
            });
            stayed_on_same_frame = true;
        } else {
            // No go() called — normal frame advancement
            let ended_sprite_nums = reserve_player_mut_async(|player| {
                Box::pin(async move { player.end_all_sprites().await })
            })
            .await;
            player_wait_available().await;
            reserve_player_mut(|player| {
                for (score_source, sprite_num) in ended_sprite_nums.iter() {
                    if let Some(sprite) =
                        get_score_sprite_mut(&mut player.movie, score_source, *sprite_num as i16)
                    {
                        sprite.exited = true;
                    }
                }
            });

            (is_playing, is_script_paused) = reserve_player_mut(|player| {
                player.advance_frame();

                player.has_player_frame_changed = false;
                (player.is_playing, player.is_script_paused)
            });
        }

        player_wait_available().await;
    }

    player_wait_available().await;

    // exitFrame has run: whatever the input handlers changed is settled. A
    // frame that had a handler is drawn right here, Director's draw point, so
    // no further handler can slip in before the picture.
    if reserve_player_mut(|player| player.draw_hold_since_ms.take().is_some()) {
        let _ = crate::rendering::draw_frame_at_end_for_owner(&session_handle, player_id, &owner);
    }

    // Eager movie mount anywhere in the exitFrame dispatches above: hand the
    // rest of the cycle to the frame loop's pending-init path.
    if reserve_player_ref(|player| player.pending_movie_init) {
        return (is_playing, is_script_paused);
    }

    session_handle
        .borrow_mut()
        .with_player(player_id, |mut context| {
            if !stayed_on_same_frame {
                // begin_all_sprites manages frame_script_instance lifecycle: it preserves
                // the cached instance while the playhead stays within the same script's
                // span (so the script's properties survive across frames) and recreates
                // it only when the active frame script changes or the span is exited.
                context.player.begin_all_sprites(context.symbols);
            }

            context
                .player
                .movie
                .score
                .apply_tween_modifiers(context.player.movie.current_frame);
        })
        .expect("frame advancement player disappeared");

    player_wait_available().await;

    // Activate any Linked Movie (`#movie`) sprites that just gained their linked
    // bytes: start a nested DirPlayer running each one on its own active-player
    // id. Only the main loop reaches here (this whole frame loop runs at id 0);
    // sub-players get their own frame loops via spawn_nested_player -> play().
    if unsafe { ACTIVE_PLAYER_ID } == 0 {
        reserve_player_ref(|player| player.activate_nested_players());
        // Composite each on-stage sub-player's stage into the host so the WebGL2
        // `Movie` sprite arm can draw it. Only when sub-players exist.
        if unsafe { !NESTED_PLAYERS.is_empty() } {
            reserve_player_mut(|player| {
                player.render_nested_player_stages();
                player.stage_dirty = true;
            });
        }
    }

    let changed_filmloops = reserve_player_mut_async(|player| {
        Box::pin(async move { player.update_filmloop_frames().await })
    })
    .await;

    if !changed_filmloops.is_empty() {
        reserve_player_mut(|player| {
            for (member_ref, _, new_frame) in changed_filmloops {
                if let Some(member) = player
                    .movie
                    .cast_manager
                    .find_mut_member_by_ref(&member_ref)
                {
                    if let CastMemberType::FilmLoop(film_loop) = &mut member.member_type {
                        film_loop.score.apply_tween_modifiers(new_frame);
                    }
                }
            }
            // Filmloop frame changes require a redraw
            player.stage_dirty = true;
        });
    }

    player_wait_available().await;

    // Collect behaviors that need initialization
    let behaviors_to_init: Vec<(ScriptInstanceRef, u32)> = reserve_player_mut(|player| {
        let mut behaviors = Vec::new();
        for channel_number in player.active_stage_behavior_channels() {
            let Some((sprite_num, fallback)) =
                player
                    .movie
                    .score
                    .channels
                    .get(channel_number)
                    .map(|channel| {
                        (
                            channel.sprite.number as u32,
                            channel.sprite.script_instance_list.clone(),
                        )
                    })
            else {
                continue;
            };

            for behavior_ref in
                player.get_sprite_script_instance_ids(sprite_num as i16, fallback.as_slice())
            {
                if player
                    .allocator
                    .get_script_instance_entry(behavior_ref.id())
                    .is_some_and(|entry| !entry.script_instance.begin_sprite_called)
                {
                    behaviors.push((behavior_ref, sprite_num));
                }
            }
        }
        behaviors
    });

    // Initialize behavior default properties
    for (behavior_ref, sprite_num) in &behaviors_to_init {
        if let Err(err) = Score::initialize_behavior_defaults_async(
            session_handle.clone(),
            player_id,
            owner.clone(),
            behavior_ref.clone(),
            *sprite_num,
        )
        .await
        {
            console_warn!("Failed to initialize behavior defaults: {}", err.message);
        }
    }

    player_wait_available().await;

    reserve_player_mut(|player| {
        player.is_in_frame_update = true;
    });

    let begin_sprite_nums =
        player_dispatch_event_beginsprite(Symbol::builtin(BuiltInSymbol::BeginSprite), &vec![])
            .await;

    player_wait_available().await;

    reserve_player_mut(|player| {
        for sprite_list in begin_sprite_nums.iter() {
            for (score_source, sprite_num) in sprite_list.iter() {
                if let Some(sprite) =
                    get_score_sprite_mut(&mut player.movie, score_source, *sprite_num as i16)
                {
                    for script_ref in &sprite.script_instance_list {
                        if let Some(entry) = player
                            .allocator
                            .get_script_instance_entry_mut(script_ref.id())
                        {
                            entry.script_instance.begin_sprite_called = true;
                        }
                    }
                }
            }
        }
    });

    // Dispatch beginSprite to any remaining behaviors that weren't handled
    // by player_dispatch_event_beginsprite (e.g., puppet sprites not in the
    // score's sprite_spans, or sprites without entered=true).
    let remaining_behaviors: Vec<ScriptInstanceRef> = reserve_player_mut(|player| {
        behaviors_to_init
            .iter()
            .filter(|(behavior_ref, _)| {
                player
                    .allocator
                    .get_script_instance_entry(behavior_ref.id())
                    .map_or(false, |entry| !entry.script_instance.begin_sprite_called)
            })
            .map(|(behavior_ref, _)| behavior_ref.clone())
            .collect()
    });

    for behavior_ref in &remaining_behaviors {
        let receivers = vec![behavior_ref.clone()];
        let _ = player_invoke_event_to_instances(
            Symbol::builtin(BuiltInSymbol::BeginSprite),
            &vec![],
            &receivers,
        )
        .await;
    }

    // Mark all remaining behaviors as having had beginSprite called
    if !remaining_behaviors.is_empty() {
        reserve_player_mut(|player| {
            for behavior_ref in &remaining_behaviors {
                if let Some(entry) = player
                    .allocator
                    .get_script_instance_entry_mut(behavior_ref.id())
                {
                    entry.script_instance.begin_sprite_called = true;
                }
            }
        });
    }

    reserve_player_mut(|player| {
        player.is_in_frame_update = false;
    });

    (is_playing, is_script_paused)
}

/// Compute Lingo's `the mouseChar` / `_mouse.mouseChar` — the 1-based
/// character index in the field/text member currently under the mouse
/// pointer. Returns -1 if the pointer is not over a field/text sprite
/// or is in the "gutter" outside the text content. Matches Director
/// 11.5 Scripting Dictionary entry for `mouseChar`.
///
/// Used by Fugue No.4's Narrative member script:
///   if the textStyle of char the mouseChar of member nar = "underline"
/// to detect clicks on inline underlined links.
/// `the mouseLine` — the RETURN-delimited line number (1-based) under the mouse
/// in the field/text sprite it's over, or -1 if not over text. Reuses the
/// char-index hit-test, then counts line breaks before that char. Used by the
/// client movie's question list (clicking a question reads `the mouseLine`).
fn compute_mouse_line(player: &mut DirPlayer) -> i32 {
    let char_index = compute_mouse_char(player);
    if char_index <= 0 {
        return -1;
    }
    let (mx, my) = player.mouse_loc;
    let sprite_num = match score::get_sprite_at(player, mx, my, false) {
        Some(n) => n,
        None => return -1,
    };
    let sprite = match player.movie.score.get_sprite(sprite_num as i16) {
        Some(s) => s,
        None => return -1,
    };
    let member_ref = match sprite.member.as_ref() {
        Some(r) => r,
        None => return -1,
    };
    let member = match player.movie.cast_manager.find_member_by_ref(member_ref) {
        Some(m) => m,
        None => return -1,
    };
    let text = match &member.member_type {
        CastMemberType::Field(f) => f.text.clone(),
        CastMemberType::Text(t) => t.text.clone(),
        _ => return -1,
    };
    let chars: Vec<char> = text.chars().collect();
    let upto = (char_index as usize).saturating_sub(1).min(chars.len());
    let line = chars[..upto]
        .iter()
        .filter(|&&c| c == '\r' || c == '\n')
        .count()
        + 1;
    line as i32
}

/// `_mouse.mouseMember` / `the mouseMember` — Director 11.5 Scripting
/// Dictionary, Mouse property, read-only: "returns the cast member assigned to
/// the sprite that is under the pointer when the property is called… When the
/// pointer is not over a sprite, this property returns the result VOID."
///
/// `scripted: false` on the hit test, because the documented subject is "the
/// sprite that is under the pointer" — any sprite, not just one carrying a
/// behavior. A sprite occupying the point but holding no member (an empty
/// channel) also answers VOID, since there is no cast member to report.
fn mouse_member_datum(player: &DirPlayer) -> Datum {
    let (x, y) = (player.mouse_loc.0 as i32, player.mouse_loc.1 as i32);
    let sprite_number = match score::get_sprite_at(player, x, y, false) {
        Some(n) => n,
        None => return Datum::Void,
    };
    match player.movie.score.get_sprite(sprite_number as i16) {
        Some(sprite) => match &sprite.member {
            Some(member_ref) => Datum::CastMember(member_ref.clone()),
            None => Datum::Void,
        },
        None => Datum::Void,
    }
}

fn compute_mouse_char(player: &mut DirPlayer) -> i32 {
    let (mx, my) = player.mouse_loc;
    let sprite_num = match score::get_sprite_at(player, mx, my, false) {
        Some(n) => n as i16,
        None => return -1,
    };
    compute_char_at(player, sprite_num, mx, my)
}

/// Shared core for `the mouseChar` and the `pointToChar()` builtin. Returns the
/// 1-based character index within `sprite_num`'s text/field member at the stage
/// coordinate `(mx, my)`, or -1 if the sprite holds no text/field member, the
/// point is outside the sprite's rect, or it falls past the end of the text.
/// Matches the Director 11.5 Scripting Dictionary `pointToChar()` contract
/// ("returns -1 if the point is not within the text").
pub fn compute_char_at(player: &mut DirPlayer, sprite_num: i16, mx: i32, my: i32) -> i32 {
    let sprite = match player.movie.score.get_sprite(sprite_num) {
        Some(s) => s,
        None => return -1,
    };
    let member_ref = match sprite.member.as_ref() {
        Some(r) => r,
        None => return -1,
    };
    let member = match player.movie.cast_manager.find_member_by_ref(member_ref) {
        Some(m) => m,
        None => return -1,
    };
    // Pull the wrap width + scroll_top so wrapped/paged fields (Fugue No.4
    // Narrative scrolls in chunks via PageNext/PagePrior to a scroll_top
    // pixel offset) map clicks back to the right character index.
    // Also pull the font name + size so we hit-test using the SAME atlas
    // the renderer drew with — using `get_system_font()` here used to
    // mean clicks were resolved against system Arial widths while the
    // renderer drew with the field's PFR Arial. The two layouts diverged
    // by enough to land Fugue No.4 clicks on the wrong underlined run.
    let (
        text,
        line_spacing,
        top_spacing,
        wrap_width,
        scroll_top,
        word_wrap,
        font_name,
        font_size,
        formatting_runs,
        is_text,
    ) = match &member.member_type {
        CastMemberType::Field(f) => (
            f.text.clone(),
            f.fixed_line_space,
            f.top_spacing,
            f.width as i32,
            f.scroll_top as i32,
            f.word_wrap,
            f.font.clone(),
            f.font_size,
            f.formatting_runs.clone(),
            false,
        ),
        CastMemberType::Text(t) => (
            t.text.clone(),
            t.fixed_line_space,
            t.top_spacing,
            t.width as i32,
            t.info.as_ref().map(|i| i.scroll_top as i32).unwrap_or(0),
            t.word_wrap,
            t.font.clone(),
            t.font_size,
            Vec::new(),
            true,
        ),
        _ => return -1,
    };
    // Pull the cast lib's font_table snapshot before dropping the member
    // borrow — needed to resolve each formatting_run.font_id to a name
    // (Arial / Arial Bold / Arial Italic) for per-run advance lookups.
    let field_font_table: std::collections::HashMap<u16, String> = player
        .movie
        .cast_manager
        .get_cast(member_ref.cast_lib as u32)
        .map(|cl| cl.font_table.clone())
        .unwrap_or_default();
    // Explicitly release the immutable borrow on `player.movie` (held
    // through `member`) before we touch `player.font_manager` /
    // `player.bitmap_manager` mutably below.
    drop(member);
    let rect = score::get_sprite_rect_in_context(player, sprite_num);
    let local_x = mx - rect.0 as i32;
    let local_y = my - rect.1 as i32;
    if local_x < 0
        || local_y < 0
        || local_x >= (rect.2 - rect.0) as i32
        || local_y >= (rect.3 - rect.1) as i32
    {
        return -1;
    }
    // Shift the y-coord into the FULL text coordinate space (member-local
    // top of all text, not just the visible page). Without this, paged
    // fields return char indices from the first page even when the user
    // is looking at page 3+.
    let text_y = local_y + scroll_top;
    // Resolve the field's actual font via font_manager. The member borrow
    // was dropped above (we cloned everything into owned locals), so the
    // mutable borrow on font_manager + bitmap_manager is safe. Fall back
    // to the system font only when the named font isn't available.
    let font_arc = player.font_manager.get_font_with_cast_and_bitmap(
        &font_name,
        &player.movie.cast_manager,
        &mut player.bitmap_manager,
        if font_size > 0 { Some(font_size) } else { None },
        None,
    );
    let font_rc = match font_arc.or_else(|| player.font_manager.get_system_font()) {
        Some(f) => f,
        None => return -1,
    };
    let font: crate::player::font::BitmapFont = (*font_rc).clone();
    let min_space_adv = {
        let sz = font.font_size.max(font.char_height) as i32;
        let v = ((sz as f32) * 0.30).round() as i16;
        if v > 0 { Some(v) } else { None }
    };

    // Build per-character advances using the same per-run atlas the
    // renderer draws with. Without this, a field that mixes Arial body
    // text and Arial Bold underlined chunks (Fugue No.4 Narrative) would
    // have hit-test wrap with regular Arial widths everywhere while the
    // renderer wraps with Bold widths for the underlined runs. The
    // accumulated drift makes clicks on visual "Christ's passion" return
    // a char position deep into "the sign of the cross" / "three motives".
    let per_char_advances_vec: Option<Vec<i32>> = if !formatting_runs.is_empty() {
        // Map each char index to which formatting_run covers it.
        // Runs use BYTE positions; convert via char_indices.
        let chars_total = text.chars().count();
        let mut advances: Vec<i32> = Vec::with_capacity(chars_total);
        // Cache loaded variant atlases by canonical name so we don't
        // re-load Arial Bold once per run.
        let mut variant_cache: std::collections::HashMap<
            String,
            std::rc::Rc<crate::player::font::BitmapFont>,
        > = std::collections::HashMap::new();
        let min_sp = min_space_adv.unwrap_or(0) as i32;
        // Resolve each run's font once (lazy via cache).
        let resolve_font = |player: &mut DirPlayer,
                            cache: &mut std::collections::HashMap<
            String,
            std::rc::Rc<crate::player::font::BitmapFont>,
        >,
                            run_font_id: u16|
         -> Option<std::rc::Rc<crate::player::font::BitmapFont>> {
            let resolved_name = field_font_table.get(&run_font_id).cloned()?;
            if let Some(f) = cache.get(&resolved_name) {
                return Some(f.clone());
            }
            let f = player.font_manager.get_font_with_cast_and_bitmap(
                &resolved_name,
                &player.movie.cast_manager,
                &mut player.bitmap_manager,
                if font_size > 0 { Some(font_size) } else { None },
                None,
            )?;
            cache.insert(resolved_name, f.clone());
            Some(f)
        };
        // Walk chars, looking up which run covers each (by BYTE position).
        // Renderer-side scaling: each char's advance is multiplied by
        // `run.font_size / field.font_size` because the renderer loads
        // variant atlases at the FIELD's base size and scales when drawing
        // chars at oversize (24pt "Fugue No. 4" header runs render at 2×
        // the advance of the loaded-at-12 Arial Bold atlas). Without this
        // scaling, hit-test underestimates the width consumed by the
        // header runs, the header wraps differently than the renderer
        // drew it, and every body line below the header is shifted in
        // source-text content.
        let field_base_size = if font_size > 0 { font_size as i32 } else { 12 };
        let mut char_iter = text.char_indices().enumerate();
        // Pre-resolve a "default base" advance per char for fallback.
        while let Some((_ci, (byte_pos, c))) = char_iter.next() {
            // Find the active run for this byte position.
            let active_run = formatting_runs
                .iter()
                .rev()
                .find(|r| (r.start_position as usize) <= byte_pos);
            let run_font_opt =
                active_run.and_then(|r| resolve_font(player, &mut variant_cache, r.font_id));
            let run_font_for_char = run_font_opt.as_deref().unwrap_or(&font);
            let run_size = active_run
                .map(|r| {
                    if r.font_size >= 6 {
                        r.font_size as i32
                    } else {
                        field_base_size
                    }
                })
                .unwrap_or(field_base_size);
            let raw_atlas = run_font_for_char.get_char_advance(c as u8) as i32;
            // Mirror the renderer's `size_px / base_size` scale factor
            // applied per-char in the PFR multi-span draw loop.
            let raw =
                (raw_atlas * run_size / field_base_size.max(1)).max(if c == ' ' { 0 } else { 1 });
            let clamped = if c == ' ' { raw.max(min_sp) } else { raw };
            advances.push(clamped);
        }
        Some(advances)
    } else {
        None
    };

    // Per-char effective font_size (for variable-line-height hit-testing).
    // Needed because the Narrative field has a 24pt "Fugue No. 4" header
    // on a wrap-line that also carries 12pt content — the renderer makes
    // that visual line as tall as its max font_size (24px), while a
    // plain hit-test using a fixed line_h (12) drifts a full line below
    // the renderer's actual layout for every click on the body.
    let per_char_font_sizes_vec: Option<Vec<i16>> = if !formatting_runs.is_empty() {
        let chars_total = text.chars().count();
        let mut sizes: Vec<i16> = Vec::with_capacity(chars_total);
        let base_size = if font_size > 0 { font_size as i16 } else { 12 };
        for (byte_pos, _c) in text.char_indices() {
            let active_run = formatting_runs
                .iter()
                .rev()
                .find(|r| (r.start_position as usize) <= byte_pos);
            let s = active_run
                .map(|r| {
                    if r.font_size >= 6 {
                        r.font_size as i16
                    } else {
                        base_size
                    }
                })
                .unwrap_or(base_size);
            sizes.push(s);
        }
        Some(sizes)
    } else {
        None
    };

    // Variable-line-height hit-test. Walks chars, wraps using per-char
    // advances, finalizes each visual line's height as the max of the
    // per-char font_sizes (matching the renderer's `line.max_size` rule),
    // and finds the char at the target (local_x, text_y).
    let idx = if let (Some(advs), Some(sizes)) = (
        per_char_advances_vec.as_ref(),
        per_char_font_sizes_vec.as_ref(),
    ) {
        let wrap_max = if word_wrap && wrap_width > 0 {
            wrap_width as i32
        } else {
            i32::MAX
        };
        let base_size = if font_size > 0 { font_size as i32 } else { 12 };
        let chars_vec: Vec<char> = text.chars().collect();
        let mut line_start_idx: usize = 0;
        let mut line_w: i32 = 0;
        let mut last_space_idx_after: Option<usize> = None; // idx AFTER the space
        let mut last_space_w_at: i32 = 0;
        let mut line_y: i32 = top_spacing as i32;
        // Visual lines as (start_idx_inclusive, end_idx_exclusive, line_h)
        let mut visual_lines: Vec<(usize, usize, i32)> = Vec::new();
        let mut line_max_size: i32 = base_size;
        let finalize_line = |start: usize,
                             end: usize,
                             max_size: i32,
                             lines: &mut Vec<(usize, usize, i32)>,
                             line_y: &mut i32| {
            let line_h = max_size.max(base_size).max(1);
            lines.push((start, end, line_h));
            *line_y += line_h;
        };
        let mut ci: usize = 0;
        while ci < chars_vec.len() {
            let c = chars_vec[ci];
            let cs = sizes.get(ci).copied().unwrap_or(base_size as i16) as i32;
            line_max_size = line_max_size.max(cs);
            if c == '\r' || c == '\n' {
                finalize_line(
                    line_start_idx,
                    ci,
                    line_max_size,
                    &mut visual_lines,
                    &mut line_y,
                );
                ci += 1;
                line_start_idx = ci;
                line_w = 0;
                line_max_size = base_size;
                last_space_idx_after = None;
                last_space_w_at = 0;
                continue;
            }
            let cw = advs.get(ci).copied().unwrap_or(0);
            if line_w + cw > wrap_max && ci > line_start_idx {
                if let Some(sp) = last_space_idx_after.filter(|&sp| sp > line_start_idx) {
                    finalize_line(
                        line_start_idx,
                        sp,
                        line_max_size,
                        &mut visual_lines,
                        &mut line_y,
                    );
                    line_start_idx = sp;
                    line_w -= last_space_w_at;
                    last_space_idx_after = None;
                    last_space_w_at = 0;
                    line_max_size = base_size;
                    // Re-evaluate max for the wrapped tail (chars sp..ci).
                    for k in sp..=ci {
                        let ks = sizes.get(k).copied().unwrap_or(base_size as i16) as i32;
                        line_max_size = line_max_size.max(ks);
                    }
                }
            }
            line_w += cw;
            if c == ' ' {
                last_space_idx_after = Some(ci + 1);
                last_space_w_at = line_w;
            }
            ci += 1;
        }
        if line_start_idx < chars_vec.len() {
            finalize_line(
                line_start_idx,
                chars_vec.len(),
                line_max_size,
                &mut visual_lines,
                &mut line_y,
            );
        }
        // Find the visual line containing text_y, then the char in it.
        let mut chosen_idx = chars_vec.len().saturating_sub(1);
        let mut accum_y: i32 = top_spacing as i32;
        for &(start, end, line_h) in &visual_lines {
            if text_y >= accum_y && text_y < accum_y + line_h {
                // Walk x within this line.
                let mut x_accum: i32 = 0;
                let mut found = end.saturating_sub(1);
                for k in start..end {
                    let cw = advs.get(k).copied().unwrap_or(0);
                    if local_x < x_accum + cw {
                        found = k;
                        break;
                    }
                    x_accum += cw;
                    found = k;
                }
                chosen_idx = found;
                break;
            }
            accum_y += line_h;
        }
        chosen_idx
    } else {
        // Text members render via the native (Canvas2D) path, where the line
        // STEP is `fixedLineSpace` itself when set (see
        // render_native_text_to_bitmap: `effective_line_height = fixed_line_space`),
        // NOT `font_size + fixedLineSpace`. Passing line_spacing on top of the
        // font-derived line height (as fields/bitmap path want) double-counts
        // and makes the y→line mapping drift by a full item per few lines —
        // e.g. spectral-wizard's help_menu mapped a bottom-of-list hover to a
        // mid-list link. So for text members override line_height to the native
        // value and zero the extra spacing.
        let (line_height_override, eff_line_spacing) = if is_text {
            let lh: u16 = if line_spacing > 0 {
                line_spacing
            } else if font.font_size > 0 {
                font.font_size
            } else {
                font.char_height
            };
            (Some(lh), 0u16)
        } else {
            (None, line_spacing)
        };
        let params = crate::player::font::DrawTextParams {
            font: &font,
            line_height: line_height_override,
            line_spacing: eff_line_spacing,
            top_spacing,
            char_spacing: 0,
            member_width: if word_wrap && wrap_width > 0 {
                Some(wrap_width as i16)
            } else {
                None
            },
            min_space_advance: min_space_adv,
            per_char_advances: per_char_advances_vec.as_deref(),
        };
        crate::player::font::get_text_index_at_pos(&text, &params, local_x, text_y)
    };
    let total = text.chars().count();

    if idx >= total { -1 } else { (idx + 1) as i32 }
}

/// Tick the global SoundManager by `delta` seconds — advances fades
/// regardless of whether the frame loop is currently paused (e.g.
/// blocked on a Flash/Ruffle load). Without this, a `sound fadeIn`
/// started just before such a pause leaves the channel at gain=0 for
/// the entire blocked period: the audio source plays silently into a
/// zero-gain node and is gone by the time the frame loop resumes.
fn tick_sound_manager(delta: f64) {
    reserve_player_mut(|player| {
        // SoundManager::update needs the owning player for bookkeeping while
        // its channel storage is borrowed through the player field.
        let player_ptr = player as *mut DirPlayer;
        // SAFETY: the callback owns the sole mutable player borrow and update
        // only reborrows the disjoint sound-manager channel state.
        unsafe {
            let _ = player.sound_manager.update(delta, &mut *player_ptr);
        }
    });
}

/// Wait until `deadline_ms`, with the 1 ms floor the browser event loop needs.
async fn wait_until_deadline(deadline_ms: f64) {
    let remaining_ms = (deadline_ms - bench_now_ms()).max(1.0);
    let _ = timeout(
        Duration::from_millis(remaining_ms.ceil() as u64),
        future::pending::<()>(),
    )
    .await;
}

pub async fn run_frame_loop() {
    unsafe {
        let player = crate::player::player_ref();
        if !player.is_playing {
            return;
        }
    }

    let generation = unsafe { PLAYER_GENERATION };
    let mut is_playing = true;
    // Absolute pacing deadline, carried across frames. See the wait at the end
    // of the loop for why this is not recomputed from scratch each frame.
    let mut next_deadline_ms = bench_now_ms();
    while is_playing {
        // Exit if the player was reset (e.g. between tests)
        if unsafe { PLAYER_GENERATION } != generation {
            return;
        }
        // Restart (`play movie <the current movie>`). Done HERE — between frames,
        // no active bytecode — so it's safe to rebuild the cast. Re-parses the
        // retained movie bytes and runs the full load+init (fresh W3D scenes etc.),
        // preserving globals + external params like Director's `play movie`.
        let do_restart = reserve_player_ref(|player| player.pending_restart);
        if do_restart {
            restart_current_movie().await;
            (is_playing, _) =
                reserve_player_ref(|player| (player.is_playing, player.is_script_paused));
            continue;
        }
        // Check for pending gotoNetMovie completion
        let goto_transition = reserve_player_ref(|player| {
            if let Some((task_id, ref target)) = player.pending_goto_net_movie {
                if player.net_manager.is_task_done(Some(task_id)) {
                    Some((task_id, target.clone()))
                } else {
                    None
                }
            } else {
                None
            }
        });

        if let Some((task_id, target)) = goto_transition {
            transition_to_net_movie(task_id, target).await;
            (is_playing, _) =
                reserve_player_ref(|player| (player.is_playing, player.is_script_paused));
            continue;
        }

        // A movie was mounted EAGERLY by go(frame, movie) while a handler was
        // executing. Once that handler stack has fully unwound, run the new
        // movie's init sequence (prepareMovie/startMovie/…) and release the
        // retired cast libraries the suspended frames were pointing into.
        let run_pending_init = reserve_player_ref(|player| {
            player.pending_movie_init && player.handler_stack_depth == 0
        });
        if run_pending_init {
            reserve_player_mut(|player| {
                player.pending_movie_init = false;
            });
            run_movie_init_sequence().await;
            reserve_player_mut(|player| {
                player.is_in_transition = false;
                player.retired_cast_libs.clear();
            });
            (is_playing, _) =
                reserve_player_ref(|player| (player.is_playing, player.is_script_paused));
            continue;
        }

        // Pre-dispatch Flash members for sprites on the current frame so they start
        // loading before Lingo scripts try to access them.
        let flash_actions = reserve_player_mut(|player| {
            player
                .pre_dispatch_flash_members()
                .map(|_| player.take_flash_host_actions())
        });
        if let Err(error) = flash_actions.and_then(emit_flash_host_actions) {
            warn!("Flash host preparation failed: {}", error.message);
        }

        // Wait for pending Flash/Ruffle instances to finish loading BEFORE running scripts.
        if is_flash_loading().unwrap_or(false) {
            debug!("[Flash] Waiting for Ruffle instance to finish loading...");
            for _ in 0..150 {
                timeout(Duration::from_millis(100), future::pending::<()>())
                    .await
                    .unwrap_err();
                // Tick sound manager during the wait: a 1.6s applause
                // with `sound fadeIn` started right before this pause
                // would otherwise stay at gain=0 for its entire
                // duration (frame loop is blocked, so the fade ramp
                // never runs). 0.1s matches the wait granularity.
                tick_sound_manager(0.1);
                if !is_flash_loading().unwrap_or(false) {
                    break;
                }
            }
            debug!("[Flash] Ruffle instance ready, resuming frame loop.");
        }

        // Run one frame cycle (scripts + advance)
        let (playing, _) = run_single_frame().await;
        is_playing = playing;

        if !is_playing {
            stop_movie_sequence().await;
            return;
        }

        // Dispatch the movie-level `idle` event. Director sends `idle`
        // continuously while the playhead waits on a frame, and many movies —
        // especially Director 4 titles like thead's playback Controller —
        // drive ALL of their animation from `on idle` in a movie script
        // (stepping sprite castNums each tick). Without it nothing animates.
        // Goes to the frame script + movie scripts (Director's idle hierarchy);
        // a no-op for movies that define no `idle` handler.
        {
            let active = reserve_player_ref(|player| {
                player.is_playing && !player.is_script_paused && !player.pending_movie_init
            });
            if active {
                if let Err(err) = player_invoke_frame_and_movie_scripts(
                    Symbol::builtin(BuiltInSymbol::Idle),
                    &vec![],
                )
                .await
                {
                    warn!("idle dispatch failed: {}", err.message);
                }
            }
        }

        // Tick the sound manager so per-channel fade-in / fade-out
        // progress. Without this, Lingo's `sound fadeIn` sets the
        // channel volume to 0 to start the fade but the ramp never
        // runs — the audio gets scheduled into a gain=0 node and plays
        // silently (storyscramble's `playPayoff` applause uses fadeIn
        // via the soundLoop script). Delta is the per-frame tempo
        // interval in seconds; matches what the SoundChannel::update
        // fade math expects.
        {
            let tempo = reserve_player_ref(|player| player.current_frame_tempo);
            let delta = if tempo == 0 {
                1.0 / 30.0
            } else {
                1.0 / tempo as f64
            };
            tick_sound_manager(delta);
        }

        // Drain cue-point events SoundChannel::update detected. The handler
        // dispatch is async — has to happen out here in the frame loop, not
        // inside the synchronous sound tick. Pass each as
        // `on cuePassed channelSymbol, number, name` per Director 11.5 spec
        // (channel arg is a symbol like `#sound1`). Dispatched to frame +
        // movie scripts in order.
        loop {
            let events =
                reserve_player_mut(|player| std::mem::take(&mut player.pending_cue_events));
            if events.is_empty() {
                break;
            }
            for (channel_num, cue_number, cue_name) in events {
                let Some(session) = retained_session_handle() else {
                    break;
                };
                let player_id = active_player_id() as u32;
                let Some((args, cue_symbol)) =
                    session.borrow_mut().with_player(player_id, |mut context| {
                        let chan_symbol = context.symbols.intern(&format!("sound{}", channel_num));
                        let chan_sym = context
                            .player
                            .alloc_datum(crate::director::lingo::datum::Datum::Symbol(chan_symbol));
                        let number_ref = context
                            .player
                            .alloc_datum(crate::director::lingo::datum::Datum::Int(cue_number));
                        let name_ref = context
                            .player
                            .alloc_datum(crate::director::lingo::datum::Datum::String(cue_name));
                        (
                            vec![chan_sym, number_ref, name_ref],
                            context.symbols.intern("cuePassed"),
                        )
                    })
                else {
                    break;
                };
                if let Err(err) = player_invoke_frame_and_movie_scripts(cue_symbol, &args).await {
                    warn!("cuePassed dispatch failed: {}", err.message);
                }
            }
        }

        // Also check after frame execution: if scripts tried to access a Flash
        // instance that doesn't exist yet, wait for Ruffle to finish loading.
        if is_flash_loading().unwrap_or(false) {
            debug!("[Flash] Scripts accessed unready Flash instance, waiting...");
            for _ in 0..150 {
                timeout(Duration::from_millis(100), future::pending::<()>())
                    .await
                    .unwrap_err();
                tick_sound_manager(0.1);
                if !is_flash_loading().unwrap_or(false) {
                    break;
                }
            }
            debug!("[Flash] Ruffle instance ready, resuming frame loop.");
        }

        // Re-read the tempo every frame. `begin_all_sprites` only runs on a frame
        // CHANGE, so without this a movie holding one frame (`go the frame`) could
        // never see a `puppetTempo()` it set itself. See `refresh_frame_tempo`.
        reserve_player_mut(|player| player.refresh_frame_tempo());

        // Get the target frame delay based on the tempo for the current frame
        let target_delay_ms = reserve_player_ref(|player| {
            let tempo = player.current_frame_tempo;
            let base = if tempo == 0 {
                1000.0 / 30.0 // Default to 30fps if tempo is 0
            } else {
                1000.0 / tempo as f64
            };
            // While an async net fetch is in flight, use a longer per-frame
            // yield (>= ~12 ms) so the browser's fetch/stream gets enough
            // event-loop time to complete. A tight high-tempo loop
            // (DGS puppetTempo(999) ≈ 1 ms/frame) that calls into Flash every
            // frame (showGameLoadStats) otherwise starves the fetch, so
            // gameLoaded()/netDone never turns true and the loader hangs on its
            // loading frame — only a debugger pause (which yields the event
            // loop) unstuck it. The slowdown lasts only while a task loads.
            // Also cover Ruffle-side browser fetches (LoadVars/URLLoader/XML)
            // that dirplayer's net_manager never sees: flashPlayerManager's
            // fetch monkey-patch publishes the count of outstanding requests as
            // `window.__dirplayerPendingNetCount`. The DGS loader spins at load
            // state 81 waiting on the preloader's translation POST (Ruffle-side,
            // 1-3 s) to set `preloaderTranslationSuccess`; without this the tight
            // loop starves that fetch's completion callback and the login links
            // (asfunction hyperlinks in the translated IDS strings) never load.
            let read_window_count = |key: &str| -> f64 {
                web_sys::window()
                    .and_then(|w| {
                        js_sys::Reflect::get(&w, &wasm_bindgen::JsValue::from_str(key)).ok()
                    })
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0)
            };
            let browser_fetch_pending = read_window_count("__dirplayerPendingNetCount") > 0.0;
            // Offscreen Ruffle instances self-tick via requestAnimationFrame; a
            // tight high-tempo Director loop (DGS puppetTempo(999) guest-gate
            // poll at load-state 350) hogs the main thread and starves those
            // ticks, so a preloader text field whose htmlText was just updated
            // (the login links) never re-renders — the movie only worked with a
            // debugger pause, which yields the loop. Floor the per-frame yield
            // to ~one RAF interval while any Flash sprite is on stage AND the
            // movie is spinning faster than 60fps (base < 16ms), so normal-tempo
            // Flash playback is unaffected but a busy-poll can't starve Ruffle.
            let flash_active = read_window_count("__dirplayerActiveFlashCount") > 0.0;
            let delay = if player.net_manager.has_in_progress_tasks() || browser_fetch_pending {
                base.max(25.0)
            } else if flash_active {
                base.max(16.0)
            } else {
                base
            };
            delay
        });

        // DEADLINE pacing, not a fixed sleep, and the movie's tempo paces every
        // frame whether or not assets are loading.
        //
        // Director's measured model (`docs/fps-probe`, captured from both the
        // authoring environment and the browser plugin) is exactly
        //
        //     frame_period = max(1000 / tempo, work)
        //
        // — the tempo is a CEILING. Director runs the frame and then burns only
        // what is LEFT of the period; past the deadline it runs flat out with no
        // added penalty (busy 40 ms at tempo 30 measured frame 40.0 ms = 25.0 fps
        // exactly). Sleeping the WHOLE period after the work instead made our real
        // period `work + period`, so fps could never reach the tempo and the gap
        // widened with frame cost — at tempo 30 with 20 ms of work Shockwave gives
        // 30 fps and we gave ~18.8.
        //
        // (Removed at the same time: a "phase-aware pacing" override from 7f38a47a
        // that forced a 1 ms yield while casts were loading. It was a real
        // load-time win, but it silently overrode the authored tempo — including a
        // script's own puppetTempo — and took precedence over the fetch/Flash
        // floors above, so a load could starve the very fetches those protect.)
        //
        // Floor at 1 ms rather than 0: this is a browser, and the loop must always
        // hand control back or network, input and rendering starve. Note a
        // setTimeout-backed sleep is additionally clamped to ~4 ms once timer
        // nesting exceeds 5 levels, capping us near 250 fps regardless — that is
        // irrelevant below tempo 250, and is why we cannot match Shockwave's
        // measured 1000 fps at tempo 999 without a different yield primitive.
        //
        // The deadline is ABSOLUTE and carried across frames rather than derived
        // from a fresh `now` each time. Computing `period - spent` per frame loses
        // whatever `.ceil()` rounds up plus however late the timer actually fires,
        // and that error repeats every frame instead of being absorbed: measured
        // +1.4 ms/frame (frame 18.10 ms against a 16.70 ms period at tempo 60,
        // 9.70 against 8.30 at tempo 120), i.e. 26.6 fps at tempo 30 where
        // Shockwave gives 29.8. Advancing a running deadline lets a frame that
        // came back early pay for one that ran late.

        let now_ms = bench_now_ms();
        next_deadline_ms += target_delay_ms;
        // Already past it - a frame overran, or the tempo just dropped. Do NOT
        // bank that debt and then sprint to repay it with a burst of zero-length
        // frames; Director simply runs flat out and starts the next period from
        // now (its measured model is `frame = max(period, work)`, with no penalty
        // for having missed). Resync.
        if next_deadline_ms < now_ms {
            next_deadline_ms = now_ms;
        }
        wait_until_deadline(next_deadline_ms).await;

        player_wait_available().await;
    }
}

pub async fn player_trigger_breakpoint(
    breakpoint: Breakpoint,
    script_ref: CastMemberRef,
    handler_ref: ScriptHandlerRef,
    bytecode_index: usize,
) {
    let (future, completer) = ManualFuture::new();
    let breakpoint_ctx = BreakpointContext {
        breakpoint,
        script_ref,
        handler_ref,
        bytecode_index,
        completer,
        error: None,
    };
    reserve_player_mut(|player| {
        player.current_breakpoint = Some(breakpoint_ctx);
        player.pause_script();
        JsApi::dispatch_scope_list(player);
    });
    future.await;
    reserve_player_mut(|player| {
        player.resume_script();
    });
}

pub async fn player_is_playing() -> bool {
    unsafe { crate::player::player_ref().is_playing }
}

pub(crate) static mut PLAYER_TX: Option<Sender<PlayerVMExecutionItem>> = None;
static mut PLAYER_EVENT_TX: Option<Sender<PlayerVMEvent>> = None;
pub static mut PLAYER_OPT: Option<DirPlayer> = None;
thread_local! {
    /// Retained production session handle. Interior mutability is confined to
    /// the single player executor thread; no mutable static alias is exposed.
    pub(crate) static PLAYER_SESSION_HANDLE: RefCell<Option<Rc<RefCell<RuntimeSession>>>> = const { RefCell::new(None) };
    #[cfg(test)]
    static TEST_SCRIPT_ERROR_COUNT: Cell<u32> = const { Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_test_script_error_count() {
    TEST_SCRIPT_ERROR_COUNT.with(|count| count.set(0));
}

#[cfg(test)]
pub(crate) fn test_script_error_count() -> u32 {
    TEST_SCRIPT_ERROR_COUNT.with(|count| count.get())
}
// pub static mut PLAYER_NAMES: Option<lasso::Rodeo> = None;

/// Generation counter incremented each time the player is reset (e.g. between
/// tests). Long-running spawned tasks (frame loop, command loop) capture the
/// generation at spawn time and exit when it becomes stale.
pub(crate) static mut PLAYER_GENERATION: u64 = 0;

pub fn player_semaphone() -> &'static Mutex<()> {
    static MAP: OnceLock<Mutex<()>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(()))
}

// pub fn get_name_string(name: &lasso::Spur) -> &'static str {
//     unsafe {
//         PLAYER_NAMES
//             .as_ref()
//             .unwrap()
//             .resolve(name)
//     }
// }

// pub fn get_name_spur(name: &str) -> lasso::Spur {
//     unsafe {
//         PLAYER_NAMES
//             .as_mut()
//             .unwrap()
//             .get_or_intern(name)
//     }
// }

pub fn init_player() {
    console_log::init_with_level(log::Level::Error).unwrap_or(());
    let (tx, rx) = channel::unbounded();
    let (event_tx, event_rx) = channel::unbounded();
    unsafe {
        PLAYER_TX = Some(tx.clone());
        PLAYER_EVENT_TX = Some(event_tx.clone());
    }

    unsafe {
        // Production owns the host player in the retained RuntimeSession so
        // evaluator continuations and handler drivers share one graph.
        let mut session = RuntimeSession::new(SymbolOwner {
            session: 1,
            generation: 1,
        });
        assert!(session.add_player(0, tx.clone()));
        PLAYER_SESSION_HANDLE.with(|slot| {
            *slot.borrow_mut() = Some(Rc::new(RefCell::new(session)));
        });
        PLAYER_OPT = None;
        // PLAYER_NAMES = Some(lasso::Rodeo::default());
    }
    let command_session =
        retained_session_handle().expect("init_player must retain RuntimeSession");
    let command_owner = command_session
        .borrow_mut()
        .with_player(0, |context| context.player.owner.clone())
        .expect("init_player must retain main player");
    // let mut player = //PLAYER_LOCK.try_write().unwrap();
    // *player = Some(DirPlayer::new(tx, allocator_rx, allocator_tx));

    let event_session = command_session.clone();
    let event_owner = command_owner.clone();
    crate::player::spawn_player_local(async move {
        // player_load_system_font().await;
        crate::player::spawn_player_local(async move {
            run_command_loop(rx, command_session, 0, command_owner).await;
        });
        crate::player::spawn_player_local(async move {
            run_event_loop(event_rx, event_session, 0, event_owner).await;
        });
    });
}

fn get_active_static_script_refs(player: &DirPlayer) -> Vec<CastMemberRef> {
    let movie = &player.movie;
    let frame_script = movie.score.get_script_in_frame(movie.current_frame);
    let movie_scripts = movie.cast_manager.get_movie_scripts();
    let movie_scripts = movie_scripts.as_ref().unwrap();

    let mut active_script_refs: Vec<CastMemberRef> = vec![];
    for script in movie_scripts {
        active_script_refs.push(script.member_ref.clone());
    }
    if let Some(frame_script) = frame_script {
        active_script_refs.push(CastMemberRef {
            cast_lib: frame_script.cast_lib.into(),
            cast_member: frame_script.cast_member.into(),
        });
    }
    // Resolve global *script* refs directly from `globals` (rare) instead of
    // building a full hydrated-globals HashMap on every ext_call — this runs on
    // the hottest builtin-dispatch path (voidp/offset/length in replaceChunks).
    for global_ref in player.globals.values() {
        if let Datum::VarRef(VarRef::Script(script_ref)) = player.get_datum(global_ref) {
            active_script_refs.push(script_ref.clone());
        }
    }
    return active_script_refs;
}

// #[allow(dead_code)]
// fn get_active_scripts<'a>(
//   movie: &'a Movie,
//   globals: &'a HashMap<String, Datum>
// ) -> Vec<&'a Rc<Script>> {
//   let frame_script = movie.score.get_script_in_frame(movie.current_frame);
//   let mut movie_scripts = movie.cast_manager.get_movie_scripts();

//   let mut active_scripts: Vec<&Rc<Script>> = vec![];
//   active_scripts.append(&mut movie_scripts);
//   if let Some(frame_script) = frame_script {
//     let script = movie.cast_manager
//       .get_cast(frame_script.cast_lib as u32)
//       .unwrap()
//       .get_script_for_member(frame_script.cast_member.into())
//       .unwrap();
//     active_scripts.push(script);
//   }
//   for global in globals.values() {
//     if let Datum::VarRef(VarRef::Script(script_ref)) = global {
//       active_scripts.push(
//         movie.cast_manager.get_script_by_ref(script_ref).unwrap()
//       );
//     }
//   }
//   return active_scripts;
// }

fn player_duplicate_datum(
    player: &mut DirPlayer,
    symbols: &crate::player::symbols::symbol_table::SymbolTable,
    datum: &DatumRef,
) -> Result<DatumRef, ScriptError> {
    crate::player::datum_duplicate::duplicate_datum(player, symbols, datum)
}

// ---------------------------------------------------------------------------
// Interpreter throughput benchmark (shared by the native unit test and the
// `bench_bytecode_throughput` wasm export, so we measure the SAME thing on the
// real target — the browser — not just native).
// ---------------------------------------------------------------------------

/// Monotonic clock in fractional milliseconds for the benchmark.
#[cfg(target_arch = "wasm32")]
pub(crate) fn bench_now_ms() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map(|p| p.now())
        .unwrap_or(0.0)
}
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn bench_now_ms() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static BASE: OnceLock<Instant> = OnceLock::new();
    BASE.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

/// A minimal valid Script. `player_execute_bytecode`/`try_execute_bytecode_sync`
/// retain a real, empty Script even though the benchmark ops never read its
/// contents.
fn bench_minimal_script() -> crate::player::script::Script {
    use crate::director::chunks::script::ScriptChunk;
    use crate::director::enums::ScriptType;
    use crate::player::cast_lib::CastMemberRef;
    use crate::player::script::Script;
    use std::cell::RefCell;
    use std::collections::HashMap;
    Script {
        member_ref: CastMemberRef {
            cast_lib: 0,
            cast_member: 0,
        },
        name: String::new(),
        chunk: ScriptChunk {
            script_number: 0,
            literals: vec![],
            handlers: vec![],
            property_name_ids: vec![],
            property_defaults: HashMap::new(),
        },
        script_type: ScriptType::Movie,
        handlers: fxhash::FxHashMap::default(),
        handler_names_raw: vec![],
        handler_names: vec![],
        properties: RefCell::new(fxhash::FxHashMap::default()),
    }
}

/// Run a synthetic bytecode-throughput benchmark against the live interpreter
/// and return a human-readable report. Each measured run owns its
/// `RuntimeSession` and player, so the benchmark exercises the same explicit
/// context boundary as production execution. All ops used are synchronous,
/// so this drives the real `try_execute_bytecode_sync` fast path + advance
/// loop without async.
pub fn run_bytecode_benchmark() -> String {
    use crate::director::chunks::handler::{Bytecode, HandlerDef};
    use crate::director::lingo::opcode::OpCode;
    use crate::player::bytecode::handler_manager::{BytecodeHandlerContext, try_execute_bytecode_sync};
    use crate::player::symbols::symbol::Symbol;

    fn run(bytecode: Vec<Bytecode>) -> (usize, f64) {
        let mut session = crate::player::session::RuntimeSession::new(
            crate::player::symbols::symbol_table::SymbolOwner {
                session: 41,
                generation: 1,
            },
        );
        assert!(session.add_player(1, async_std::channel::unbounded().0));
        let total_ops = bytecode.len();
        let handler = HandlerDef {
            name_id: 0,
            bytecode_array: bytecode,
            bytecode_index_map: fxhash::FxHashMap::default(),
            argument_name_ids: vec![],
            local_name_ids: vec![],
            global_name_ids: vec![],
            compiled_ir: std::cell::RefCell::new(None),
        };
        let names: Rc<[Symbol]> = Rc::from(Vec::<Symbol>::new());
        let script = bench_minimal_script();
        let (scope_ref, scope_owner, scope_generation, scope_epoch) = session
            .with_player(1, |mut runtime| {
                let scope_ref = runtime.player.push_scope();
                let scope_generation = runtime.player.scopes[scope_ref].generation;
                (
                    scope_ref,
                    runtime.player.owner.clone(),
                    scope_generation,
                    runtime.player.scope_invalidation_epoch,
                )
            })
            .unwrap();
        let ctx = BytecodeHandlerContext {
            scope: ScopeToken {
                owner: scope_owner,
                slot: scope_ref,
                generation: scope_generation,
                epoch: scope_epoch,
            },
            code: HandlerCode {
                script: Rc::new(script),
                handler: Rc::new(handler),
                names,
            },
            multiplier: 1,
        };

        let start = bench_now_ms();
        loop {
            let step = session
                .with_player(1, |mut runtime| {
                    try_execute_bytecode_sync(&mut runtime, &ctx)
                })
                .and_then(|result| result);
            match step {
                Some(Ok(HandlerExecutionResult::Advance)) => {
                    let done = session
                        .with_player(1, |runtime| {
                            let scope = runtime.player.scopes.get_mut(scope_ref).unwrap();
                            if scope.bytecode_index + 1 >= total_ops {
                                true
                            } else {
                                scope.bytecode_index += 1;
                                false
                            }
                        })
                        .unwrap();
                    if done {
                        break;
                    }
                }
                // Stop / Jump / Err / async-op(None) — none expected mid-run for
                // this synchronous flat handler; stop the loop.
                _ => break,
            }
        }
        let elapsed_ms = bench_now_ms() - start;
        session
            .with_player(1, |runtime| runtime.player.pop_scope())
            .unwrap();
        (total_ops, elapsed_ms)
    }

    // Variable-op runner: getlocal + pop. Exercises the cached `ctx.multiplier`
    // and the locals lookup (representative of the preloader's hot variable ops).
    fn run_getlocal(n_pairs: usize) -> (usize, f64) {
        let mut session = crate::player::session::RuntimeSession::new(
            crate::player::symbols::symbol_table::SymbolOwner {
                session: 42,
                generation: 1,
            },
        );
        assert!(session.add_player(1, async_std::channel::unbounded().0));
        let mut bc = Vec::with_capacity(n_pairs * 2);
        for i in 0..n_pairs {
            bc.push(Bytecode::new(OpCode::GetLocal, 0, i * 2)); // slot 0 (obj/mult = 0)
            bc.push(Bytecode::new(OpCode::Pop, 1, i * 2 + 1));
        }
        let total_ops = bc.len();
        let handler = HandlerDef {
            name_id: 0,
            bytecode_array: bc,
            bytecode_index_map: fxhash::FxHashMap::default(),
            argument_name_ids: vec![],
            local_name_ids: vec![0], // one local: slot 0 -> name_id 0
            global_name_ids: vec![],
            compiled_ir: std::cell::RefCell::new(None),
        };
        let names: Rc<[Symbol]> = Rc::from(Vec::<Symbol>::new());
        let script = bench_minimal_script();
        let (scope_ref, scope_owner, scope_generation, scope_epoch) = session
            .with_player(1, |mut runtime| {
                let s = runtime.player.push_scope();
                let d = runtime
                    .player
                    .alloc_datum(crate::director::lingo::datum::Datum::Int(1));
                runtime
                    .player
                    .scopes
                    .get_mut(s)
                    .unwrap()
                    .set_local(0, crate::player::scope::StackDatum::Ref(d));
                (
                    s,
                    runtime.player.owner.clone(),
                    runtime.player.scopes[s].generation,
                    runtime.player.scope_invalidation_epoch,
                )
            })
            .unwrap();
        let ctx = BytecodeHandlerContext {
            scope: ScopeToken {
                owner: scope_owner,
                slot: scope_ref,
                generation: scope_generation,
                epoch: scope_epoch,
            },
            code: HandlerCode {
                script: Rc::new(script),
                handler: Rc::new(handler),
                names,
            },
            multiplier: 1,
        };
        let start = bench_now_ms();
        loop {
            let step = session
                .with_player(1, |mut runtime| {
                    try_execute_bytecode_sync(&mut runtime, &ctx)
                })
                .and_then(|result| result);
            match step {
                Some(Ok(HandlerExecutionResult::Advance)) => {
                    let done = session
                        .with_player(1, |runtime| {
                            let scope = runtime.player.scopes.get_mut(scope_ref).unwrap();
                            if scope.bytecode_index + 1 >= total_ops {
                                true
                            } else {
                                scope.bytecode_index += 1;
                                false
                            }
                        })
                        .unwrap();
                    if done {
                        break;
                    }
                }
                _ => break,
            }
        }
        let elapsed_ms = bench_now_ms() - start;
        session
            .with_player(1, |runtime| runtime.player.pop_scope())
            .unwrap();
        (total_ops, elapsed_ms)
    }

    fn line(name: &str, ops: usize, ms: f64) -> String {
        let ops_per_sec = if ms > 0.0 {
            ops as f64 / (ms / 1000.0)
        } else {
            0.0
        };
        let ns_per_op = if ops > 0 {
            ms * 1_000_000.0 / ops as f64
        } else {
            0.0
        };
        format!(
            "{name:<22} {ops:>9} ops in {ms:>7.1}ms  =>  {ops_per_sec:>10.0} ops/sec  ({ns_per_op:>6.1} ns/op)"
        )
    }

    // Warm up (codegen, pool init) so the timed runs are steady.
    {
        let mut warm = Vec::with_capacity(4000);
        for i in 0..2000 {
            warm.push(Bytecode::new(OpCode::PushInt8, 1, i * 2));
            warm.push(Bytecode::new(OpCode::Pop, 1, i * 2 + 1));
        }
        let _ = run(warm);
    }

    let mut out = String::from("[interp bench]\n");

    // A) baseline dispatch floor: pooled int push + pop.
    const PAIRS: usize = 500_000;
    let mut a = Vec::with_capacity(PAIRS * 2);
    for i in 0..PAIRS {
        a.push(Bytecode::new(OpCode::PushInt8, 1, i * 2));
        a.push(Bytecode::new(OpCode::Pop, 1, i * 2 + 1));
    }
    let (ops, ms) = run(a);
    out.push_str(&line("pushint8+pop", ops, ms));
    out.push('\n');

    // B) heaviest preloader op: build a 4-arg arglist then drop it.
    const CYCLES: usize = 150_000;
    let mut b = Vec::with_capacity(CYCLES * 6);
    let mut pos = 0usize;
    for _ in 0..CYCLES {
        for _ in 0..4 {
            b.push(Bytecode::new(OpCode::PushInt8, 1, pos));
            pos += 1;
        }
        b.push(Bytecode::new(OpCode::PushArgList, 4, pos));
        pos += 1;
        // Discard the marker AND its 4 arguments. `pusharglist` no longer pops
        // the arguments — it leaves them in place and pushes a marker, which the
        // following call opcode consumes together with them. A bare `Pop 1` here
        // would leak 4 entries per cycle (600k over the run) and the bench would
        // be measuring stack growth.
        b.push(Bytecode::new(OpCode::Pop, 5, pos));
        pos += 1;
    }
    let (ops, ms) = run(b);
    out.push_str(&line("pusharglist(4)+pop", ops, ms));
    out.push('\n');

    // C) variable op: getlocal + pop (uses cached ctx.multiplier + locals lookup).
    let (ops, ms) = run_getlocal(500_000);
    out.push_str(&line("getlocal+pop", ops, ms));
    out.push('\n');

    // C2) inline arithmetic: push,push,add,pop. With inline-aware `add` the two
    //     operands and the sum never touch the arena — only the final pop
    //     materializes (pooled). Compare against the pre-inline cost (3 allocs +
    //     2 get_datum + add_datums per cycle).
    const ARITH: usize = 150_000;
    let mut c2 = Vec::with_capacity(ARITH * 4);
    let mut pos = 0usize;
    for _ in 0..ARITH {
        c2.push(Bytecode::new(OpCode::PushInt8, 3, pos));
        pos += 1;
        c2.push(Bytecode::new(OpCode::PushInt8, 4, pos));
        pos += 1;
        c2.push(Bytecode::new(OpCode::Add, 0, pos));
        pos += 1;
        c2.push(Bytecode::new(OpCode::Pop, 1, pos));
        pos += 1;
    }
    let (ops, ms) = run(c2);
    out.push_str(&line("push+push+add+pop", ops, ms));
    out.push('\n');

    // C3) inline compare: push,push,lt,pop (the canonical loop-guard shape).
    let mut c3 = Vec::with_capacity(ARITH * 4);
    let mut pos = 0usize;
    for _ in 0..ARITH {
        c3.push(Bytecode::new(OpCode::PushInt8, 1, pos));
        pos += 1;
        c3.push(Bytecode::new(OpCode::PushInt8, 9, pos));
        pos += 1;
        c3.push(Bytecode::new(OpCode::Lt, 0, pos));
        pos += 1;
        c3.push(Bytecode::new(OpCode::Pop, 1, pos));
        pos += 1;
    }
    let (ops, ms) = run(c3);
    out.push_str(&line("push+push+lt+pop", ops, ms));
    out.push('\n');

    // ---- Datum-inlining feasibility (step 0) ----
    // D) Isolated cost of the datum machinery for a primitive: alloc_datum(Int)
    //    (pooled fast path) + DatumRef drop, with NO dispatch/scope/advance.
    //    This is the upper bound on what inlining primitives onto the operand
    //    stack could remove per push.
    {
        use crate::director::lingo::datum::Datum;
        let n = 1_000_000usize;
        let mut session = crate::player::session::RuntimeSession::new(
            crate::player::symbols::symbol_table::SymbolOwner {
                session: 43,
                generation: 1,
            },
        );
        assert!(session.add_player(1, async_std::channel::unbounded().0));
        let mut sink = 0usize;
        let start = bench_now_ms();
        session
            .with_player(1, |runtime| {
                for i in 0..n {
                    // Small ints are pooled (ref_count = MAX), so this remains the
                    // same allocation/drop workload without an ambient player.
                    let r = runtime.player.alloc_datum(Datum::Int((i % 200) as i32));
                    sink = sink.wrapping_add(r.unwrap());
                    drop(r);
                }
            })
            .unwrap();
        let ms = bench_now_ms() - start;
        out.push_str(&line("alloc_datum(Int)+drop", n, ms));
        out.push_str(&format!("   [sink={}]\n", sink & 1));
    }

    // E) Inline baseline: push/pop a primitive held INLINE on a Vec (what the
    //    redesign would do). Delta (D - E) ~= per-push saving from inlining.
    {
        enum Sv {
            Int(i32),
        }
        let n = 1_000_000usize;
        let mut stack: Vec<Sv> = Vec::with_capacity(64);
        let mut sink = 0i64;
        let start = bench_now_ms();
        for i in 0..n {
            stack.push(Sv::Int(i as i32));
            if let Some(Sv::Int(v)) = stack.pop() {
                sink = sink.wrapping_add(v as i64);
            }
        }
        let ms = bench_now_ms() - start;
        out.push_str(&line("inline push/pop", n, ms));
        out.push_str(&format!("   [sink={}]\n", sink & 1));
    }

    // ---- Register-IR PoC (Stage 2): same workloads via the compiled IR ----
    // Go/no-go: does compiling basic ops to a dense-register IR (native-stack
    // operand stack + locals, pre-decoded ops, no reserve_player / scope fetch /
    // HashMap) beat the interpreter? Compare these to the interpreter lines above.
    {
        use crate::player::compiled;

        // push+push+add+pop, compiled from bytecode then run once (with a Ret).
        const ARITH_IR: usize = 150_000;
        let mut bc = Vec::with_capacity(ARITH_IR * 4 + 1);
        let mut pos: usize = 0;
        for _ in 0..ARITH_IR {
            bc.push(Bytecode::new(OpCode::PushInt8, 3, pos));
            pos += 1;
            bc.push(Bytecode::new(OpCode::PushInt8, 4, pos));
            pos += 1;
            bc.push(Bytecode::new(OpCode::Add, 0, pos));
            pos += 1;
            bc.push(Bytecode::new(OpCode::Pop, 1, pos));
            pos += 1;
        }
        bc.push(Bytecode::new(OpCode::Ret, 0, pos));
        let total = bc.len() - 1;
        let handler = HandlerDef {
            name_id: 0,
            bytecode_array: bc,
            bytecode_index_map: fxhash::FxHashMap::default(),
            argument_name_ids: vec![],
            local_name_ids: vec![],
            global_name_ids: vec![],
            compiled_ir: std::cell::RefCell::new(None),
        };
        let compiled_h = compiled::compile(&handler, 1).expect("eligible");
        let start = bench_now_ms();
        let _ = compiled::run(&compiled_h, &[]);
        let ms = bench_now_ms() - start;
        out.push_str(&line("IR push+push+add+pop", total, ms));
        out.push('\n');

        // Loop-counter pattern (str_test's actual culprit): `repeat with j=1 to N`,
        // built directly as IR. ~9 executed ops per iteration.
        let n: i32 = 1_000_000;
        let ops = vec![
            compiled::IrOp::PushInt(1),    // 0
            compiled::IrOp::SetLocal(0),   // 1   j = 1
            compiled::IrOp::GetLocal(0),   // 2   cond: j
            compiled::IrOp::PushInt(n),    // 3         N
            compiled::IrOp::LtEq,          // 4         j <= N
            compiled::IrOp::JmpIfZero(11), // 5   if !cond -> Ret
            compiled::IrOp::GetLocal(0),   // 6   incr: j
            compiled::IrOp::PushInt(1),    // 7         1
            compiled::IrOp::Add,           // 8         j+1
            compiled::IrOp::SetLocal(0),   // 9   j = j+1
            compiled::IrOp::Jmp(2),        // 10  loop
            compiled::IrOp::Ret,           // 11
        ];
        let loop_ops = (n as usize) * 9;
        let compiled_loop = compiled::CompiledHandler { ops, n_locals: 1 };
        let start = bench_now_ms();
        let _ = compiled::run(&compiled_loop, &[]);
        let ms = bench_now_ms() - start;
        out.push_str(&line("IR repeat-counter loop", loop_ops, ms));
        out.push('\n');

        // Entry and scope controls for the compiled path. The production
        // runner is called with the validated owner token and player; the
        // controls below keep the benchmark's wrapper and scope work visible
        // without exposing an unchecked scope pointer.
        {
            let escapes: usize = 8_000_000;
            let chunk = 1024usize;
            let compiled_esc = compiled::CompiledHandler {
                ops: vec![compiled::IrOp::Escape; chunk],
                n_locals: 8,
            };
            let mut session = crate::player::session::RuntimeSession::new(
                crate::player::symbols::symbol_table::SymbolOwner {
                    session: 44,
                    generation: 1,
                },
            );
            assert!(session.add_player(1, async_std::channel::unbounded().0));
            let (scope_ref, scope_token) = session
                .with_player(1, |mut runtime| {
                    let scope_ref = runtime.player.push_scope();
                    let scope_token = ScopeToken {
                        owner: runtime.player.owner.clone(),
                        slot: scope_ref,
                        generation: runtime.player.scopes[scope_ref].generation,
                        epoch: runtime.player.scope_invalidation_epoch,
                    };
                    let dr = runtime.player.alloc_datum(Datum::Int(7));
                    runtime.player.scopes[scope_ref].args.push(dr);
                    (scope_ref, scope_token)
                })
                .unwrap();
            let warmup = 1 << 20;
            for pass in 1..=2 {
                session
                    .with_player(1, |mut runtime| {
                        for i in 0..warmup {
                            runtime.player.scopes[scope_ref].bytecode_index = i % chunk;
                            let _ = compiled::run_handler_resumable(
                                &compiled_esc,
                                &scope_token,
                                runtime.player,
                                runtime.symbols,
                            );
                        }
                    })
                    .unwrap();
                let start = bench_now_ms();
                let mut done = 0usize;
                session
                    .with_player(1, |mut runtime| {
                        while done < escapes {
                            for i in 0..chunk {
                                runtime.player.scopes[scope_ref].bytecode_index = i;
                                let _ = compiled::run_handler_resumable(
                                    &compiled_esc,
                                    &scope_token,
                                    runtime.player,
                                    runtime.symbols,
                                );
                            }
                            done += chunk;
                        }
                    })
                    .unwrap();
                let ms = bench_now_ms() - start;
                out.push_str(&line(
                    &format!("IR escape re-entry: validated wrapper p{pass}"),
                    done,
                    ms,
                ));
                out.push('\n');

                session
                    .with_player(1, |mut runtime| {
                        let scope = &mut runtime.player.scopes[scope_ref];
                        for i in 0..warmup {
                            scope.bytecode_index = i % chunk;
                            let _ = compiled::run_handler_stub(&compiled_esc, scope);
                        }
                    })
                    .unwrap();
                let start = bench_now_ms();
                let mut stub_done = 0usize;
                session
                    .with_player(1, |mut runtime| {
                        let scope = &mut runtime.player.scopes[scope_ref];
                        while stub_done < escapes {
                            for i in 0..chunk {
                                scope.bytecode_index = i;
                                let _ = compiled::run_handler_stub(&compiled_esc, scope);
                            }
                            stub_done += chunk;
                        }
                    })
                    .unwrap();
                let ms = bench_now_ms() - start;
                out.push_str(&line(
                    &format!("IR escape control: &mut Scope stub p{pass}"),
                    stub_done,
                    ms,
                ));
                out.push('\n');

                session
                    .with_player(1, |mut runtime| {
                        let scope = &mut runtime.player.scopes[scope_ref];
                        scope.ensure_locals(compiled_esc.n_locals);
                        std::hint::black_box(scope.locals.len());
                    })
                    .unwrap();
                let start = bench_now_ms();
                let mut local_done = 0usize;
                session
                    .with_player(1, |mut runtime| {
                        let scope = &mut runtime.player.scopes[scope_ref];
                        while local_done < escapes {
                            scope.ensure_locals(compiled_esc.n_locals);
                            std::hint::black_box(scope.locals.len());
                            local_done += 1;
                        }
                    })
                    .unwrap();
                let ms = bench_now_ms() - start;
                out.push_str(&line(
                    &format!("IR scope local sizing p{pass}"),
                    local_done,
                    ms,
                ));
                out.push('\n');
            }

            session
                .with_player(1, |runtime| runtime.player.pop_scope())
                .unwrap();
        }
    }
    out
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod interp_bench {
    //! Native runner for the shared interpreter throughput benchmark. Run with:
    //!   cargo test --lib --manifest-path vm-rust/Cargo.toml interp_bench -- --nocapture
    use crate::player::testing::{TestPlayer, run_test};

    #[test]
    fn bytecode_throughput() {
        // Symbol table must exist before DirPlayer::new (builtin symbols).
        run_test(async {
            let _player = TestPlayer::new();
            let report = crate::player::run_bytecode_benchmark();
            println!("{report}");
            assert!(report.contains("ops/sec"));
        });
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod cursor_reset_tests {
    use super::*;
    use crate::player::cast_lib::{CastLib, CastMemberRef};
    use crate::player::script::ScriptInstance;
    use crate::player::score::SpriteChannel;
    use crate::player::testing::{TestHarness, TestPlayer, run_test};
    use crate::player::timeout::Timeout;

    #[test]
    fn a_new_movie_starts_with_the_arrow() {
        run_test(async {
            let player = TestPlayer::new();
            let runtime = player.harness_runtime();
            runtime
                .with_context(|context| {
                    let p = context.player;
                    p.cursor = CursorRef::System(200);
                    p.cursor_is_hidden = true;
                    p.wants_pointer_lock = true;
                    p.reset_cursor_for_new_movie();
                    assert!(matches!(p.cursor, CursorRef::System(0)));
                    assert!(!p.cursor_is_hidden);
                    assert!(!p.wants_pointer_lock);
                })
                .expect("harness player must remain owned");
        });
    }

    #[test]
    fn dropped_frame_guard_clears_only_its_captured_owner() {
        run_test(async {
            let player = TestPlayer::new();
            let runtime = player.harness_runtime();
            let session = runtime.session();
            let player_id = runtime.player_id();
            let owner = runtime.owner().clone();
            runtime
                .with_context(|context| context.player.is_in_frame_update = true)
                .expect("harness player must remain owned");

            {
                let _guard = FrameUpdateFlagGuard::new(session.clone(), player_id, owner.clone());
            }
            assert_eq!(
                session
                    .borrow_mut()
                    .with_player(player_id, |context| context.player.is_in_frame_update),
                Some(false)
            );

            runtime
                .with_context(|context| context.player.is_in_frame_update = true)
                .expect("harness player must remain owned");
            let guard = FrameUpdateFlagGuard::new(session.clone(), player_id, owner.clone());
            let replacement = session
                .borrow_mut()
                .reset_player_owned(player_id, &owner)
                .expect("captured owner must reset its player");
            session
                .borrow_mut()
                .with_player(player_id, |context| {
                    context.player.is_in_frame_update = true
                })
                .expect("replacement player must remain owned");
            drop(guard);

            assert_eq!(
                session.borrow_mut().with_player(player_id, |context| {
                    (
                        context.player.owner.same_identity(&replacement),
                        context.player.is_in_frame_update,
                    )
                }),
                Some((true, true))
            );
        });
    }

    #[test]
    fn suspended_owned_frame_future_does_not_touch_replacement() {
        run_test(async {
            let player = TestPlayer::new();
            let runtime = player.harness_runtime();
            let session = runtime.session();
            let player_id = runtime.player_id();
            let owner = runtime.owner().clone();
            let prepare_frame = Symbol::builtin(BuiltInSymbol::PrepareFrame);
            session
                .borrow_mut()
                .with_player(player_id, |context| {
                    let do_symbol = Symbol::builtin(BuiltInSymbol::Do);
                    let update_stage_source = context.symbols.intern("updateStage()");
                    let member_ref = CastMemberRef {
                        cast_lib: 1,
                        cast_member: 1,
                    };
                    let handler = Rc::new(crate::director::chunks::handler::HandlerDef {
                        name_id: 0,
                        bytecode_array: vec![
                            crate::director::chunks::handler::Bytecode::new(
                                crate::director::lingo::opcode::OpCode::PushSymb,
                                2,
                                0,
                            ),
                            crate::director::chunks::handler::Bytecode::new(
                                crate::director::lingo::opcode::OpCode::PushArgListNoRet,
                                1,
                                1,
                            ),
                            crate::director::chunks::handler::Bytecode::new(
                                crate::director::lingo::opcode::OpCode::ExtCall,
                                1,
                                2,
                            ),
                            crate::director::chunks::handler::Bytecode::new(
                                crate::director::lingo::opcode::OpCode::Ret,
                                0,
                                3,
                            ),
                        ],
                        bytecode_index_map: FxHashMap::default(),
                        argument_name_ids: vec![],
                        local_name_ids: vec![],
                        global_name_ids: vec![],
                        compiled_ir: RefCell::new(None),
                    });
                    let script = Rc::new(crate::player::script::Script {
                        member_ref: member_ref.clone(),
                        name: "owned-frame-suspend".to_owned(),
                        chunk: crate::director::chunks::script::ScriptChunk {
                            script_number: 1,
                            literals: vec![],
                            handlers: vec![],
                            property_name_ids: vec![],
                            property_defaults: HashMap::new(),
                        },
                        script_type: ScriptType::Movie,
                        handlers: FxHashMap::from_iter([(prepare_frame.clone(), handler)]),
                        handler_names_raw: vec![
                            "prepareFrame".to_owned(),
                            "do".to_owned(),
                            "updateStage()".to_owned(),
                        ],
                        handler_names: vec![prepare_frame, do_symbol, update_stage_source.clone()],
                        properties: RefCell::new(FxHashMap::default()),
                    });
                    let mut cast = CastLib::test_external(1, 0);
                    cast.name_symbols = Rc::from(vec![
                        Symbol::builtin(BuiltInSymbol::PrepareFrame),
                        Symbol::builtin(BuiltInSymbol::Do),
                        update_stage_source,
                    ]);
                    cast.scripts.insert(1, script);
                    context.player.movie.cast_manager.casts.push(cast);
                    context.player.command_handler_yielding = true;
                    context.player.in_frame_script = true;
                })
                .expect("harness player must remain owned");

            let frame_request = || crate::player::handlers::movie::MovieAsyncRequest {
                player_id,
                owner: owner.clone(),
                kind: crate::player::handlers::movie::MovieAsyncKind::FrameUpdate {
                    now_ms: 1_000.25,
                },
                args: Vec::new(),
            };
            let mut frame = Box::pin(crate::player::handlers::movie::execute_movie_async(
                session.clone(),
                frame_request(),
            ));
            let waker = futures::task::noop_waker();
            let mut cx = std::task::Context::from_waker(&waker);
            assert!(matches!(
                frame.as_mut().poll(&mut cx),
                std::task::Poll::Pending
            ));
            assert_eq!(
                session
                    .borrow_mut()
                    .with_player(player_id, |context| context.player.is_in_frame_update),
                Some(true)
            );

            drop(frame);
            assert_eq!(
                session
                    .borrow_mut()
                    .with_player(player_id, |context| context.player.is_in_frame_update),
                Some(false)
            );

            let mut frame = Box::pin(crate::player::handlers::movie::execute_movie_async(
                session.clone(),
                frame_request(),
            ));
            assert!(matches!(
                frame.as_mut().poll(&mut cx),
                std::task::Poll::Pending
            ));
            assert_eq!(
                session
                    .borrow_mut()
                    .with_player(player_id, |context| context.player.is_in_frame_update),
                Some(true)
            );

            let replacement = session
                .borrow_mut()
                .reset_player_owned(player_id, &owner)
                .expect("captured owner must reset its player");
            session
                .borrow_mut()
                .with_player(player_id, |context| {
                    context.player.is_in_frame_update = true
                })
                .expect("replacement player must remain owned");
            drop(frame);

            assert_eq!(
                session.borrow_mut().with_player(player_id, |context| {
                    (
                        context.player.owner.same_identity(&replacement),
                        context.player.is_in_frame_update,
                    )
                }),
                Some((true, true))
            );
        });
    }

    #[test]
    fn suspended_owned_frame_drop_does_not_touch_replacement() {
        run_test(async {
            let player = TestPlayer::new();
            let runtime = player.harness_runtime();
            let session = runtime.session();
            let player_id = runtime.player_id();
            let owner = runtime.owner().clone();
            runtime
                .with_context(|context| context.player.is_in_frame_update = true)
                .expect("harness player must remain owned");

            let mut suspended = Box::pin(async {
                let _guard = FrameUpdateFlagGuard::new(session.clone(), player_id, owner.clone());
                future::pending::<()>().await;
            });
            let waker = futures::task::noop_waker();
            let mut cx = std::task::Context::from_waker(&waker);
            assert!(matches!(
                suspended.as_mut().poll(&mut cx),
                std::task::Poll::Pending
            ));

            let replacement = session
                .borrow_mut()
                .reset_player_owned(player_id, &owner)
                .expect("captured owner must reset its player");
            session
                .borrow_mut()
                .with_player(player_id, |context| {
                    context.player.is_in_frame_update = true
                })
                .expect("replacement player must remain owned");
            drop(suspended);

            assert_eq!(
                session.borrow_mut().with_player(player_id, |context| {
                    (
                        context.player.owner.same_identity(&replacement),
                        context.player.is_in_frame_update,
                    )
                }),
                Some((true, true))
            );
        });
    }

    #[test]
    fn owned_init_orders_timeout_before_exit_and_marks_begin_sprite_channel() {
        run_test(async {
            let player = TestPlayer::new();
            let runtime = player.harness_runtime();
            let session = runtime.session();
            let player_id = runtime.player_id();
            let owner = runtime.owner().clone();
            let (instance_id, markers) = session
                .borrow_mut()
                .with_player(player_id, |context| {
                    context.player.is_playing = true;
                    let exit_frame = Symbol::builtin(BuiltInSymbol::ExitFrame);
                    let prepare_movie = Symbol::builtin(BuiltInSymbol::PrepareMovie);
                    let prepare_frame = Symbol::builtin(BuiltInSymbol::PrepareFrame);
                    let start_movie = Symbol::builtin(BuiltInSymbol::StartMovie);
                    let enter_frame = Symbol::builtin(BuiltInSymbol::EnterFrame);
                    let begin_sprite = Symbol::builtin(BuiltInSymbol::BeginSprite);
                    let timeout_marker = context.symbols.intern("lifecycleTimeoutSeen");
                    let order_marker = context.symbols.intern("lifecycleExitOrder");
                    let prepare_marker = context.symbols.intern("lifecyclePrepareMovie");
                    let begin_marker = context.symbols.intern("lifecycleBeginSprite");
                    let prepare_frame_marker = context.symbols.intern("lifecyclePrepareFrame");
                    let start_marker = context.symbols.intern("lifecycleStartMovie");
                    let enter_marker = context.symbols.intern("lifecycleEnterFrame");
                    let names = vec![
                        exit_frame.clone(),
                        timeout_marker.clone(),
                        order_marker.clone(),
                        prepare_marker.clone(),
                        begin_marker.clone(),
                        prepare_frame_marker.clone(),
                        start_marker.clone(),
                        enter_marker.clone(),
                    ];
                    let set_global = |index: u16| {
                        Rc::new(HandlerDef {
                            name_id: 0,
                            bytecode_array: vec![
                                Bytecode::new(
                                    crate::director::lingo::opcode::OpCode::PushInt8,
                                    1,
                                    0,
                                ),
                                Bytecode::new(
                                    crate::director::lingo::opcode::OpCode::SetGlobal,
                                    index as i64,
                                    1,
                                ),
                                Bytecode::new(crate::director::lingo::opcode::OpCode::Ret, 0, 2),
                            ],
                            bytecode_index_map: FxHashMap::default(),
                            argument_name_ids: vec![],
                            local_name_ids: vec![],
                            global_name_ids: (1..names.len() as u16).collect(),
                            compiled_ir: RefCell::new(None),
                        })
                    };
                    let exit_handler = Rc::new(HandlerDef {
                        name_id: 0,
                        bytecode_array: vec![
                            Bytecode::new(crate::director::lingo::opcode::OpCode::GetGlobal, 1, 0),
                            Bytecode::new(crate::director::lingo::opcode::OpCode::SetGlobal, 2, 1),
                            Bytecode::new(crate::director::lingo::opcode::OpCode::Ret, 0, 2),
                        ],
                        bytecode_index_map: FxHashMap::default(),
                        argument_name_ids: vec![],
                        local_name_ids: vec![],
                        global_name_ids: (1..names.len() as u16).collect(),
                        compiled_ir: RefCell::new(None),
                    });
                    let behavior_ref = CastMemberRef {
                        cast_lib: 1,
                        cast_member: 1,
                    };
                    let static_ref = CastMemberRef {
                        cast_lib: 1,
                        cast_member: 2,
                    };
                    let timeout_ref = CastMemberRef {
                        cast_lib: 1,
                        cast_member: 3,
                    };
                    let script_chunk = || crate::director::chunks::script::ScriptChunk {
                        script_number: 1,
                        literals: vec![],
                        handlers: vec![],
                        property_name_ids: vec![],
                        property_defaults: HashMap::new(),
                    };
                    let behavior_script = Rc::new(Script {
                        member_ref: behavior_ref.clone(),
                        name: "lifecycle-behavior".to_owned(),
                        chunk: script_chunk(),
                        script_type: ScriptType::Score,
                        handlers: FxHashMap::from_iter([(begin_sprite.clone(), set_global(4))]),
                        handler_names_raw: vec!["beginSprite".to_owned()],
                        handler_names: vec![begin_sprite],
                        properties: RefCell::new(FxHashMap::default()),
                    });
                    let timeout_script = Rc::new(Script {
                        member_ref: timeout_ref.clone(),
                        name: "lifecycle-timeout".to_owned(),
                        chunk: script_chunk(),
                        script_type: ScriptType::Score,
                        handlers: FxHashMap::from_iter([(exit_frame.clone(), set_global(1))]),
                        handler_names_raw: vec!["exitFrame".to_owned()],
                        handler_names: vec![exit_frame.clone()],
                        properties: RefCell::new(FxHashMap::default()),
                    });
                    let static_script = Rc::new(Script {
                        member_ref: static_ref,
                        name: "lifecycle-movie".to_owned(),
                        chunk: script_chunk(),
                        script_type: ScriptType::Movie,
                        handlers: FxHashMap::from_iter([
                            (prepare_movie, set_global(3)),
                            (prepare_frame, set_global(5)),
                            (start_movie, set_global(6)),
                            (enter_frame, set_global(7)),
                            (exit_frame.clone(), exit_handler),
                        ]),
                        handler_names_raw: vec![
                            "prepareMovie".to_owned(),
                            "prepareFrame".to_owned(),
                            "startMovie".to_owned(),
                            "enterFrame".to_owned(),
                            "exitFrame".to_owned(),
                        ],
                        handler_names: vec![
                            Symbol::builtin(BuiltInSymbol::PrepareMovie),
                            Symbol::builtin(BuiltInSymbol::PrepareFrame),
                            Symbol::builtin(BuiltInSymbol::StartMovie),
                            Symbol::builtin(BuiltInSymbol::EnterFrame),
                            Symbol::builtin(BuiltInSymbol::ExitFrame),
                        ],
                        properties: RefCell::new(FxHashMap::default()),
                    });
                    let mut cast = CastLib::test_external(1, 0);
                    cast.name_symbols = Rc::from(names);
                    cast.scripts.insert(1, behavior_script);
                    cast.scripts.insert(2, static_script);
                    cast.scripts.insert(3, timeout_script);
                    context.player.movie.cast_manager.casts.push(cast);
                    let instance = context
                        .player
                        .allocator
                        .alloc_script_instance(ScriptInstance {
                            instance_id: 1,
                            script: behavior_ref,
                            ancestor: None,
                            properties: FxHashMap::default(),
                            begin_sprite_called: false,
                        });
                    let instance_id = instance.id();
                    let timeout_instance =
                        context
                            .player
                            .allocator
                            .alloc_script_instance(ScriptInstance {
                                instance_id: 2,
                                script: timeout_ref,
                                ancestor: None,
                                properties: FxHashMap::default(),
                                begin_sprite_called: false,
                            });
                    let timeout_instance_ref = timeout_instance.clone();
                    let mut channel = SpriteChannel::new(1);
                    channel.sprite.entered = true;
                    channel.sprite.script_instance_list = vec![instance];
                    context
                        .player
                        .movie
                        .score
                        .channels
                        .push(SpriteChannel::new(0));
                    context.player.movie.score.channels.push(channel);
                    let timeout_target = context
                        .player
                        .alloc_datum(Datum::ScriptInstanceRef(timeout_instance_ref));
                    context.player.timeout_manager.add_timeout(Timeout {
                        name: "lifecycle-exit".to_owned(),
                        period: 1,
                        handler: exit_frame.clone(),
                        target_ref: timeout_target,
                        is_scheduled: true,
                        next_fire_ms: 0.0,
                    });
                    (
                        instance_id,
                        [
                            timeout_marker,
                            order_marker,
                            prepare_marker,
                            begin_marker,
                            prepare_frame_marker,
                            start_marker,
                            enter_marker,
                        ],
                    )
                })
                .expect("harness player must remain owned");
            run_movie_init_owned(session.clone(), player_id, owner.clone())
                .await
                .expect("owned movie should initialize");

            let observed = session
                .borrow_mut()
                .with_player(player_id, |context| {
                    let value = |symbol: &Symbol| {
                        context.player.globals.get(symbol).and_then(|value| {
                            match context.player.get_datum(value) {
                                Datum::Int(value) => Some(*value),
                                _ => None,
                            }
                        })
                    };
                    (
                        context
                            .player
                            .allocator
                            .get_script_instance_entry(instance_id)
                            .map(|entry| entry.script_instance.begin_sprite_called),
                        markers.map(|symbol| value(&symbol)),
                    )
                })
                .expect("harness player must remain owned");
            assert_eq!(
                observed,
                (
                    Some(true),
                    [
                        Some(1),
                        Some(1),
                        Some(1),
                        Some(1),
                        Some(1),
                        Some(1),
                        Some(1)
                    ],
                )
            );
        });
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod handler_gap_tests {
    use super::*;
    use crate::player::testing::{TestHarness, TestPlayer, run_test};

    #[test]
    fn input_waits_until_the_frame_handler_returns() {
        run_test(async {
            let player = TestPlayer::new();
            let runtime = player.harness_runtime();
            let session = runtime.session();
            let player_id = runtime.player_id();
            let owner = runtime.owner().clone();
            runtime
                .with_context(|context| {
                    let p = context.player;
                    p.is_playing = true;
                    p.in_frame_script = true;
                })
                .expect("harness player must remain owned");
            let held = timeout(
                Duration::from_millis(30),
                wait_for_handler_gap_owned(session.clone(), player_id, owner.clone()),
            )
            .await;
            assert!(held.is_err(), "input ran inside the frame handler");
            runtime
                .with_context(|context| context.player.in_frame_script = false)
                .expect("harness player must remain owned");
            let released = timeout(
                Duration::from_millis(200),
                wait_for_handler_gap_owned(session, player_id, owner),
            )
            .await;
            assert!(
                matches!(released, Ok(Ok(()))),
                "input never ran after the handler returned"
            );
        });
    }

    #[test]
    fn a_mouse_handler_does_not_hold_input() {
        run_test(async {
            let player = TestPlayer::new();
            let runtime = player.harness_runtime();
            let session = runtime.session();
            let player_id = runtime.player_id();
            let owner = runtime.owner().clone();
            runtime
                .with_context(|context| {
                    let p = context.player;
                    p.is_playing = true;
                    p.in_frame_script = true;
                    p.in_mouse_command = true;
                })
                .expect("harness player must remain owned");
            let released = timeout(
                Duration::from_millis(200),
                wait_for_handler_gap_owned(session, player_id, owner),
            )
            .await;
            assert!(matches!(released, Ok(Ok(()))));
        });
    }

    #[test]
    fn a_retired_owner_cancels_before_input_mutation() {
        run_test(async {
            let player = TestPlayer::new();
            let runtime = player.harness_runtime();
            let session = runtime.session();
            let player_id = runtime.player_id();
            let owner = runtime.owner().clone();
            owner.mark_arena_dead();

            let result = wait_for_handler_gap_owned(session, player_id, owner).await;
            assert!(
                result.is_err(),
                "retired owners must not reach input handling"
            );
        });
    }

    #[test]
    fn a_reset_cancels_a_pending_gap_without_touching_the_replacement() {
        run_test(async {
            let player = TestPlayer::new();
            let runtime = player.harness_runtime();
            let session = runtime.session();
            let player_id = runtime.player_id();
            let owner = runtime.owner().clone();
            runtime
                .with_context(|context| {
                    context.player.is_playing = true;
                    context.player.in_frame_script = true;
                })
                .expect("harness player must remain owned");

            let mut gap = Box::pin(wait_for_handler_gap_owned(
                session.clone(),
                player_id,
                owner.clone(),
            ));
            let waker = futures::task::noop_waker();
            let mut context = std::task::Context::from_waker(&waker);
            assert!(matches!(
                gap.as_mut().poll(&mut context),
                std::task::Poll::Pending,
            ));

            let replacement_owner = session
                .borrow_mut()
                .reset_player_owned(player_id, &owner)
                .expect("captured owner must reset its player");
            session
                .borrow_mut()
                .with_player(player_id, |context| {
                    context.player.is_playing = true;
                    context.player.in_frame_script = true;
                })
                .expect("replacement player must remain installed");
            async_std::task::sleep(Duration::from_millis(10)).await;

            assert!(matches!(
                gap.as_mut().poll(&mut context),
                std::task::Poll::Ready(Err(_)),
            ));
            let replacement_state = session
                .borrow_mut()
                .with_player(player_id, |context| {
                    (
                        context.player.owner.same_identity(&replacement_owner),
                        context.player.is_playing,
                        context.player.in_frame_script,
                    )
                })
                .expect("replacement player must remain installed");
            assert_eq!(replacement_state, (true, true, true));
        });
    }
}

#[cfg(test)]
mod handler_code_lifetime_tests {
    use super::*;
    use crate::director::chunks::{
        handler::{Bytecode, HandlerDef},
        script::ScriptChunk,
    };
    use crate::director::lingo::opcode::OpCode;
    use crate::director::enums::ScriptType;
    use crate::player::cast_lib::{CastLib, CastMemberRef};
    use crate::player::symbols::{builtin::BuiltInSymbol, symbol::Symbol};
    use std::{cell::RefCell, collections::HashMap};

    fn handler(opcode: OpCode) -> Rc<HandlerDef> {
        Rc::new(HandlerDef {
            name_id: 0,
            bytecode_array: vec![Bytecode::new(opcode, 0, 0)],
            bytecode_index_map: fxhash::FxHashMap::default(),
            argument_name_ids: vec![0],
            local_name_ids: vec![99],
            global_name_ids: vec![],
            compiled_ir: RefCell::new(None),
        })
    }

    fn script(name: &str, handler: Rc<HandlerDef>) -> Rc<crate::player::script::Script> {
        let handler_name = Symbol::builtin(BuiltInSymbol::New);
        let mut handlers = fxhash::FxHashMap::default();
        handlers.insert(handler_name.clone(), handler);
        Rc::new(crate::player::script::Script {
            member_ref: CastMemberRef {
                cast_lib: 1,
                cast_member: 1,
            },
            name: name.to_owned(),
            chunk: ScriptChunk {
                script_number: 1,
                literals: vec![],
                handlers: vec![],
                property_name_ids: vec![],
                property_defaults: HashMap::new(),
            },
            script_type: ScriptType::Movie,
            handlers,
            handler_names_raw: vec!["new".to_owned()],
            handler_names: vec![handler_name],
            properties: RefCell::new(fxhash::FxHashMap::default()),
        })
    }

    #[test]
    fn owned_handler_code_survives_source_replacement_until_context_drop() {
        let old_handler = handler(OpCode::PushInt8);
        let old_handler_weak = Rc::downgrade(&old_handler);
        let old_script = script("old-script", old_handler);
        let old_script_weak = Rc::downgrade(&old_script);
        let old_names: Rc<[Symbol]> = Rc::from(vec![Symbol::builtin(BuiltInSymbol::New)]);
        let old_names_weak = Rc::downgrade(&old_names);
        let mut source_cast = CastLib::test_external(1, 0);
        source_cast.scripts.insert(1, old_script);
        source_cast.name_symbols = old_names.clone();

        let old_script = source_cast.scripts.get(&1).unwrap().clone();
        let old_handler = old_script
            .get_own_handler(Symbol::builtin(BuiltInSymbol::New))
            .unwrap()
            .clone();

        let ctx = BytecodeHandlerContext {
            scope: ScopeToken {
                owner: OwnerToken::transitional(),
                slot: 0,
                generation: 0,
                epoch: 0,
            },
            code: HandlerCode {
                script: old_script.clone(),
                handler: old_handler.clone(),
                names: old_names.clone(),
            },
            multiplier: 1,
        };

        // Replace each source owner as a cast reload would. The retained code
        // must continue to observe the old immutable snapshot.
        let replacement_handler = handler(OpCode::PushInt16);
        let replacement_script = script("replacement-script", replacement_handler.clone());
        let replacement_names: Rc<[Symbol]> =
            Rc::from(vec![Symbol::builtin(BuiltInSymbol::Script)]);
        drop(source_cast.scripts.insert(1, replacement_script).unwrap());
        source_cast.name_symbols = replacement_names;
        drop(replacement_handler);
        drop(old_script);
        drop(old_handler);
        drop(old_names);

        assert_eq!(ctx.code.script.name, "old-script");
        assert_eq!(ctx.code.handler.bytecode_array[0].opcode, OpCode::PushInt8);
        assert_eq!(
            ctx.code.names[ctx.code.handler.argument_name_ids[0] as usize],
            BuiltInSymbol::New
        );
        assert_eq!(
            ctx.code.handler.local_name_ids.iter().position(|&nid| {
                ctx.code
                    .names
                    .get(nid as usize)
                    .is_some_and(|candidate| candidate == &Symbol::builtin(BuiltInSymbol::New))
            }),
            None
        );
        assert!(old_script_weak.upgrade().is_some());
        assert!(old_handler_weak.upgrade().is_some());
        assert!(old_names_weak.upgrade().is_some());

        drop(ctx);
        assert!(old_script_weak.upgrade().is_none());
        assert!(old_handler_weak.upgrade().is_none());
        assert!(old_names_weak.upgrade().is_none());
        drop(source_cast);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod scope_token_tests {
    use super::*;
    use crate::director::chunks::{
        handler::{Bytecode, HandlerDef},
        script::ScriptChunk,
        score::{ScoreChunk, ScoreChunkHeader},
    };
    use crate::director::enums::{FilmLoopInfo, ScriptType};
    use crate::director::lingo::opcode::OpCode;
    use crate::player::cast_lib::{CastLib, CastMemberRef};
    use crate::player::cast_member::{CastMember, CastMemberType, FilmLoopMember, FlashMember};
    use crate::player::bitmap::bitmap::{Bitmap, PaletteRef};
    use crate::player::geometry::IntRect;
    use crate::player::score::{Score, ScoreSpriteSpan, SpriteChannel};
    use crate::player::script::Script;
    use crate::player::symbols::symbol::Symbol;
    use async_std::channel;
    use std::{cell::RefCell, collections::HashMap};

    fn make_player(owner: OwnerToken) -> DirPlayer {
        let (tx, _rx) = channel::unbounded();
        DirPlayer::new_with_owner(tx, owner)
    }

    #[test]
    fn reset_core_clears_transient_bitmap_records_after_allocator_sweep() {
        let mut player = make_player(OwnerToken::transitional());
        let mut anchored = Bitmap::new(1, 1, 32, 32, 8, PaletteRef::Default);
        anchored.data[..4].copy_from_slice(&[1, 2, 3, 4]);
        let anchored_id = player.bitmap_manager.add_bitmap(anchored);
        let anchored_handle = player.bitmap_manager.local_handle(anchored_id).unwrap();

        let ephemeral_id = player.bitmap_manager.add_ephemeral_bitmap(Bitmap::new(
            1,
            1,
            32,
            32,
            8,
            PaletteRef::Default,
        ));
        let ephemeral_handle = player.bitmap_manager.local_handle(ephemeral_id).unwrap();
        let retained = player.alloc_datum(Datum::BitmapRef(ephemeral_handle.clone()));
        player.stage_image = Some(ephemeral_id);
        player.flash_frame_buffers.insert(1, ephemeral_id);
        player.nested_movie_images.insert(
            CastMemberRef {
                cast_lib: 1,
                cast_member: 1,
            },
            ephemeral_id,
        );
        player.w3d_frame_buffers.insert((1, 1), ephemeral_id);

        player.reset_owned_core();

        assert!(player.stage_image.is_none());
        assert!(player.flash_frame_buffers.is_empty());
        assert!(player.nested_movie_images.is_empty());
        assert!(player.w3d_frame_buffers.is_empty());
        assert!(player.bitmap_manager.get_bitmap(ephemeral_id).is_none());
        assert!(
            player
                .bitmap_manager
                .get_bitmap_handle(&ephemeral_handle)
                .is_none()
        );
        let fresh_anchor = player.bitmap_manager.local_handle(anchored_id).unwrap();
        assert_eq!(
            player
                .bitmap_manager
                .get_bitmap_handle(&fresh_anchor)
                .unwrap()
                .data,
            vec![1, 2, 3, 4]
        );

        drop(retained);
    }

    #[test]
    fn reset_core_clears_flash_state_and_requeues_with_rotated_owner() {
        let mut player = make_player(OwnerToken::new(super::ownership::OwnerKey {
            session: 96,
            player: 1,
            generation: 1,
        }));
        let old_owner = player.owner.clone();
        let swf = include_bytes!("../../tests/fixtures/flash_initial_access.swf").to_vec();
        let member_ref = CastMemberRef {
            cast_lib: 1,
            cast_member: 1,
        };
        player.movie.cast_manager.casts.push({
            let mut cast = CastLib::test_external(1, 0);
            cast.members.insert(
                1,
                CastMember::new(
                    1,
                    CastMemberType::Flash(FlashMember {
                        data: swf.clone(),
                        reg_point: (0, 0),
                        flash_info: None,
                    }),
                ),
            );
            cast
        });
        player.movie.score.channels = vec![SpriteChannel::new(0), SpriteChannel::new(1)];
        player.movie.score.channels[1].sprite.member = Some(member_ref);
        player.movie.score.channels[1].sprite.visible = true;
        player
            .queue_flash_member_load(1, 1, 1, 1, swf, 1, 1, false, -1)
            .expect("old Flash load should queue");
        player.flash_ready_sprites.insert(1);
        assert_eq!(player.flash_sprite_loaded.len(), 1);
        assert_eq!(player.flash_host_actions.len(), 1);

        player.reset_owned_core();

        assert!(!old_owner.is_arena_live());
        assert_eq!(
            player.movie.score.channels[1].sprite.member,
            Some(member_ref)
        );
        assert!(player.flash_sprite_loaded.is_empty());
        assert!(player.flash_host_actions.is_empty());
        assert!(player.flash_ready_sprites.is_empty());
        let new_owner = player.owner.clone();
        assert!(!new_owner.same_identity(&old_owner));
        player
            .pre_dispatch_flash_members()
            .expect("preserved Flash member should queue under the new owner");
        let new_generation = player
            .flash_instance_generation(1)
            .expect("new owner should reserve a Flash generation");
        let actions = player.take_flash_host_actions();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            FlashHostAction::Load {
                fence, generation, ..
            } => {
                assert!(fence.owner.same_identity(&new_owner));
                assert_eq!(*generation, new_generation);
            }
            _ => panic!("preserved Flash member should queue a Load action"),
        }
    }

    #[test]
    fn pre_dispatch_unloads_exact_old_pair_without_touching_new_generation() {
        let mut player = make_player(OwnerToken::new(super::ownership::OwnerKey {
            session: 98,
            player: 1,
            generation: 1,
        }));
        player.movie.score.channels = vec![SpriteChannel::new(0), SpriteChannel::new(1)];
        player.movie.score.channels[1].sprite.member = Some(CastMemberRef {
            cast_lib: 1,
            cast_member: 2,
        });
        let old_generation = player
            .flash_binding_state
            .borrow_mut()
            .reserve_for_pair(1, 1, 1)
            .expect("old generation");
        assert!(
            player
                .flash_binding_state
                .borrow_mut()
                .publish_first(1, 1, 1, old_generation)
        );
        player.flash_binding_state.borrow_mut().transition_member(
            1,
            Some((1, 1)),
            Some((1, 2)),
            FlashMemberClassification::ValidFlash,
        );
        let new_generation = player
            .flash_binding_state
            .borrow_mut()
            .reserve_for_pair(1, 1, 2)
            .expect("new generation");
        player.flash_sprite_loaded.insert((1, 1, 1));
        player
            .pre_dispatch_flash_members()
            .expect("old unload should queue");
        let actions = player.take_flash_host_actions();
        assert!(actions.iter().any(|action| matches!(
            action,
            FlashHostAction::Unload {
                cast_lib: 1,
                cast_member: 1,
                generation,
                ..
            } if *generation == old_generation
        )));
        assert_eq!(player.flash_instance_generation(1), Some(new_generation));
        assert!(
            player
                .flash_binding_state
                .borrow()
                .is_current(1, new_generation)
        );
    }

    #[test]
    fn failed_first_publication_retires_old_generation_before_replacement() {
        let mut state = FlashBindingState::new();
        let old_generation = state.reserve_for_pair(1, 3, 4).expect("old generation");
        assert!(state.retire_failed_publication(1, 3, 4, old_generation));
        assert!(state.is_retired_generation(1, 3, 4, old_generation));
        let replacement = state
            .reserve_for_pair(1, 3, 5)
            .expect("replacement generation");
        assert_ne!(replacement, old_generation);
        assert_eq!(state.generations.get(&1), Some(&replacement));
        assert!(state.is_retired_generation(1, 3, 4, old_generation));
    }

    #[test]
    fn failed_stale_publication_records_exact_old_generation_for_teardown() {
        let mut state = FlashBindingState::new();
        let old_generation = state.reserve_for_pair(1, 7, 8).expect("old generation");
        let replacement = state
            .reserve_for_pair(1, 7, 9)
            .expect("replacement generation");
        assert!(!state.retire_failed_publication(1, 7, 8, old_generation));
        assert!(state.is_current(1, replacement));
        assert!(state.is_retired_generation(1, 7, 8, old_generation));
        state.clear_retired_generation(1, 7, 8, old_generation);
        assert!(!state.is_retired_generation(1, 7, 8, old_generation));
    }

    #[test]
    fn cast_reload_retires_unresolved_exact_binding_without_loaded_cache() {
        let mut player = make_player(OwnerToken::new(super::ownership::OwnerKey {
            session: 99,
            player: 1,
            generation: 1,
        }));
        player.movie.score.channels = vec![SpriteChannel::new(0), SpriteChannel::new(1)];
        player.movie.score.channels[1].sprite.member = Some(CastMemberRef {
            cast_lib: 4,
            cast_member: 8,
        });
        let generation = player
            .flash_binding_state
            .borrow_mut()
            .reserve_for_pair(1, 4, 8)
            .expect("unresolved exact generation");
        player.invalidate_flash_for_cast_lib(4);
        assert_eq!(player.flash_instance_generation(1), None);
        assert_eq!(player.flash_binding_origin(1), FlashBindingOrigin::Absent);
        assert!(
            !player
                .flash_binding_state
                .borrow()
                .is_retired_generation(1, 4, 8, generation)
        );
        assert!(player.take_flash_host_actions().is_empty());
    }

    #[test]
    fn cast_reload_drains_all_published_same_pair_generations() {
        let mut player = make_player(OwnerToken::new(super::ownership::OwnerKey {
            session: 100,
            player: 1,
            generation: 1,
        }));
        player.movie.score.channels = vec![SpriteChannel::new(0), SpriteChannel::new(1)];
        player.movie.score.channels[1].sprite.member = Some(CastMemberRef {
            cast_lib: 5,
            cast_member: 9,
        });
        let first = player
            .flash_binding_state
            .borrow_mut()
            .reserve_for_pair(1, 5, 9)
            .expect("first generation");
        assert!(
            player
                .flash_binding_state
                .borrow_mut()
                .publish_first(1, 5, 9, first)
        );
        assert!(
            player
                .flash_binding_state
                .borrow_mut()
                .retire_current_for_pair(1, 5, 9, first)
        );
        let second = player
            .flash_binding_state
            .borrow_mut()
            .reserve_for_pair(1, 5, 9)
            .expect("second generation");
        assert!(
            player
                .flash_binding_state
                .borrow_mut()
                .publish_first(1, 5, 9, second)
        );
        player.flash_sprite_loaded.insert((1, 5, 9));
        player.invalidate_flash_for_cast_lib(5);
        let actions = player.take_flash_host_actions();
        let mut generations: Vec<_> = actions
            .iter()
            .filter_map(|action| match action {
                FlashHostAction::Unload { generation, .. } => Some(*generation),
                _ => None,
            })
            .collect();
        generations.sort_unstable();
        assert_eq!(generations, vec![first, second]);
        assert_eq!(player.flash_instance_generation(1), None);
    }

    #[test]
    fn pre_dispatch_retires_exact_binding_after_member_replacement_before_load() {
        let mut player = make_player(OwnerToken::new(super::ownership::OwnerKey {
            session: 101,
            player: 1,
            generation: 1,
        }));
        player.movie.score.channels = vec![SpriteChannel::new(0), SpriteChannel::new(1)];
        player.movie.score.channels[1].sprite.member = Some(CastMemberRef {
            cast_lib: 6,
            cast_member: 2,
        });
        let generation = player
            .flash_binding_state
            .borrow_mut()
            .reserve_for_pair(1, 6, 1)
            .expect("exact reservation");
        player
            .pre_dispatch_flash_members()
            .expect("reconciliation should complete");
        assert_eq!(player.flash_instance_generation(1), None);
        assert!(
            !player
                .flash_binding_state
                .borrow()
                .is_retired_generation(1, 6, 1, generation)
        );
        assert!(player.take_flash_host_actions().is_empty());
    }

    #[test]
    fn reset_owned_core_owner_generation_exhaustion_preserves_player_state() {
        use std::panic::{catch_unwind, AssertUnwindSafe};

        let owner = OwnerToken::new(super::ownership::OwnerKey {
            session: 48,
            player: 1,
            generation: u64::MAX,
        });
        let mut player = make_player(owner.clone());
        let retained = player.alloc_datum(Datum::String("retained".to_owned()));
        let result = catch_unwind(AssertUnwindSafe(|| player.reset_owned_core()));
        assert!(result.is_err());
        assert!(owner.is_arena_live());
        assert!(player.owner.same_identity(&owner));
        assert!(matches!(player.get_datum(&retained), Datum::String(value) if value == "retained"));
    }

    #[test]
    fn owned_filmloop_advancement_invalidates_cache_and_holds_one_shot_end_frame() {
        async_std::task::block_on(async {
            let session = RuntimeSession::new(SymbolOwner {
                session: 77,
                generation: 1,
            })
            .into_handle();
            assert!(session.borrow_mut().add_player(1, channel::unbounded().0));
            let owner = session
                .borrow_mut()
                .with_player(1, |context| context.player.owner.clone())
                .expect("filmloop test player must exist");
            let member_ref = CastMemberRef {
                cast_lib: 1,
                cast_member: 1,
            };

            session
                .borrow_mut()
                .with_player(1, |context| {
                    let mut filmloop_score = Score::empty();
                    filmloop_score.frame_count = Some(2);
                    let filmloop = CastMember::new(
                        1,
                        CastMemberType::FilmLoop(FilmLoopMember {
                            info: FilmLoopInfo {
                                reg_point: (0, 0),
                                width: 1,
                                height: 1,
                                center: 0,
                                crop: 0,
                                sound: 0,
                                loops: 0,
                            },
                            score_chunk: ScoreChunk {
                                header: ScoreChunkHeader {
                                    total_length: 0,
                                    unk1: 0,
                                    unk2: 0,
                                    entry_count: 0,
                                    unk3: 0,
                                    entry_size_sum: 0,
                                },
                                entries: Vec::new(),
                                frame_intervals: Vec::new(),
                                frame_data: Default::default(),
                                sprite_details: std::collections::HashMap::new(),
                            },
                            score: filmloop_score,
                            current_frame: 1,
                            initial_rect: IntRect {
                                left: 0,
                                top: 0,
                                right: 1,
                                bottom: 1,
                            },
                            cached_total_frames: Some(2),
                        }),
                    );
                    let mut cast = CastLib::test_external(1, 0);
                    cast.members.insert(1, filmloop);
                    context.player.movie.cast_manager.casts.push(cast);

                    let mut channel = SpriteChannel::new(1);
                    channel.sprite.member = Some(member_ref);
                    channel.sprite.puppet = true;
                    channel.sprite.visible = true;
                    context.player.movie.score.channels = vec![SpriteChannel::new(0), channel];
                    context.player.movie.current_frame = 1;
                    // Seed both caches so the owned mutation must invalidate
                    // them rather than merely producing the same lookup.
                    assert_eq!(
                        context.player.active_stage_filmloop_member_refs(),
                        vec![member_ref]
                    );
                })
                .expect("filmloop setup must remain owner-bound");

            let (initial_behavior_generation, initial_filmloop_generation) = session
                .borrow_mut()
                .with_player(1, |context| {
                    (
                        context.player.behavior_channel_cache_generation,
                        context.player.active_stage_filmloop_cache_generation,
                    )
                })
                .expect("filmloop cache state must be readable");

            advance_filmloops_owned(session.clone(), 1, owner.clone())
                .await
                .expect("owned filmloop advancement should complete");
            let first = session
                .borrow_mut()
                .with_player(1, |context| {
                    let current = context
                        .player
                        .movie
                        .cast_manager
                        .find_member_by_ref(&member_ref)
                        .and_then(|member| match &member.member_type {
                            CastMemberType::FilmLoop(filmloop) => Some(filmloop.current_frame),
                            _ => None,
                        });
                    (
                        current,
                        context.player.behavior_channel_cache_generation,
                        context.player.active_stage_filmloop_cache_generation,
                        context.player.stage_dirty,
                    )
                })
                .expect("first filmloop advancement must preserve owner");
            assert_eq!(first.0, Some(2));
            assert!(first.1 > initial_behavior_generation);
            assert!(first.2 > initial_filmloop_generation);
            assert!(first.3, "a filmloop redraw must mark the stage dirty");

            // A non-looping filmloop holds its final frame and must not keep
            // invalidating redraw/cache state on every subsequent frame.
            session
                .borrow_mut()
                .with_player(1, |context| context.player.stage_dirty = false)
                .expect("filmloop player must remain owned");
            advance_filmloops_owned(session.clone(), 1, owner)
                .await
                .expect("one-shot filmloop hold should complete");
            let held = session
                .borrow_mut()
                .with_player(1, |context| {
                    (
                        context
                            .player
                            .movie
                            .cast_manager
                            .find_member_by_ref(&member_ref)
                            .and_then(|member| match &member.member_type {
                                CastMemberType::FilmLoop(filmloop) => Some(filmloop.current_frame),
                                _ => None,
                            }),
                        context.player.behavior_channel_cache_generation,
                        context.player.active_stage_filmloop_cache_generation,
                        context.player.stage_dirty,
                    )
                })
                .expect("one-shot filmloop hold must preserve owner");
            assert_eq!(held, (Some(2), first.1, first.2, false));
        });
    }

    #[test]
    fn owned_stop_ends_persistent_stage_and_filmloop_spans_once() {
        async_std::task::block_on(async {
            let session = RuntimeSession::new(SymbolOwner {
                session: 78,
                generation: 1,
            })
            .into_handle();
            assert!(session.borrow_mut().add_player(1, channel::unbounded().0));
            let owner = session
                .borrow_mut()
                .with_player(1, |context| context.player.owner.clone())
                .expect("stop test player must exist");
            let member_ref = CastMemberRef {
                cast_lib: 1,
                cast_member: 1,
            };

            session
                .borrow_mut()
                .with_player(1, |context| {
                    let mut filmloop_score = Score::empty();
                    filmloop_score.sprite_spans.push(ScoreSpriteSpan {
                        channel_number: 1,
                        start_frame: 1,
                        end_frame: 20,
                        scripts: Vec::new(),
                    });
                    filmloop_score.channels = vec![SpriteChannel::new(0), SpriteChannel::new(1)];
                    filmloop_score.channels[1].sprite.entered = true;
                    let filmloop = CastMember::new(
                        1,
                        CastMemberType::FilmLoop(FilmLoopMember {
                            info: FilmLoopInfo {
                                reg_point: (0, 0),
                                width: 1,
                                height: 1,
                                center: 0,
                                crop: 0,
                                sound: 0,
                                loops: 0,
                            },
                            score_chunk: ScoreChunk {
                                header: ScoreChunkHeader {
                                    total_length: 0,
                                    unk1: 0,
                                    unk2: 0,
                                    entry_count: 0,
                                    unk3: 0,
                                    entry_size_sum: 0,
                                },
                                entries: Vec::new(),
                                frame_intervals: Vec::new(),
                                frame_data: Default::default(),
                                sprite_details: std::collections::HashMap::new(),
                            },
                            score: filmloop_score,
                            current_frame: 1,
                            initial_rect: IntRect {
                                left: 0,
                                top: 0,
                                right: 1,
                                bottom: 1,
                            },
                            cached_total_frames: Some(20),
                        }),
                    );
                    let mut cast = CastLib::test_external(1, 0);
                    cast.members.insert(1, filmloop);
                    context.player.movie.cast_manager.casts.push(cast);

                    let mut stage_channel = SpriteChannel::new(1);
                    stage_channel.sprite.member = Some(member_ref);
                    stage_channel.sprite.entered = true;
                    context.player.movie.score.channels =
                        vec![SpriteChannel::new(0), stage_channel, SpriteChannel::new(2)];
                    context.player.movie.score.channels[2].sprite.entered = true;
                    context.player.movie.score.channels[2].sprite.exited = true;
                    context
                        .player
                        .movie
                        .score
                        .sprite_spans
                        .push(ScoreSpriteSpan {
                            channel_number: 1,
                            start_frame: 1,
                            end_frame: 20,
                            scripts: Vec::new(),
                        });
                    context.player.movie.current_frame = 5;
                    assert_eq!(
                        context.player.active_stage_filmloop_member_refs(),
                        vec![member_ref]
                    );
                })
                .expect("stop fixture setup must remain owner-bound");

            let stage_ended = end_score_sprites_owned(
                session.clone(),
                1,
                owner.clone(),
                ScoreRef::Stage,
                5,
                5,
                true,
            )
            .await
            .expect("stage stop span query must remain owner-bound");
            assert_eq!(stage_ended, vec![1]);
            let film_ended = end_score_sprites_owned(
                session.clone(),
                1,
                owner.clone(),
                ScoreRef::FilmLoop(member_ref.clone()),
                1,
                1,
                true,
            )
            .await
            .expect("filmloop stop span query must remain owner-bound");
            assert_eq!(film_ended, vec![1]);

            stop_movie_sequence_owned(session.clone(), 1, owner.clone())
                .await
                .expect("owned stop sequence must complete");
            let states = session
                .borrow_mut()
                .with_player(1, |context| {
                    let stage = context.player.movie.score.channels[1].sprite.exited;
                    let already_ended = context.player.movie.score.channels[2].sprite.exited;
                    let film = context
                        .player
                        .movie
                        .cast_manager
                        .find_member_by_ref(&member_ref)
                        .and_then(|member| match &member.member_type {
                            CastMemberType::FilmLoop(loop_member) => {
                                Some(loop_member.score.channels[1].sprite.exited)
                            }
                            _ => None,
                        });
                    (stage, already_ended, film)
                })
                .expect("stop result must remain owner-bound");
            assert_eq!(states, (true, true, Some(true)));
        });
    }

    #[test]
    fn cancelled_movie_transition_restores_only_the_captured_owner_flags() {
        async_std::task::block_on(async {
            let session = RuntimeSession::new(SymbolOwner {
                session: 79,
                generation: 1,
            })
            .into_handle();
            assert!(session.borrow_mut().add_player(1, channel::unbounded().0));
            let owner = session
                .borrow_mut()
                .with_player(1, |context| context.player.owner.clone())
                .expect("transition test player must exist");
            let (cancel_tx, cancel_rx) = channel::bounded(1);
            let (started_tx, started_rx) = channel::bounded(1);
            let cancel_task = async_std::task::spawn(async move {
                started_rx
                    .recv()
                    .await
                    .expect("transition operation must start");
                cancel_tx
                    .send(())
                    .await
                    .expect("transition cancellation must be delivered");
            });
            let initial_flags = (false, true);
            let session_for_operation = session.clone();
            let owner_for_operation = owner.clone();
            let result = await_playback_or_cancel(&cancel_rx, async move {
                session_for_operation
                    .borrow_mut()
                    .with_player(1, |context| {
                        context.player.is_in_transition = true;
                        context.player.is_dispatching_events = false;
                    });
                started_tx
                    .send(())
                    .await
                    .expect("transition start must be observed");
                future::pending::<()>().await;
                let _ = owner_for_operation;
            })
            .await;
            assert!(
                result.is_none(),
                "cancellation must drop the pending transition"
            );
            cancel_task.await;
            restore_playback_transition_flags_owned(
                &session,
                1,
                &owner,
                initial_flags.0,
                initial_flags.1,
            )
            .await;
            assert_eq!(
                session.borrow_mut().with_player(1, |context| {
                    (
                        context.player.is_in_transition,
                        context.player.is_dispatching_events,
                    )
                }),
                Some(initial_flags),
            );

            let replacement = session
                .borrow_mut()
                .reset_player_owned(1, &owner)
                .expect("old owner reset must create a replacement");
            session
                .borrow_mut()
                .with_player(1, |context| {
                    context.player.is_in_transition = true;
                    context.player.is_dispatching_events = false;
                })
                .expect("replacement flags must be writable");
            restore_playback_transition_flags_owned(
                &session,
                1,
                &owner,
                initial_flags.0,
                initial_flags.1,
            )
            .await;
            assert_eq!(
                session.borrow_mut().with_player(1, |context| {
                    (
                        context.player.owner.same_identity(&replacement),
                        context.player.is_in_transition,
                        context.player.is_dispatching_events,
                    )
                }),
                Some((true, true, false)),
                "stale cancellation must not rewrite replacement flags",
            );
        });
    }

    #[test]
    fn cancellation_after_stop_boundary_waits_for_transition_without_replaying_old_stop() {
        async_std::task::block_on(async {
            let stop_phase = Rc::new(Cell::new(PlaybackTransitionStopPhase::NotStarted));
            let (cancel_tx, cancel_rx) = channel::bounded(1);
            let (started_tx, started_rx) = channel::bounded(1);
            let (release_tx, release_rx) = channel::bounded(1);
            let operation_phase = stop_phase.clone();
            let operation = async move {
                operation_phase.set(PlaybackTransitionStopPhase::InProgress);
                started_tx
                    .send(())
                    .await
                    .expect("transition stop boundary must be observed");
                operation_phase.set(PlaybackTransitionStopPhase::Complete);
                release_rx
                    .recv()
                    .await
                    .expect("transition mount boundary must be released");
                Ok::<(), ()>(())
            };
            let cancellation = async_std::task::spawn(async move {
                started_rx
                    .recv()
                    .await
                    .expect("transition operation must start");
                cancel_tx
                    .send(())
                    .await
                    .expect("transition cancellation must be delivered");
                // Let the transition selector observe cancellation before the
                // post-StopMovie mount boundary is released.  Releasing both
                // channels in one poll would make the test depend on select's
                // tie-breaking when the operation is also ready.
                async_std::task::yield_now().await;
                release_tx
                    .send(())
                    .await
                    .expect("transition operation must reach its mount boundary");
            });

            let outcome =
                await_playback_transition_or_cancel(&cancel_rx, operation, stop_phase.clone())
                    .await;
            cancellation.await;
            assert!(outcome.cancelled);
            assert!(matches!(outcome.result, Some(Ok(()))));
            assert_eq!(
                stop_phase.get(),
                PlaybackTransitionStopPhase::Complete,
                "cancellation must not drop the operation between old StopMovie and new mount",
            );
        });
    }

    #[test]
    fn reset_during_pending_stop_cancels_old_owner_without_touching_replacement() {
        async_std::task::block_on(async {
            let session = RuntimeSession::new(SymbolOwner {
                session: 86,
                generation: 1,
            })
            .into_handle();
            let (command_tx, _command_rx) = channel::unbounded();
            assert!(session.borrow_mut().add_player(1, command_tx));
            let owner = session
                .borrow_mut()
                .with_player(1, |context| context.player.owner.clone())
                .expect("stop owner must exist");
            let (_epoch, cancel_rx) = session
                .borrow_mut()
                .begin_playback_loop(1, &owner)
                .expect("stop loop must be claimable")
                .expect("stop loop must be installed");
            let stop_phase = Rc::new(Cell::new(PlaybackTransitionStopPhase::InProgress));
            let (started_tx, started_rx) = channel::bounded(1);
            let (release_tx, release_rx) = channel::bounded(1);
            let operation_session = session.clone();
            let operation_owner = owner.clone();
            let operation = async move {
                started_tx.send(()).await.expect("pending stop must start");
                release_rx
                    .recv()
                    .await
                    .expect("pending stop must be released");
                if !operation_session
                    .borrow()
                    .player_owner_matches(1, &operation_owner)
                {
                    return Err(cancelled_scope_error());
                }
                Ok::<(), ScriptError>(())
            };
            let reset_session = session.clone();
            let reset_owner = owner.clone();
            let reset_task = async move {
                started_rx
                    .recv()
                    .await
                    .expect("pending stop must be observable");
                let replacement = reset_session
                    .borrow_mut()
                    .reset_player_owned(1, &reset_owner)
                    .expect("reset must retire the pending stop owner");
                reset_session
                    .borrow_mut()
                    .with_player(1, |context| {
                        context.player.is_in_transition = true;
                        context.player.is_dispatching_events = false;
                    })
                    .expect("replacement flags must remain writable");
                release_tx
                    .send(())
                    .await
                    .expect("pending stop must finish its cancellation path");
                replacement
            };
            let (outcome, replacement) = futures::join!(
                await_playback_transition_or_cancel(&cancel_rx, operation, stop_phase),
                reset_task,
            );
            assert!(outcome.cancelled);
            assert!(matches!(
                outcome.result,
                Some(Err(ScriptError {
                    code: ScriptErrorCode::Abort,
                    ..
                }))
            ));
            assert_eq!(
                session.borrow_mut().with_player(1, |context| {
                    (
                        context.player.owner.same_identity(&replacement),
                        context.player.is_in_transition,
                        context.player.is_dispatching_events,
                    )
                }),
                Some((true, true, false)),
            );
        });
    }

    #[test]
    fn owned_stop_cleanup_error_is_propagated_and_replay_is_consumed() {
        async_std::task::block_on(async {
            let session = RuntimeSession::new(SymbolOwner {
                session: 87,
                generation: 1,
            })
            .into_handle();
            let (command_tx, _command_rx) = channel::unbounded();
            assert!(session.borrow_mut().add_player(1, command_tx));
            let owner = session
                .borrow_mut()
                .with_player(1, |context| context.player.owner.clone())
                .expect("cleanup owner must exist");
            let (epoch, _cancel_rx) = session
                .borrow_mut()
                .begin_playback_loop(1, &owner)
                .expect("cleanup loop must be claimable")
                .expect("cleanup loop must be installed");
            assert_eq!(
                session.borrow_mut().cancel_playback_loop(1, &owner, true),
                Some(epoch)
            );
            assert!(
                session
                    .borrow_mut()
                    .begin_playback_loop(1, &owner)
                    .expect("same-owner replay must be accepted")
                    .is_none()
            );

            // Keep the cancellation record and replay request, but make the
            // captured capability stale before cleanup starts.  This reaches
            // the real owner-bound StopMovie path and proves its error is not
            // converted into a successful replay.
            owner.begin_reset();
            owner.mark_arena_dead();
            let result = cleanup_after_playback_cancel(&session, 1, &owner, epoch).await;
            assert!(matches!(
                result,
                Err(ScriptError {
                    code: ScriptErrorCode::Abort,
                    ..
                })
            ));
            assert!(
                session
                    .borrow_mut()
                    .discard_playback_replay_request(1, &owner, epoch)
            );
            assert!(
                !session
                    .borrow_mut()
                    .take_playback_replay_request(1, &owner, epoch)
            );
        });
    }

    fn token_for(player: &DirPlayer, slot: ScopeRef) -> ScopeToken {
        ScopeToken {
            owner: player.owner.clone(),
            slot,
            generation: player.scopes[slot].generation,
            epoch: player.scope_invalidation_epoch,
        }
    }

    #[test]
    fn owner_generation_and_epoch_are_required_for_scope_access() {
        let owner_a = OwnerToken::transitional();
        let owner_b = OwnerToken::transitional();
        assert_eq!(owner_a.key(), owner_b.key());
        let mut first = make_player(owner_a);
        let mut second = make_player(owner_b);
        let slot = first.push_scope();
        let token = token_for(&first, slot);
        assert!(token.validate_active(&first));
        assert!(token.validate_top(&first));
        let wrong_owner = ScopeToken {
            owner: second.owner.clone(),
            slot,
            generation: token.generation,
            epoch: token.epoch,
        };
        assert!(!wrong_owner.validate_active(&first));
        first.bump_scope_invalidation_epoch();
        assert!(!token.validate_active(&first));
        let other_slot = second.push_scope();
        assert!(token_for(&second, other_slot).validate_top(&second));

        let mut generation_player = make_player(OwnerToken::transitional());
        let generation_slot = generation_player.push_scope();
        let old = token_for(&generation_player, generation_slot);
        generation_player.pop_scope();
        generation_player.push_scope();
        assert!(!old.validate_active(&generation_player));
    }

    #[test]
    fn non_top_teardown_does_not_mutate_replacement_state() {
        let mut player = make_player(OwnerToken::transitional());
        {
            let parent_slot = player.push_scope();
            let parent = token_for(&player, parent_slot);
            player.handler_stack_depth = 1;
            player.in_frame_script = true;
            let child_slot = player.push_scope();
            let child = token_for(&player, child_slot);
            player.handler_stack_depth = 2;
            let before_depth = player.handler_stack_depth;
            let before_count = player.scope_count;
            let before_last = player.last_handler_result.clone();
            let before_frame = player.in_frame_script;
            let before_parent_passed = player.scopes[parent_slot].passed;
            let before_child_stack = player.scopes[child_slot].stack.len();
            let child_value = player.alloc_datum(Datum::Int(42));
            player.scopes[child_slot].return_value = child_value.clone();

            assert!(teardown_handler_frame(&mut player, &parent, true).is_err());
            assert_eq!(player.handler_stack_depth, before_depth);
            assert_eq!(player.scope_count, before_count);
            assert_eq!(player.last_handler_result, before_last);
            assert_eq!(player.in_frame_script, before_frame);
            assert_eq!(player.scopes[parent_slot].passed, before_parent_passed);
            assert_eq!(player.scopes[child_slot].stack.len(), before_child_stack);
            assert!(child.validate_top(&player));

            assert!(teardown_handler_frame(&mut player, &child, false).is_ok());
            assert_eq!(player.handler_stack_depth, 1);
            assert!(parent.validate_top(&player));
            let result = ScopeResult {
                passed: true,
                return_value: child_value.clone(),
            };
            assert!(deliver_scope_return(&mut player, &parent, &result, true));
            assert!(player.scopes[parent_slot].passed);
            assert_eq!(player.scopes[parent_slot].stack.len(), 1);
            let pushed = {
                let (scopes, allocator, bitmap_manager) = (
                    &mut player.scopes,
                    &mut player.allocator,
                    &mut player.bitmap_manager,
                );
                scopes[parent_slot]
                    .stack
                    .get_ref_with(0, allocator, bitmap_manager)
                    .unwrap()
            };
            assert!(matches!(player.get_datum(&pushed), Datum::Int(42)));
        }
    }

    #[test]
    fn setup_expectation_anchors_parent_and_rejects_stack_epoch_changes() {
        let mut player = make_player(OwnerToken::transitional());
        {
            let parent_slot = player.push_scope();
            let parent = token_for(&player, parent_slot);
            let expectation = SetupExpectation::capture(&player, Some(&parent));
            assert!(expectation.validate(&player));

            let child_slot = player.push_scope();
            assert!(!expectation.validate(&player));
            player.pop_scope();
            assert!(expectation.validate(&player));

            player.bump_scope_invalidation_epoch();
            assert!(!expectation.validate(&player));
            assert_eq!(child_slot, parent_slot + 1);
        }
        let mut root_player = make_player(OwnerToken::transitional());
        let root_slot = root_player.push_scope();
        let root_expectation = SetupExpectation::capture(&root_player, None);
        root_player.pop_scope();
        root_player.push_scope();
        assert_eq!(root_slot, 0);
        assert!(!root_expectation.validate(&root_player));
        let empty_root = make_player(OwnerToken::transitional());
        let empty_expectation = SetupExpectation::capture(&empty_root, None);
        assert!(empty_expectation.validate(&empty_root));
        let mut invalidated_root = empty_root;
        invalidated_root.bump_scope_invalidation_epoch();
        assert!(!empty_expectation.validate(&invalidated_root));
    }

    #[test]
    fn post_await_unwind_and_return_guards_leave_stale_scopes_untouched() {
        let mut player = make_player(OwnerToken::transitional());
        {
            let slot = player.push_scope();
            let token = token_for(&player, slot);
            player.handler_stack_depth = 1;
            let mut parents = Vec::new();
            player.bump_scope_invalidation_epoch();
            assert!(!unwind_handler_frames(
                &mut player,
                &token,
                false,
                &mut parents
            ));
            assert_eq!(player.scope_count, 1);
            assert_eq!(player.handler_stack_depth, 1);

            let result = ScopeResult {
                passed: true,
                return_value: DatumRef::Void,
            };
            assert!(!deliver_scope_return(&mut player, &token, &result, true));
            assert!(player.scopes[slot].stack.is_empty());
        }
    }

    fn plan_script() -> Rc<Script> {
        let handler_name = Symbol::builtin(BuiltInSymbol::New);
        let mut handlers = fxhash::FxHashMap::default();
        handlers.insert(
            handler_name.clone(),
            Rc::new(HandlerDef {
                name_id: 0,
                bytecode_array: vec![Bytecode::new(OpCode::Ret, 0, 0)],
                bytecode_index_map: fxhash::FxHashMap::default(),
                argument_name_ids: vec![],
                local_name_ids: vec![],
                global_name_ids: vec![],
                compiled_ir: RefCell::new(None),
            }),
        );
        Rc::new(Script {
            member_ref: CastMemberRef {
                cast_lib: 1,
                cast_member: 1,
            },
            name: "eager-old".to_owned(),
            chunk: ScriptChunk {
                script_number: 1,
                literals: vec![],
                handlers: vec![],
                property_name_ids: vec![],
                property_defaults: HashMap::new(),
            },
            script_type: ScriptType::Movie,
            handlers,
            handler_names_raw: vec!["new".to_owned()],
            handler_names: vec![handler_name],
            properties: RefCell::new(fxhash::FxHashMap::default()),
        })
    }

    #[test]
    fn eager_cast_replacement_keeps_setup_anchor_and_owned_plan_valid() {
        let mut player = make_player(OwnerToken::transitional());
        let slot = player.push_scope();
        let token = token_for(&player, slot);
        let mut old_cast = CastLib::test_external(1, 0);
        old_cast.scripts.insert(1, plan_script());
        player.movie.cast_manager.casts.push(old_cast);
        let expectation = SetupExpectation::capture(&player, None);
        let member = CastMemberRef {
            cast_lib: 1,
            cast_member: 1,
        };
        let plan =
            capture_handler_plan(&player, &member, &Symbol::builtin(BuiltInSymbol::New)).unwrap();
        player.movie_mount_generation += 1;
        player.movie.cast_manager.casts[0] = CastLib::test_external(1, 0);
        assert!(token.validate_top(&player));
        assert!(expectation.validate(&player));
        assert_eq!(plan.code.script.name, "eager-old");
        assert_eq!(plan.code.handler.bytecode_array[0].opcode, OpCode::Ret);
    }

    #[test]
    #[should_panic(expected = "scope invalidation epoch exhausted")]
    fn scope_epoch_exhaustion_is_explicit() {
        let mut player = make_player(OwnerToken::transitional());
        player.scope_invalidation_epoch = u64::MAX;
        player.bump_scope_invalidation_epoch();
    }

    #[test]
    #[should_panic(expected = "scope generation exhausted")]
    fn scope_generation_exhaustion_is_explicit() {
        let mut player = make_player(OwnerToken::transitional());
        player.scopes[0].generation = u64::MAX;
        player.push_scope();
    }
}
