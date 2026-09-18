//! Session-owned linked `#movie` children.
//!
//! The registry deliberately contains only capabilities and channels.  Player
//! teardown remains a session concern: `take_children_for_parent` moves the
//! records out so the teardown owner can close channels and retire children
//! exactly once.

use std::collections::{HashMap, HashSet};

use async_std::channel::Sender;
use std::rc::Rc;

use super::{
    cast_lib::CastMemberRef,
    cast_member::CastMemberType,
    commands::PlayerVMCommand,
    events::PlayerVMEvent,
    font::BitmapFont,
    owner_key_string,
    ownership::{OwnerKey, OwnerToken},
    session::{ExecutionContext, PlayerId, RuntimeSessionHandle},
    PlayerVMExecutionItem, ScriptError,
};

use crate::director::file::{read_director_file_bytes, DirectorFile};

/// Immutable data captured from the parent before any parse, registration, or
/// host await.  A failed parse therefore cannot leave a child in the registry.
#[derive(Clone)]
pub(crate) struct NestedMovieStart {
    pub(crate) parent_id: PlayerId,
    pub(crate) parent_owner: OwnerToken,
    pub(crate) member_ref: CastMemberRef,
    pub(crate) bytes: Rc<Vec<u8>>,
    pub(crate) base_url: String,
    pub(crate) file_name: String,
    pub(crate) system_font: Option<Rc<BitmapFont>>,
}

impl NestedMovieStart {
    pub(crate) fn from_context(
        context: &ExecutionContext<'_>,
        member_ref: CastMemberRef,
    ) -> Result<Self, ScriptError> {
        let member = context
            .player
            .movie
            .cast_manager
            .find_member_by_ref(&member_ref)
            .ok_or_else(|| ScriptError::new("nested movie member is missing".into()))?;
        let movie = match &member.member_type {
            CastMemberType::Movie(movie) => movie,
            _ => return Err(ScriptError::new("nested member is not a movie".into())),
        };
        let bytes = movie
            .bytes
            .clone()
            .ok_or_else(|| ScriptError::new("nested movie bytes are not loaded".into()))?;
        let file_name = movie
            .file_name
            .rsplit(&['/', '\\'][..])
            .find(|name| !name.is_empty())
            .unwrap_or("nested.dcr")
            .to_owned();
        Ok(Self {
            parent_id: context.player_id,
            parent_owner: context.player.owner.clone(),
            member_ref,
            bytes,
            base_url: movie.base_url.clone(),
            file_name,
            system_font: context.player.font_manager.system_font.clone(),
        })
    }

    pub(crate) fn parse(&self) -> Result<DirectorFile, ScriptError> {
        read_director_file_bytes(&self.bytes, &self.file_name, &self.base_url).map_err(|error| {
            ScriptError::new(format!(
                "failed to parse nested movie '{}': {error}",
                self.file_name
            ))
        })
    }
}

fn ready_movie_member_refs(
    candidates: impl IntoIterator<Item = (CastMemberRef, bool)>,
) -> Vec<CastMemberRef> {
    candidates
        .into_iter()
        .filter_map(|(member_ref, ready)| ready.then_some(member_ref))
        .collect()
}

struct NestedStartupGuard {
    session: RuntimeSessionHandle,
    child_id: PlayerId,
    child_owner: OwnerToken,
    command_tx: Sender<PlayerVMExecutionItem>,
    event_tx: Sender<PlayerVMEvent>,
    committed: bool,
}

impl NestedStartupGuard {
    fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for NestedStartupGuard {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        self.command_tx.close();
        self.event_tx.close();
        // Startup does not retain a session borrow across an await.  If a
        // competing synchronous borrow exists during cancellation, invalidate
        // the capability and let the session teardown drain the stale record.
        let retired = self
            .session
            .try_borrow_mut()
            .map(|mut session| {
                session.retire_nested_child_if_identity(self.child_id, &self.child_owner)
            })
            .unwrap_or(false);
        if retired {
            // Host teardown can re-enter the session, so this must run after
            // the RefMut from the identity check has been released.
            super::commands::drain_host_teardowns(&self.session);
        } else if !self.child_owner.is_arena_live() {
            // A reset may already have removed the record.  Keep this branch
            // side-effect free; the owner capability is already invalid.
            return;
        } else {
            self.child_owner.mark_arena_dead();
        }
    }
}

#[derive(Clone)]
pub(crate) struct NestedChildRecord {
    pub(crate) parent_id: PlayerId,
    pub(crate) parent_owner: OwnerToken,
    pub(crate) member_ref: CastMemberRef,
    pub(crate) child_id: PlayerId,
    pub(crate) child_owner: OwnerToken,
    pub(crate) command_tx: Sender<PlayerVMExecutionItem>,
    pub(crate) event_tx: Sender<PlayerVMEvent>,
    pub(crate) browser_owner_registered: bool,
}

#[derive(Default)]
pub(crate) struct NestedPlayerRegistry {
    next_id: PlayerId,
    children: HashMap<PlayerId, NestedChildRecord>,
    by_parent_member: HashMap<(OwnerKey, CastMemberRef), PlayerId>,
}

impl NestedPlayerRegistry {
    pub(crate) fn new() -> Self {
        Self {
            next_id: 1,
            ..Self::default()
        }
    }

    /// Allocate a positive id that is absent from both the session player
    /// graph and this registry.  The checked wrap test is intentionally
    /// parameterised so exhaustion is testable without constructing billions
    /// of players.
    pub(crate) fn allocate_id(
        &mut self,
        occupied: impl Fn(PlayerId) -> bool,
    ) -> Result<PlayerId, ScriptError> {
        self.allocate_id_with_limit(occupied, PlayerId::MAX)
    }

    fn allocate_id_with_limit(
        &mut self,
        occupied: impl Fn(PlayerId) -> bool,
        max_id: PlayerId,
    ) -> Result<PlayerId, ScriptError> {
        if max_id == 0 {
            return Err(ScriptError::new("nested player id space exhausted".into()));
        }
        let first = self.next_id.clamp(1, max_id);
        let mut candidate = first;
        loop {
            if !occupied(candidate) && !self.children.contains_key(&candidate) {
                self.next_id = if candidate == max_id {
                    1
                } else {
                    candidate + 1
                };
                return Ok(candidate);
            }
            candidate = if candidate == max_id {
                1
            } else {
                candidate + 1
            };
            if candidate == first {
                return Err(ScriptError::new("nested player id space exhausted".into()));
            }
        }
    }

    pub(crate) fn insert(&mut self, record: NestedChildRecord) -> Result<(), ScriptError> {
        if record.child_id == 0
            || !record.parent_owner.is_arena_live()
            || !record.child_owner.is_arena_live()
            || self.children.contains_key(&record.child_id)
        {
            return Err(ScriptError::new("invalid nested child registration".into()));
        }
        let key = (record.parent_owner.key(), record.member_ref);
        if self.by_parent_member.contains_key(&key) {
            return Err(ScriptError::new("nested movie is already active".into()));
        }
        self.by_parent_member.insert(key, record.child_id);
        self.children.insert(record.child_id, record);
        Ok(())
    }

    pub(crate) fn child_for(
        &self,
        parent_id: PlayerId,
        parent_owner: &OwnerToken,
        member_ref: CastMemberRef,
    ) -> Option<&NestedChildRecord> {
        let id = self
            .by_parent_member
            .get(&(parent_owner.key(), member_ref))?;
        let record = self.children.get(id)?;
        (record.parent_id == parent_id
            && record.parent_owner.same_identity(parent_owner)
            && record.parent_owner.is_arena_live()
            && record.child_owner.is_arena_live())
        .then_some(record)
    }

    pub(crate) fn child(
        &self,
        child_id: PlayerId,
        child_owner: &OwnerToken,
    ) -> Option<&NestedChildRecord> {
        let record = self.children.get(&child_id)?;
        (record.child_owner.same_identity(child_owner)
            && record.parent_owner.is_arena_live()
            && child_owner.is_arena_live())
        .then_some(record)
    }

    pub(crate) fn child_mut(
        &mut self,
        child_id: PlayerId,
        child_owner: &OwnerToken,
    ) -> Option<&mut NestedChildRecord> {
        let record = self.children.get_mut(&child_id)?;
        (record.child_owner.same_identity(child_owner)
            && record.parent_owner.is_arena_live()
            && child_owner.is_arena_live())
        .then_some(record)
    }

    pub(crate) fn child_mut_identity(
        &mut self,
        child_id: PlayerId,
        child_owner: &OwnerToken,
    ) -> Option<&mut NestedChildRecord> {
        let record = self.children.get_mut(&child_id)?;
        record
            .child_owner
            .same_identity(child_owner)
            .then_some(record)
    }

    pub(crate) fn replace_child_runtime(
        &mut self,
        child_id: PlayerId,
        old_owner: &OwnerToken,
        new_owner: OwnerToken,
        command_tx: Sender<PlayerVMExecutionItem>,
        event_tx: Sender<PlayerVMEvent>,
    ) -> Result<(PlayerId, OwnerToken), ScriptError> {
        let record = self
            .children
            .get_mut(&child_id)
            .filter(|record| record.child_owner.same_identity(old_owner))
            .ok_or_else(crate::player::cancelled_scope_error)?;
        let parent_id = record.parent_id;
        let parent_owner = record.parent_owner.clone();
        record.child_owner = new_owner;
        record.command_tx = command_tx;
        record.event_tx = event_tx;
        record.browser_owner_registered = false;
        Ok((parent_id, parent_owner))
    }

    /// Check only the capability identity for teardown.  Cancellation can mark
    /// the captured owner dead before the loop gets a final scheduling turn;
    /// requiring arena liveness here would leave the stale registry record in
    /// place.  Callers still use the full `child` lookup for normal routing.
    pub(crate) fn contains_child_identity(
        &self,
        child_id: PlayerId,
        child_owner: &OwnerToken,
    ) -> bool {
        self.children
            .get(&child_id)
            .is_some_and(|record| record.child_owner.same_identity(child_owner))
    }

    pub(crate) fn remove_child(&mut self, child_id: PlayerId) -> Option<NestedChildRecord> {
        let record = self.children.remove(&child_id)?;
        self.by_parent_member
            .remove(&(record.parent_owner.key(), record.member_ref));
        Some(record)
    }

    pub(crate) fn children_for(
        &self,
        parent_id: PlayerId,
        parent_owner: &OwnerToken,
    ) -> impl Iterator<Item = &NestedChildRecord> {
        self.children.values().filter(move |record| {
            record.parent_id == parent_id
                && record.parent_owner.same_identity(parent_owner)
                && record.parent_owner.is_arena_live()
                && record.child_owner.is_arena_live()
        })
    }

    pub(crate) fn take_children_for_parent(
        &mut self,
        parent_id: PlayerId,
        parent_owner: &OwnerToken,
    ) -> Vec<NestedChildRecord> {
        let mut ids: Vec<PlayerId> = self
            .children
            .iter()
            .filter_map(|(id, record)| {
                (record.parent_id == parent_id && record.parent_owner.same_identity(parent_owner))
                    .then_some(*id)
            })
            .collect();
        ids.sort_by_key(|id| {
            self.children
                .get(id)
                .map(|record| {
                    (
                        record.member_ref.cast_lib,
                        record.member_ref.cast_member,
                        *id,
                    )
                })
                .unwrap_or((i32::MAX, i32::MAX, *id))
        });
        ids.into_iter()
            .filter_map(|id| {
                let record = self.children.remove(&id)?;
                self.by_parent_member
                    .remove(&(record.parent_owner.key(), record.member_ref));
                Some(record)
            })
            .collect()
    }

    pub(crate) fn len(&self) -> usize {
        self.children.len()
    }
}

/// Parse, register, load, and initialize one linked movie.  Registration is
/// deliberately after parsing and every post-registration failure removes the
/// child before returning, so a bad linked file cannot leave a ghost player.
pub(crate) async fn start_nested_movie_owned(
    session: RuntimeSessionHandle,
    start: NestedMovieStart,
) -> Result<(PlayerId, OwnerToken), ScriptError> {
    let dir = start.parse()?;
    let (command_tx, command_rx) = async_std::channel::unbounded();
    let (event_tx, event_rx) = async_std::channel::unbounded();
    let (child_id, child_owner) = session.borrow_mut().register_nested_player(
        start.parent_id,
        &start.parent_owner,
        start.member_ref,
        command_tx.clone(),
        event_tx.clone(),
    )?;

    let startup_guard = NestedStartupGuard {
        session: session.clone(),
        child_id,
        child_owner: child_owner.clone(),
        command_tx: command_tx.clone(),
        event_tx: event_tx.clone(),
        committed: false,
    };

    // Register the exact child capability before any loop can dispatch a
    // Flash action. The session borrow used to capture the capability state
    // ends before the frontend callback factory is entered.
    let (flash_pending, flash_binding_tx) = session
        .borrow_mut()
        .with_player(child_id, |context| {
            (
                context.player.flash_scripted_access_pending.clone(),
                context.player.queue_tx.clone(),
            )
        })
        .ok_or_else(crate::player::cancelled_scope_error)?;
    let browser_owner_capability = crate::BrowserOwnerCapability::new_child(
        session.clone(),
        child_id,
        child_owner.clone(),
        flash_pending,
        flash_binding_tx,
    );
    let parent_owner_key = owner_key_string(&start.parent_owner);
    let child_owner_key = owner_key_string(&child_owner);
    #[cfg(target_arch = "wasm32")]
    {
        let browser_owner_capability: wasm_bindgen::JsValue = browser_owner_capability.into();
        crate::js_api::JsApi::register_nested_browser_owner(
            &parent_owner_key,
            &child_owner_key,
            &browser_owner_capability,
        )?;
        let mark_result = {
            let result = session.borrow_mut().mark_nested_browser_owner_registered(
                start.parent_id,
                &start.parent_owner,
                child_id,
                &child_owner,
            );
            result
        };
        if let Err(error) = mark_result {
            // The callback may synchronously reset either side of the pair.
            // Retire the exact registration before startup guard cleanup so a
            // replacement child can never inherit this host.
            if let Err(retire_error) = crate::js_api::JsApi::retire_nested_browser_owner(
                &parent_owner_key,
                &child_owner_key,
            ) {
                log::error!("nested Flash registration rollback failed: {retire_error}");
            }
            return Err(error);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = browser_owner_capability;
        session.borrow_mut().activate_nested_browser_owner_route(
            start.parent_id,
            &start.parent_owner,
            child_id,
            &child_owner,
        )?;
    }

    // Start servicing the child before load/init. Startup callbacks can issue
    // PumpPending work; starting these loops afterwards would deadlock them.
    let command_session = session.clone();
    let command_owner = child_owner.clone();
    async_std::task::spawn_local(async move {
        super::commands::run_command_loop(command_rx, command_session, child_id, command_owner)
            .await;
    });
    let event_session = session.clone();
    let event_owner = child_owner.clone();
    async_std::task::spawn_local(async move {
        super::events::run_event_loop(event_rx, event_session, child_id, event_owner).await;
    });

    if let Err(error) =
        super::load_movie_from_dir_owned(session.clone(), child_id, child_owner.clone(), dir).await
    {
        return Err(error);
    }

    let font = start.system_font.clone();
    let font_result = session
        .borrow_mut()
        .with_player(child_id, |context| {
            if !child_owner.same_identity(&context.player.owner) || !child_owner.is_arena_live() {
                return Err(crate::player::cancelled_scope_error());
            }
            if context.player.font_manager.system_font.is_none() {
                context.player.font_manager.system_font = font;
            }
            context.player.is_playing = true;
            Ok(())
        })
        .ok_or_else(crate::player::cancelled_scope_error)?;
    if let Err(error) = font_result {
        return Err(error);
    }

    if let Err(error) =
        super::run_movie_init_owned(session.clone(), child_id, child_owner.clone()).await
    {
        return Err(error);
    }
    startup_guard.commit();
    Ok((child_id, child_owner))
}

/// Rotate one linked child without leaving its old command/event loops or
/// frontend owner alive. The old senders are closed only after reset
/// preflight and owner rotation succeed; fresh channels are installed and
/// their loops are spawned only after the exact replacement Flash owner has
/// been registered.
pub(crate) fn reset_nested_player_owned(
    session: RuntimeSessionHandle,
    child_id: PlayerId,
    old_owner: OwnerToken,
) -> Result<OwnerToken, ScriptError> {
    let old_record = session
        .borrow()
        .nested_child_owned(child_id, &old_owner)
        .ok_or_else(crate::player::cancelled_scope_error)?;
    let parent_id = old_record.parent_id;
    let parent_owner = old_record.parent_owner.clone();
    let new_owner = session
        .borrow_mut()
        .reset_player_owned(child_id, &old_owner)?;
    // Only close the old runtime after reset preflight and owner rotation have
    // succeeded. Exhaustion must leave both old channels usable.
    old_record.command_tx.close();
    old_record.event_tx.close();
    // The reset queues exact old child retirement while the borrow is held.
    // Deliver it before publishing the replacement key.
    super::commands::drain_host_teardowns(&session);

    let (command_tx, command_rx) = async_std::channel::unbounded();
    let (event_tx, event_rx) = async_std::channel::unbounded();
    {
        let mut runtime = session.borrow_mut();
        let replace_result = runtime.replace_nested_child_runtime(
            child_id,
            &old_owner,
            new_owner.clone(),
            command_tx.clone(),
            event_tx.clone(),
        );
        if let Err(error) = replace_result {
            drop(runtime);
            session
                .borrow_mut()
                .retire_nested_child_if_owner(child_id, &new_owner);
            super::commands::drain_host_teardowns(&session);
            return Err(error);
        }
        let install_result = runtime.with_player(child_id, |context| {
            context.player.queue_tx = command_tx.clone();
            context
                .player
                .set_nested_flash_host_route(super::FlashHostRoute::NestedPending);
        });
        if install_result.is_none() {
            drop(runtime);
            session
                .borrow_mut()
                .retire_nested_child_if_owner(child_id, &new_owner);
            super::commands::drain_host_teardowns(&session);
            return Err(crate::player::cancelled_scope_error());
        }
    }

    let (flash_pending, flash_binding_tx) = session
        .borrow_mut()
        .with_player(child_id, |context| {
            (
                context.player.flash_scripted_access_pending.clone(),
                context.player.queue_tx.clone(),
            )
        })
        .ok_or_else(crate::player::cancelled_scope_error)?;
    let browser_owner_capability = crate::BrowserOwnerCapability::new_child(
        session.clone(),
        child_id,
        new_owner.clone(),
        flash_pending,
        flash_binding_tx,
    );
    let parent_owner_key = owner_key_string(&parent_owner);
    let child_owner_key = owner_key_string(&new_owner);
    #[cfg(target_arch = "wasm32")]
    {
        let browser_owner_capability: wasm_bindgen::JsValue = browser_owner_capability.into();
        if let Err(error) = crate::js_api::JsApi::register_nested_browser_owner(
            &parent_owner_key,
            &child_owner_key,
            &browser_owner_capability,
        ) {
            session
                .borrow_mut()
                .retire_nested_child_if_owner(child_id, &new_owner);
            super::commands::drain_host_teardowns(&session);
            return Err(error);
        }
        let mark_result = {
            let result = session.borrow_mut().mark_nested_browser_owner_registered(
                parent_id,
                &parent_owner,
                child_id,
                &new_owner,
            );
            result
        };
        if let Err(error) = mark_result {
            if let Err(retire_error) = crate::js_api::JsApi::retire_nested_browser_owner(
                &parent_owner_key,
                &child_owner_key,
            ) {
                log::error!("nested Flash reset rollback failed: {retire_error}");
            }
            session
                .borrow_mut()
                .retire_nested_child_if_owner(child_id, &new_owner);
            super::commands::drain_host_teardowns(&session);
            return Err(error);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = browser_owner_capability;
        let activate_result = {
            let result = session.borrow_mut().activate_nested_browser_owner_route(
                parent_id,
                &parent_owner,
                child_id,
                &new_owner,
            );
            result
        };
        if let Err(error) = activate_result {
            session
                .borrow_mut()
                .retire_nested_child_if_owner(child_id, &new_owner);
            return Err(error);
        }
    }

    let command_session = session.clone();
    let command_owner = new_owner.clone();
    async_std::task::spawn_local(async move {
        super::commands::run_command_loop(command_rx, command_session, child_id, command_owner)
            .await;
    });
    let event_session = session.clone();
    let event_owner = new_owner.clone();
    async_std::task::spawn_local(async move {
        super::events::run_event_loop(event_rx, event_session, child_id, event_owner).await;
    });
    Ok(new_owner)
}

/// Discover linked movie sprites after the parent frame has settled.  The
/// channel order is retained so activation timing follows the authored score;
/// already registered parent/member pairs are skipped.
pub(crate) async fn activate_nested_players_owned(
    session: RuntimeSessionHandle,
    parent_id: PlayerId,
    parent_owner: OwnerToken,
) -> Result<usize, ScriptError> {
    let refs = session
        .borrow_mut()
        .with_player(parent_id, |context| {
            if !parent_owner.same_identity(&context.player.owner) || !parent_owner.is_arena_live() {
                return Err(crate::player::cancelled_scope_error());
            }
            let candidates: Vec<(CastMemberRef, bool)> = context
                .player
                .movie
                .score
                .channels
                .iter()
                .filter_map(|channel| {
                    let member_ref = channel.sprite.member?;
                    let member = context
                        .player
                        .movie
                        .cast_manager
                        .find_member_by_ref(&member_ref)?;
                    let ready = matches!(&member.member_type, CastMemberType::Movie(movie) if movie.bytes.is_some());
                    Some((member_ref, ready))
                })
                .collect();
            Ok(ready_movie_member_refs(candidates))
        })
        .ok_or_else(crate::player::cancelled_scope_error)??;
    let mut starts = Vec::new();
    let mut seen = HashSet::new();
    for member_ref in refs {
        if !seen.insert(member_ref) {
            continue;
        }
        if session
            .borrow()
            .nested_child_for(parent_id, &parent_owner, member_ref)
            .is_some()
        {
            continue;
        }
        let start = session
            .borrow_mut()
            .with_player(parent_id, |context| {
                NestedMovieStart::from_context(&context, member_ref)
            })
            .ok_or_else(crate::player::cancelled_scope_error)??;
        starts.push(start);
    }
    let mut started = 0;
    for start in starts {
        match start_nested_movie_owned(session.clone(), start).await {
            Ok(_) => started += 1,
            Err(error) => {
                // A linked movie can legitimately be unavailable while its
                // parent continues; the failed child was rolled back by the
                // start routine, so report and continue with other channels.
                log::warn!("nested movie activation failed: {}", error);
            }
        }
    }
    Ok(started)
}

/// Advance all children of one parent at the same captured timestamp.  A
/// stale child is retired by session teardown; it is skipped here so a reset
/// cannot mutate a replacement player.
pub(crate) async fn advance_nested_players_owned(
    session: RuntimeSessionHandle,
    parent_id: PlayerId,
    parent_owner: OwnerToken,
    frame_now_ms: f64,
) -> Result<(), ScriptError> {
    let children = session
        .borrow()
        .nested_children_for(parent_id, &parent_owner);
    for child in children {
        if !child.child_owner.is_arena_live() {
            continue;
        }
        // Children may themselves contain linked movies. Heap-box this edge
        // so the async frame future has a finite type instead of recursive
        // E0733 expansion.
        Box::pin(super::run_single_frame_owned_at(
            session.clone(),
            child.child_id,
            child.child_owner,
            frame_now_ms,
        ))
        .await?;
    }
    Ok(())
}

/// Render children into the parent's bitmap manager after their frame work.
/// The parent and child borrows are disjoint and never cross an await.  The
/// existing Movie sprite renderer consumes `nested_movie_images`, preserving
/// the authored sprite rect/crop/center composition rules.
pub(crate) fn render_nested_players_owned(
    session: &RuntimeSessionHandle,
    parent_id: PlayerId,
    parent_owner: &OwnerToken,
) -> Result<usize, ScriptError> {
    let children = session
        .borrow()
        .nested_children_for(parent_id, parent_owner);
    let mut rendered_count = 0;
    for child in children {
        let rendered = session
            .borrow_mut()
            .with_player(child.child_id, |context| {
                if !child.child_owner.same_identity(&context.player.owner)
                    || !child.child_owner.is_arena_live()
                {
                    return None;
                }
                super::render_nested_player_bitmap(
                    context.player,
                    Some(context.symbols),
                    child.child_id as usize,
                )
            })
            .flatten();
        let Some(bitmap) = rendered else { continue };
        session
            .borrow_mut()
            .with_player(parent_id, |context| {
                if !parent_owner.same_identity(&context.player.owner)
                    || !parent_owner.is_arena_live()
                {
                    return;
                }
                let member_ref = child.member_ref;
                let (w, h) = (bitmap.width, bitmap.height);
                let reuse = context.player.nested_movie_images.get(&member_ref).copied().filter(|r| {
                    matches!(context.player.bitmap_manager.get_bitmap(*r), Some(b) if b.width == w && b.height == h)
                });
                match reuse {
                    Some(reference) => context.player.bitmap_manager.replace_bitmap(reference, bitmap),
                    None => {
                        let reference = context.player.bitmap_manager.add_bitmap(bitmap);
                        context.player.nested_movie_images.insert(member_ref, reference);
                    }
                }
                context.player.stage_dirty = true;
            });
        rendered_count += 1;
    }
    Ok(rendered_count)
}

/// Route browser pointer commands through the child's owner-bound command
/// loop. `PlayerVMEvent` is reserved for VM callbacks; browser mouse commands
/// must execute the full MouseDown/MouseUp/MouseMove pipeline.
pub(crate) fn enqueue_nested_pointer_command_owned(
    session: &RuntimeSessionHandle,
    parent_id: PlayerId,
    parent_owner: &OwnerToken,
    member_ref: CastMemberRef,
    channel_num: i16,
    host_x: i32,
    host_y: i32,
    command: PlayerVMCommand,
) -> Result<(i32, i32), ScriptError> {
    let (child, local) = map_nested_pointer_owned(
        session,
        parent_id,
        parent_owner,
        member_ref,
        channel_num,
        host_x,
        host_y,
    )?;
    // The parent hit-test uses host coordinates, but the child command
    // pipeline must observe its own stage coordinates.  Do not forward the
    // original parent-space tuple after `map_nested_pointer_owned` updated the
    // child's mouse location.
    let command = match command {
        PlayerVMCommand::MouseDown(_) => PlayerVMCommand::MouseDown(local),
        PlayerVMCommand::MouseUp(_) => PlayerVMCommand::MouseUp(local),
        PlayerVMCommand::MouseMove(_) => PlayerVMCommand::MouseMove(local),
        PlayerVMCommand::RightMouseDown(_) => PlayerVMCommand::RightMouseDown(local),
        PlayerVMCommand::RightMouseUp(_) => PlayerVMCommand::RightMouseUp(local),
        other => other,
    };
    session
        .borrow_mut()
        .with_player(child.child_id, |context| {
            if !child.child_owner.same_identity(&context.player.owner)
                || !child.child_owner.is_arena_live()
            {
                return Err(crate::player::cancelled_scope_error());
            }
            match &command {
                PlayerVMCommand::MouseDown(_) => context.player.movie.mouse_down = true,
                PlayerVMCommand::MouseUp(_) => context.player.movie.mouse_down = false,
                PlayerVMCommand::RightMouseDown(_) => context.player.movie.right_mouse_down = true,
                PlayerVMCommand::RightMouseUp(_) => context.player.movie.right_mouse_down = false,
                PlayerVMCommand::MouseMove(_) => {}
                _ => {
                    return Err(ScriptError::new("invalid nested pointer command".into()));
                }
            }
            Ok(())
        })
        .ok_or_else(crate::player::cancelled_scope_error)??;
    child
        .command_tx
        .try_send(PlayerVMExecutionItem {
            command,
            completer: None,
        })
        .map_err(|_| ScriptError::new("nested player command loop is closed".into()))?;
    Ok(local)
}

fn map_nested_pointer_owned(
    session: &RuntimeSessionHandle,
    parent_id: PlayerId,
    parent_owner: &OwnerToken,
    member_ref: CastMemberRef,
    channel_num: i16,
    host_x: i32,
    host_y: i32,
) -> Result<(super::nested::NestedChildRecord, (i32, i32)), ScriptError> {
    let child = session
        .borrow()
        .nested_child_for(parent_id, parent_owner, member_ref)
        .ok_or_else(crate::player::cancelled_scope_error)?;
    let (rect, crop, center) = session
        .borrow_mut()
        .with_player(parent_id, |context| {
            context
                .player
                .movie
                .score
                .channels
                .iter()
                .find(|channel| {
                    channel.number as i16 == channel_num
                        && channel.sprite.member == Some(member_ref)
                })
                .map(|channel| {
                    let rect =
                        super::score::get_concrete_sprite_rect(&context.player, &channel.sprite);
                    let (crop, center) = context
                        .player
                        .movie
                        .cast_manager
                        .find_member_by_ref(&member_ref)
                        .and_then(|member| match &member.member_type {
                            CastMemberType::Movie(movie) => Some((movie.crop, movie.center)),
                            _ => None,
                        })
                        .unwrap_or((false, false));
                    (rect, crop, center)
                })
        })
        .flatten()
        .ok_or_else(crate::player::cancelled_scope_error)?;
    let (child_width, child_height) = session
        .borrow_mut()
        .with_player(child.child_id, |context| {
            (
                context.player.movie.rect.width().max(1),
                context.player.movie.rect.height().max(1),
            )
        })
        .ok_or_else(crate::player::cancelled_scope_error)?;
    let dest_width = rect.width().max(1);
    let dest_height = rect.height().max(1);
    let local = if crop {
        let offset_x = if center {
            (child_width - dest_width) / 2
        } else {
            0
        };
        let offset_y = if center {
            (child_height - dest_height) / 2
        } else {
            0
        };
        (host_x - rect.left + offset_x, host_y - rect.top + offset_y)
    } else {
        (
            ((host_x - rect.left) as i64 * child_width as i64 / dest_width as i64) as i32,
            ((host_y - rect.top) as i64 * child_height as i64 / dest_height as i64) as i32,
        )
    };
    session
        .borrow_mut()
        .with_player(child.child_id, |context| {
            if !child.child_owner.same_identity(&context.player.owner)
                || !child.child_owner.is_arena_live()
            {
                return Err(crate::player::cancelled_scope_error());
            }
            context.player.mouse_loc = local;
            Ok(())
        })
        .ok_or_else(crate::player::cancelled_scope_error)??;
    Ok((child, local))
}

/// Return the exact linked movie channel under a parent-space point. Frontend
/// input adapters call this before the owner-bound command route, then retain
/// the channel number through coordinate mapping and dispatch.
pub(crate) fn nested_hit_target_owned(
    session: &RuntimeSessionHandle,
    parent_id: PlayerId,
    parent_owner: &OwnerToken,
    host_x: i32,
    host_y: i32,
) -> Option<(CastMemberRef, i16)> {
    // Snapshot hit rectangles without holding the session borrow while we
    // validate the owner-qualified child capability.  A normal bitmap/text
    // sprite may be in front of a linked movie sprite; it must not consume
    // the hit-test merely because it has a member reference.
    let candidates = session
        .borrow_mut()
        .with_player(parent_id, |context| {
            if !parent_owner.same_identity(&context.player.owner) || !parent_owner.is_arena_live() {
                return Vec::new();
            }
            context
                .player
                .movie
                .score
                .get_sorted_channels(context.player.movie.current_frame)
                .iter()
                .rev()
                .filter(|channel| channel.sprite.visible)
                .filter_map(|channel| {
                    let member_ref = channel.sprite.member?;
                    super::score::concrete_sprite_hit_test(
                        &context.player,
                        &channel.sprite,
                        host_x,
                        host_y,
                    )
                    .then_some((member_ref, channel.number as i16))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    candidates.into_iter().find(|(member_ref, _channel_num)| {
        session
            .borrow()
            .nested_child_for(parent_id, parent_owner, *member_ref)
            .is_some()
    })
}

/// Mirror a browser key state and command into every visible linked child.
/// Children are stable-sorted by the session registry; the host's parent state
/// is updated by BrowserPlayerHandle before this helper is called.
pub(crate) fn enqueue_nested_key_command_owned(
    session: &RuntimeSessionHandle,
    parent_id: PlayerId,
    parent_owner: &OwnerToken,
    key: &str,
    code: u16,
    is_down: bool,
) -> Result<usize, ScriptError> {
    let visible_members: HashSet<CastMemberRef> = session
        .borrow_mut()
        .with_player(parent_id, |context| {
            context
                .player
                .movie
                .score
                .channels
                .iter()
                .filter(|channel| channel.sprite.visible)
                .filter_map(|channel| channel.sprite.member)
                .collect()
        })
        .ok_or_else(crate::player::cancelled_scope_error)?;
    let children = session
        .borrow()
        .nested_children_for(parent_id, parent_owner)
        .into_iter()
        .filter(|child| visible_members.contains(&child.member_ref))
        .collect::<Vec<_>>();
    let mut sent = 0;
    for child in children {
        session
            .borrow_mut()
            .with_player(child.child_id, |context| {
                if !child.child_owner.same_identity(&context.player.owner)
                    || !child.child_owner.is_arena_live()
                {
                    return Err(crate::player::cancelled_scope_error());
                }
                if is_down {
                    context
                        .player
                        .keyboard_manager
                        .key_down(key.to_owned(), code);
                } else {
                    context.player.keyboard_manager.key_up(key, code);
                }
                Ok(())
            })
            .ok_or_else(crate::player::cancelled_scope_error)??;
        let command = if is_down {
            PlayerVMCommand::KeyDown(key.to_owned(), code)
        } else {
            PlayerVMCommand::KeyUp(key.to_owned(), code)
        };
        child
            .command_tx
            .try_send(PlayerVMExecutionItem {
                command,
                completer: None,
            })
            .map_err(|_| ScriptError::new("nested player command loop is closed".into()))?;
        sent += 1;
    }
    Ok(sent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::player::geometry::IntRect;
    use crate::player::score::SpriteChannel;
    use crate::player::session::RuntimeSession;
    use crate::player::symbols::symbol_table::SymbolOwner;
    use manual_future::ManualFuture;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    fn owner(session: u64, player: u64) -> OwnerToken {
        OwnerToken::new(OwnerKey {
            session,
            player,
            generation: 1,
        })
    }

    fn record(parent: PlayerId, parent_owner: OwnerToken, child: PlayerId) -> NestedChildRecord {
        let child_owner = owner(1, child as u64);
        let (command_tx, _) = async_std::channel::unbounded();
        let (event_tx, _) = async_std::channel::unbounded();
        NestedChildRecord {
            parent_id: parent,
            parent_owner,
            member_ref: CastMemberRef {
                cast_lib: 1,
                cast_member: child as i32,
            },
            child_id: child,
            child_owner,
            command_tx,
            event_tx,
            browser_owner_registered: false,
        }
    }

    #[test]
    fn allocation_skips_live_players_and_reports_bounded_exhaustion() {
        let mut registry = NestedPlayerRegistry::new();
        registry.next_id = 3;
        assert_eq!(registry.allocate_id(|id| id == 1).unwrap(), 3);
        let mut bounded = NestedPlayerRegistry::new();
        bounded.next_id = 1;
        let parent = owner(1, 1);
        for id in 1..=3 {
            bounded.insert(record(1, parent.clone(), id)).unwrap();
        }
        bounded.next_id = 2;
        assert!(bounded.allocate_id_with_limit(|_| false, 3).is_err());
    }

    #[test]
    fn registration_rejects_foreign_parent_and_take_is_recursive_per_parent() {
        let mut registry = NestedPlayerRegistry::new();
        let parent = owner(1, 7);
        let other = owner(1, 8);
        let first = record(7, parent.clone(), 11);
        let member = first.member_ref;
        registry.insert(first).unwrap();
        assert!(registry.child_for(8, &other, member).is_none());
        assert!(registry.insert(record(7, parent.clone(), 12)).is_ok());
        let taken = registry.take_children_for_parent(7, &parent);
        assert_eq!(taken.len(), 2);
        assert_eq!(
            taken
                .iter()
                .map(|record| record.member_ref.cast_member)
                .collect::<Vec<_>>(),
            vec![11, 12]
        );
        assert_eq!(registry.len(), 0);
        assert!(registry.child_for(7, &parent, member).is_none());
    }

    #[test]
    fn stale_child_capability_cannot_be_selected_or_routed() {
        let mut registry = NestedPlayerRegistry::new();
        let parent = owner(2, 1);
        let child = record(1, parent.clone(), 9);
        let member = child.member_ref;
        let child_owner = child.child_owner.clone();
        registry.insert(child).unwrap();
        child_owner.mark_arena_dead();
        assert!(registry.child_for(1, &parent, member).is_none());
        assert!(registry.child(9, &child_owner).is_none());
        assert!(registry.contains_child_identity(9, &child_owner));
        assert_eq!(registry.take_children_for_parent(1, &parent).len(), 1);
    }

    #[test]
    fn stale_identity_cleanup_cannot_remove_numeric_replacement() {
        let mut registry = NestedPlayerRegistry::new();
        let parent = owner(3, 1);
        let child = record(1, parent.clone(), 9);
        let stale = child.child_owner.clone();
        registry.insert(child).unwrap();
        assert!(registry.contains_child_identity(9, &stale));
        let replacement = owner(3, 9);
        assert!(!stale.same_identity(&replacement));
        assert!(!registry.contains_child_identity(9, &replacement));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn owned_nested_start_mounts_d5_channel_one_in_child_context() {
        let session = Rc::new(RefCell::new(RuntimeSession::new(SymbolOwner {
            session: 79,
            generation: 1,
        })));
        let (parent_tx, _parent_rx) = async_std::channel::unbounded();
        assert!(session.borrow_mut().add_player(1, parent_tx));
        let parent_owner = session
            .borrow_mut()
            .with_player(1, |context| context.player.owner.clone())
            .unwrap();
        let parent_member = CastMemberRef {
            cast_lib: 1,
            cast_member: 91,
        };
        session
            .borrow_mut()
            .with_player(1, |context| {
                let mut channel = SpriteChannel::new(1);
                channel.sprite.member = Some(parent_member);
                channel.sprite.entered = true;
                context.player.movie.score.channels = vec![SpriteChannel::new(0), channel];
            })
            .unwrap();

        let make_start = || NestedMovieStart {
            parent_id: 1,
            parent_owner: parent_owner.clone(),
            member_ref: CastMemberRef {
                cast_lib: 1,
                cast_member: 1,
            },
            bytes: Rc::new(include_bytes!("../../tests/fixtures/nested_flash_a.dcr").to_vec()),
            base_url: "https://fixture.invalid/".to_owned(),
            file_name: "nested_flash_a.dcr".to_owned(),
            system_font: None,
        };
        let parsed = make_start().parse().expect("D5 fixture should parse");
        let frame_pair = parsed.score.as_ref().and_then(|score| {
            score
                .frame_data
                .frame_channel_data
                .iter()
                .find(|(frame, channel, _)| *frame == 0 && *channel == 6)
                .map(|(_, _, data)| (data.cast_lib, data.cast_member))
        });
        assert_eq!(frame_pair, Some((1, 1)));

        // Exercise the production loader up to the host-emission boundary.
        // This proves the parsed D5 channel is mounted into the selected child
        // before native startup rejects the first Flash action.
        let (child_id, child_owner) = async_std::task::block_on(async {
            let start = make_start();
            let dir = start.parse()?;
            let (command_tx, _command_rx) = async_std::channel::unbounded();
            let (event_tx, _event_rx) = async_std::channel::unbounded();
            let (child_id, child_owner) = session.borrow_mut().register_nested_player(
                start.parent_id,
                &start.parent_owner,
                start.member_ref,
                command_tx,
                event_tx,
            )?;
            session.borrow_mut().activate_nested_browser_owner_route(
                start.parent_id,
                &start.parent_owner,
                child_id,
                &child_owner,
            )?;
            super::super::load_movie_from_dir_owned(
                session.clone(),
                child_id,
                child_owner.clone(),
                dir,
            )
            .await?;
            Ok::<_, ScriptError>((child_id, child_owner))
        })
        .expect("production child mount should finish before host emission");

        let (root_member, root_entered) = session
            .borrow_mut()
            .with_player(1, |context| {
                (
                    context
                        .player
                        .movie
                        .score
                        .get_sprite(1)
                        .and_then(|sprite| sprite.member),
                    context
                        .player
                        .movie
                        .score
                        .get_sprite(1)
                        .is_some_and(|sprite| sprite.entered),
                )
            })
            .unwrap();
        let (child_member, child_entered, child_props_applied, child_script_wrote, child_route) =
            session
                .borrow_mut()
                .with_player(child_id, |context| {
                    let sprite = context.player.movie.score.get_sprite(1);
                    (
                        sprite.and_then(|sprite| sprite.member),
                        sprite.is_some_and(|sprite| sprite.entered),
                        sprite.is_some_and(|sprite| sprite.score_props_already_applied),
                        sprite.is_some_and(|sprite| sprite.script_wrote_since_span_init),
                        context.player.flash_host_route,
                    )
                })
                .unwrap();
        eprintln!(
            "owned nested D5 mount: frame_pair={frame_pair:?} root_member={root_member:?} root_entered={root_entered} child_id={child_id} child_owner={:?} child_member={child_member:?} child_entered={child_entered} props_applied={child_props_applied} script_wrote={child_script_wrote} route={child_route:?}",
            child_owner.key(),
        );
        assert_eq!(child_owner.key().player, child_id as u64);
        assert_eq!(root_member, Some(parent_member));
        assert!(root_entered);
        assert_eq!(
            child_member,
            Some(CastMemberRef {
                cast_lib: 1,
                cast_member: 1
            })
        );
        assert!(!child_entered);
        assert!(child_props_applied);
        assert!(!child_script_wrote);
        assert_eq!(child_route, super::super::FlashHostRoute::LocalOwned);

        session.borrow_mut().remove_player(child_id);

        let error = async_std::task::block_on(async {
            start_nested_movie_owned(session.clone(), make_start()).await
        })
        .expect_err("native nested startup must reject the first Flash host action");
        assert_eq!(error.message, "Flash host is unavailable on native");
        assert!(session
            .borrow()
            .nested_children_for(1, &parent_owner)
            .is_empty());
        assert_eq!(session.borrow().players().len(), 1);

        let (root_member, root_entered) = session
            .borrow_mut()
            .with_player(1, |context| {
                (
                    context
                        .player
                        .movie
                        .score
                        .get_sprite(1)
                        .and_then(|sprite| sprite.member),
                    context
                        .player
                        .movie
                        .score
                        .get_sprite(1)
                        .is_some_and(|sprite| sprite.entered),
                )
            })
            .unwrap();
        assert_eq!(root_member, Some(parent_member));
        assert!(root_entered);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn direct_nested_reset_replaces_owner_and_both_runtime_channels() {
        let session = Rc::new(RefCell::new(RuntimeSession::new(SymbolOwner {
            session: 77,
            generation: 1,
        })));
        let (parent_tx, _parent_rx) = async_std::channel::unbounded();
        assert!(session.borrow_mut().add_player(1, parent_tx));
        let parent_owner = session
            .borrow_mut()
            .with_player(1, |context| context.player.owner.clone())
            .unwrap();
        let (old_command_tx, old_command_rx) = async_std::channel::unbounded();
        let (old_event_tx, old_event_rx) = async_std::channel::unbounded();
        let (child_id, old_owner) = session
            .borrow_mut()
            .register_nested_player(
                1,
                &parent_owner,
                CastMemberRef {
                    cast_lib: 3,
                    cast_member: 4,
                },
                old_command_tx.clone(),
                old_event_tx.clone(),
            )
            .unwrap();
        session
            .borrow_mut()
            .activate_nested_browser_owner_route(1, &parent_owner, child_id, &old_owner)
            .unwrap();

        let new_owner = async_std::task::block_on(async {
            reset_nested_player_owned(session.clone(), child_id, old_owner.clone())
        })
        .unwrap();
        assert!(!old_owner.same_identity(&new_owner));
        assert!(old_command_tx
            .try_send(PlayerVMExecutionItem {
                command: PlayerVMCommand::PumpPending,
                completer: None,
            })
            .is_err());
        while old_command_rx.try_recv().is_ok() {}
        while old_event_rx.try_recv().is_ok() {}
        assert!(matches!(
            old_command_rx.try_recv(),
            Err(async_std::channel::TryRecvError::Closed)
        ));
        assert!(matches!(
            old_event_rx.try_recv(),
            Err(async_std::channel::TryRecvError::Closed)
        ));

        // reset_owned_core intentionally stops a player.  The replacement
        // loops are live, but this direct channel test must put the child
        // back into playback before exercising the MouseDown command path.
        session
            .borrow_mut()
            .with_player(child_id, |context| {
                context.player.is_playing = true;
            })
            .unwrap();

        let replacement = session
            .borrow()
            .nested_child_owned(child_id, &new_owner)
            .expect("replacement nested record");
        async_std::task::block_on(async {
            replacement
                .event_tx
                .try_send(PlayerVMEvent::Global(
                    crate::player::symbols::symbol::Symbol::builtin(
                        crate::player::symbols::builtin::BuiltInSymbol::BeginSprite,
                    ),
                    vec![],
                ))
                .unwrap();
            let (command_future, command_completer) = ManualFuture::new();
            replacement
                .command_tx
                .send(PlayerVMExecutionItem {
                    command: PlayerVMCommand::MouseDown((1, 1)),
                    completer: Some(command_completer),
                })
                .await
                .unwrap();
            async_std::future::timeout(Duration::from_millis(250), command_future)
                .await
                .expect("replacement command loop did not complete")
                .expect("replacement MouseDown command failed");
            let deadline = Instant::now() + Duration::from_millis(250);
            while replacement.event_tx.len() != 0 {
                assert!(
                    Instant::now() < deadline,
                    "replacement event loop did not consume event"
                );
                async_std::task::yield_now().await;
            }
        });
        assert!(session
            .borrow_mut()
            .with_player(child_id, |context| context.player.movie.mouse_down)
            .unwrap());
        assert!(!replacement.event_tx.is_closed());

        let (flash_pending, flash_binding_tx) = session
            .borrow_mut()
            .with_player(child_id, |context| {
                (
                    context.player.flash_scripted_access_pending.clone(),
                    context.player.queue_tx.clone(),
                )
            })
            .unwrap();
        let capability = crate::BrowserOwnerCapability::new_child(
            session.clone(),
            child_id,
            new_owner,
            flash_pending,
            flash_binding_tx,
        );
        capability
            .update_flash_frame(1, 1, 1, &[255, 0, 0, 255])
            .unwrap();
        assert!(session
            .borrow_mut()
            .with_player(child_id, |context| {
                context.player.flash_frame_buffers.contains_key(&1)
            })
            .unwrap());
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn nested_reset_exhaustion_preserves_old_owner_and_runtime_channels() {
        let session = Rc::new(RefCell::new(RuntimeSession::new(SymbolOwner {
            session: 78,
            generation: u64::MAX,
        })));
        let (parent_tx, _parent_rx) = async_std::channel::unbounded();
        assert!(session.borrow_mut().add_player(1, parent_tx));
        let parent_owner = session
            .borrow_mut()
            .with_player(1, |context| context.player.owner.clone())
            .unwrap();
        let (command_tx, command_rx) = async_std::channel::unbounded();
        let (event_tx, event_rx) = async_std::channel::unbounded();
        let (child_id, child_owner) = session
            .borrow_mut()
            .register_nested_player(
                1,
                &parent_owner,
                CastMemberRef {
                    cast_lib: 4,
                    cast_member: 5,
                },
                command_tx.clone(),
                event_tx.clone(),
            )
            .unwrap();

        let result = async_std::task::block_on(async {
            reset_nested_player_owned(session.clone(), child_id, child_owner.clone())
        });
        assert!(result.is_err());
        assert!(child_owner.is_arena_live());
        assert!(session
            .borrow()
            .nested_child_owned(child_id, &child_owner)
            .is_some());
        assert!(session
            .borrow_mut()
            .with_player(child_id, |context| context
                .player
                .owner
                .same_identity(&child_owner))
            .unwrap());
        assert!(command_tx
            .try_send(PlayerVMExecutionItem {
                command: PlayerVMCommand::PumpPending,
                completer: None,
            })
            .is_ok());
        assert!(event_tx
            .try_send(PlayerVMEvent::Global(
                crate::player::symbols::symbol::Symbol::builtin(
                    crate::player::symbols::builtin::BuiltInSymbol::BeginSprite,
                ),
                vec![],
            ))
            .is_ok());
        assert!(command_rx.try_recv().is_ok());
        assert!(event_rx.try_recv().is_ok());
    }

    #[test]
    fn pointer_route_uses_topmost_visible_channel_and_child_coordinates() {
        let session = Rc::new(RefCell::new(RuntimeSession::new(SymbolOwner {
            session: 44,
            generation: 1,
        })));
        let (parent_tx, _parent_rx) = async_std::channel::unbounded();
        assert!(session.borrow_mut().add_player(1, parent_tx));
        let parent_owner = session
            .borrow_mut()
            .with_player(1, |context| context.player.owner.clone())
            .unwrap();
        let member = CastMemberRef {
            cast_lib: 1,
            cast_member: 10,
        };
        let other_member = CastMemberRef {
            cast_lib: 1,
            cast_member: 11,
        };
        let (child_tx, child_rx) = async_std::channel::unbounded();
        let (child_id, child_owner) = session
            .borrow_mut()
            .register_nested_player(
                1,
                &parent_owner,
                member,
                child_tx,
                async_std::channel::unbounded().0,
            )
            .unwrap();
        let (other_tx, _other_rx) = async_std::channel::unbounded();
        let (other_id, other_owner) = session
            .borrow_mut()
            .register_nested_player(
                1,
                &parent_owner,
                other_member,
                other_tx,
                async_std::channel::unbounded().0,
            )
            .unwrap();
        session.borrow_mut().with_player(child_id, |context| {
            context.player.movie.rect = IntRect::from_size(0, 0, 50, 40);
        });
        session.borrow_mut().with_player(other_id, |context| {
            context.player.movie.rect = IntRect::from_size(0, 0, 20, 20);
        });
        session.borrow_mut().with_player(1, |context| {
            let mut lower = SpriteChannel::new(1);
            lower.sprite.member = Some(member);
            lower.sprite.loc_h = 0;
            lower.sprite.loc_v = 0;
            lower.sprite.width = 250;
            lower.sprite.height = 100;
            lower.sprite.loc_z = 1;
            lower.sprite.puppet = true;

            let mut hidden = SpriteChannel::new(2);
            hidden.sprite.member = Some(other_member);
            hidden.sprite.loc_h = 250;
            hidden.sprite.loc_v = 0;
            hidden.sprite.width = 100;
            hidden.sprite.height = 100;
            hidden.sprite.loc_z = 3;
            hidden.sprite.visible = false;
            hidden.sprite.puppet = true;

            let mut same_member_top = SpriteChannel::new(3);
            same_member_top.sprite.member = Some(member);
            same_member_top.sprite.loc_h = 150;
            same_member_top.sprite.loc_v = 0;
            same_member_top.sprite.width = 100;
            same_member_top.sprite.height = 100;
            same_member_top.sprite.loc_z = 2;
            same_member_top.sprite.puppet = true;

            context.player.movie.score.channels =
                vec![SpriteChannel::new(0), lower, hidden, same_member_top];
            context.player.movie.score.invalidate_render_channel_cache();
        });

        // The hidden channel cannot consume the point, while the same-member
        // top channel still supplies its own rectangle for mapping.
        assert!(nested_hit_target_owned(&session, 1, &parent_owner, 300, 50).is_none());
        session.borrow_mut().with_player(1, |context| {
            context.player.movie.score.channels[2].sprite.visible = true;
            context.player.movie.score.invalidate_render_channel_cache();
        });
        assert_eq!(
            nested_hit_target_owned(&session, 1, &parent_owner, 300, 50),
            Some((other_member, 2))
        );
        assert_eq!(
            nested_hit_target_owned(&session, 1, &parent_owner, 175, 50),
            Some((member, 3))
        );
        enqueue_nested_pointer_command_owned(
            &session,
            1,
            &parent_owner,
            member,
            3,
            175,
            50,
            PlayerVMCommand::MouseDown((175, 50)),
        )
        .unwrap();
        match child_rx.try_recv().unwrap().command {
            PlayerVMCommand::MouseDown(local) => assert_eq!(local, (12, 20)),
            _ => panic!("nested pointer route queued the wrong command"),
        }
        assert!(session
            .borrow_mut()
            .with_player(child_id, |context| {
                context.player.movie.mouse_down
                    && context.player.mouse_loc == (12, 20)
                    && child_owner.same_identity(&context.player.owner)
            })
            .unwrap());
        session.borrow_mut().with_player(1, |context| {
            context.player.movie.score.channels[2].sprite.visible = false;
            context.player.movie.score.invalidate_render_channel_cache();
        });
        assert_eq!(
            enqueue_nested_key_command_owned(&session, 1, &parent_owner, "a", 65, true).unwrap(),
            1
        );
        match child_rx.try_recv().unwrap().command {
            PlayerVMCommand::KeyDown(key, code) => {
                assert_eq!(key, "a");
                assert_eq!(code, 65);
            }
            _ => panic!("nested key route queued the wrong command"),
        }
        assert!(session
            .borrow_mut()
            .with_player(child_id, |context| {
                context.player.keyboard_manager.is_key_down("a")
            })
            .unwrap());

        // Removing the child retires the registry capability; a replacement
        // with the same numeric id cannot receive the stale route.
        session.borrow_mut().remove_player(other_id);
        assert!(!other_owner.is_arena_live());
        let (replacement_tx, _replacement_rx) = async_std::channel::unbounded();
        assert!(session.borrow_mut().add_player(other_id, replacement_tx));
        assert!(nested_hit_target_owned(&session, 1, &parent_owner, 300, 50).is_none());
    }

    #[test]
    fn activation_filters_normal_and_unloaded_members() {
        let normal = CastMemberRef {
            cast_lib: 1,
            cast_member: 2,
        };
        let unloaded = CastMemberRef {
            cast_lib: 1,
            cast_member: 4,
        };
        let linked = CastMemberRef {
            cast_lib: 1,
            cast_member: 3,
        };
        assert_eq!(
            ready_movie_member_refs([(normal, false), (unloaded, false), (linked, true)]),
            vec![linked]
        );
    }
}
