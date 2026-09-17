//! Owned host events which can cross the runtime boundary after a player
//! borrow has ended.
//!
//! The event payloads in this module deliberately contain owned Rust values.
//! They never retain a `DatumRef`, `JsValue`, or a reference into a
//! `DirPlayer`, so a reset can invalidate the producing owner without making
//! an already detached event unsafe to inspect.  JavaScript conversion is a
//! boundary concern and happens only after the owner has been revalidated.

use std::collections::VecDeque;
use std::rc::{Rc, Weak};

use super::ownership::OwnerToken;

/// Maximum number of detached host events retained for one player generation.
/// State-like entries may replace an older entry of the same kind, but ordered
/// transitions are never discarded to make room for a newer transition.
pub const MAX_HOST_EVENTS: usize = 256;
/// One additional bounded slot is reserved for a terminal owner retirement.
/// This lets Drop release a bound sink even when ordinary lifecycle work has
/// filled the regular queue.
pub const HOST_EVENT_TERMINAL_RESERVE: usize = 1;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostEvent {
    /// A direct browser sink has become authoritative for this owner.
    OwnerBound {
        owner_key: String,
    },
    /// A direct browser sink is no longer authoritative for this owner.
    OwnerRetired {
        owner_key: String,
    },
    FrameChanged {
        frame: u32,
    },
    StageSizeChanged {
        width: u32,
        height: u32,
        center: bool,
    },
    MovieLoaded {
        version: u16,
        cast_names: Vec<String>,
    },
    MovieLoadFailed {
        path: String,
        error: String,
    },
    ScriptError {
        message: String,
    },
    ScriptErrorCleared,
    CastListChanged {
        names: Vec<String>,
    },
    CastNameChanged {
        cast: u32,
        name: String,
    },
    CastMemberListChanged {
        cast: u32,
        members: Vec<(u32, String)>,
    },
    /// The old owner must be torn down after the player borrow is released.
    /// This is intentionally an event, rather than a callback closure, so a
    /// replacement owner cannot accidentally consume it.
    FlashReset {
        owner_key: String,
    },
}

impl HostEvent {
    /// State snapshots may be coalesced.  Transitions such as load/failure,
    /// cast changes, and Flash teardown retain their insertion order.
    pub fn is_state_snapshot(&self) -> bool {
        matches!(
            self,
            Self::FrameChanged { .. } | Self::StageSizeChanged { .. }
        )
    }

    fn same_snapshot_kind(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::FrameChanged { .. }, Self::FrameChanged { .. })
                | (Self::StageSizeChanged { .. }, Self::StageSizeChanged { .. })
                | (Self::ScriptError { .. }, Self::ScriptError { .. })
                | (Self::ScriptErrorCleared, Self::ScriptErrorCleared)
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostEventOverflow {
    pub capacity: usize,
}

#[derive(Clone, Debug)]
pub struct NativeHostEvent {
    pub player_id: u32,
    pub owner: OwnerToken,
    pub event: HostEvent,
}

/// Owned score data for native inspection. These DTOs deliberately contain
/// no arena references, JavaScript values, or borrowed player state.
#[derive(Clone, Debug, PartialEq)]
pub struct NativeScoreSnapshot {
    pub channel_count: u16,
    pub frame_count: u32,
    pub spans: Vec<NativeScoreSpan>,
    /// True when the bounded snapshot omitted score entries or nested values.
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeScoreSpan {
    pub channel_number: u32,
    pub start_frame: u32,
    pub end_frame: u32,
    pub member_ref: (u16, u16),
    pub member_name: String,
    pub behavior_references: Vec<NativeBehaviorReference>,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeBehaviorReference {
    pub cast_lib: u16,
    pub cast_member: u16,
    pub parameter_debug: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeChannelSnapshot {
    pub channel: i16,
    pub display_name: String,
    pub member_ref: Option<(i32, i32)>,
    pub script_instance_ids: Vec<u32>,
    pub width: i32,
    pub height: i32,
    pub loc_h: i32,
    pub loc_v: i32,
    pub visible: bool,
    pub puppet: bool,
    pub ink: i32,
    pub blend: i32,
    pub rotation: f64,
    pub skew: f64,
    pub flip_h: bool,
    pub flip_v: bool,
    pub color: String,
    pub bg_color: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NativeDatumValue {
    Null,
    Bool(bool),
    Int(i32),
    Float(f64),
    String(String),
    Symbol(String),
    Array(Vec<NativeDatumValue>),
    PropList(Vec<(String, NativeDatumValue)>, bool),
    Reference { kind: String, id: String },
    Opaque(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeMemberSnapshot {
    pub member_ref: (i32, i32),
    pub number: u32,
    pub name: String,
    pub type_name: String,
    pub fields: Vec<(String, NativeDatumValue)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeDatumSnapshot {
    pub datum_id: usize,
    pub type_name: String,
    pub debug_description: String,
    pub value: NativeDatumValue,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NativeScriptInstanceSnapshot {
    pub instance_id: Option<u32>,
    pub script_ref: (i32, i32),
    pub ancestor_id: Option<u32>,
    pub properties: Vec<(String, NativeDatumSnapshot)>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum NativePlayerNotificationKind {
    ScoreChanged(NativeScoreSnapshot),
    ChannelChanged(NativeChannelSnapshot),
    ChannelNameChanged { channel: i16, name: String },
    ChannelNamesChanged(Vec<(i16, String)>),
    CastMemberChanged(NativeMemberSnapshot),
    CastMemberListChanged {
        cast: u32,
        members: Vec<NativeMemberSnapshot>,
    },
    CastMemberNameChanged {
        slot: u32,
        names: Vec<(i16, String)>,
    },
    DatumSnapshot(NativeDatumSnapshot),
    ScriptInstanceSnapshot(NativeScriptInstanceSnapshot),
    Host(HostEvent),
}

#[derive(Clone, Debug)]
pub struct NativePlayerNotification {
    pub player_id: u32,
    pub owner: OwnerToken,
    pub kind: NativePlayerNotificationKind,
}

/// One bounded native conversion failure retained for inspection.  The
/// failing notification is consumed; the detached tail remains retryable.
#[derive(Clone, Debug)]
pub struct NativeNotificationError {
    pub player_id: u32,
    pub owner: OwnerToken,
    pub notification: String,
    pub message: String,
}

/// Bounded owner-tagged native sink. The queue is FIFO and never coalesces
/// notifications because native inspection must observe transitions exactly.
#[derive(Debug)]
pub struct NativePlayerNotificationMailbox {
    events: VecDeque<NativePlayerNotification>,
    capacity: usize,
}

impl Default for NativePlayerNotificationMailbox {
    fn default() -> Self {
        Self::new(MAX_HOST_EVENTS)
    }
}

impl NativePlayerNotificationMailbox {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "native notification capacity must be positive");
        Self {
            events: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, event: NativePlayerNotification) -> Result<(), HostEventOverflow> {
        if self.events.len() >= self.capacity {
            return Err(HostEventOverflow {
                capacity: self.capacity,
            });
        }
        self.events.push_back(event);
        Ok(())
    }

    pub fn drain(&mut self) -> Vec<NativePlayerNotification> {
        self.events.drain(..).collect()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }
}

/// Portable native sink used when there is no JavaScript callback.  Keeping
/// events here makes native execution observable without constructing wasm
/// values or silently dropping lifecycle transitions.
#[derive(Debug)]
pub struct NativeHostEventMailbox {
    events: VecDeque<NativeHostEvent>,
    capacity: usize,
}

/// A callback capability retained by the browser handle. RuntimeSession only
/// stores a weak reference, preventing a session→sink→handle cycle.
#[derive(Debug)]
pub struct BrowserHostSink {
    #[cfg(target_arch = "wasm32")]
    callback: js_sys::Function,
}

impl BrowserHostSink {
    #[cfg(target_arch = "wasm32")]
    pub fn new(callback: js_sys::Function) -> Self {
        Self { callback }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(_callback: js_sys::Function) -> Self {
        Self {}
    }

    #[cfg(target_arch = "wasm32")]
    pub fn dispatch(
        &self,
        event: js_sys::Object,
        owner_key: &str,
    ) -> Result<(), wasm_bindgen::JsValue> {
        self.callback
            .call2(
                &wasm_bindgen::JsValue::UNDEFINED,
                &event,
                &wasm_bindgen::JsValue::from_str(owner_key),
            )
            .map(|_| ())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn dispatch(&self, _event: (), _owner_key: &str) -> Result<(), String> {
        Ok(())
    }
}

pub type BrowserHostSinkRef = Rc<BrowserHostSink>;
pub type BrowserHostSinkWeak = Weak<BrowserHostSink>;

/// A lifecycle callback detached from the handle method that produced it.
/// The strong sink is retained only until this delivery is attempted.
#[derive(Clone)]
pub struct HostEventDelivery {
    pub player_id: u32,
    pub owner: OwnerToken,
    pub sink: BrowserHostSinkRef,
    pub event: HostEvent,
}

impl Default for NativeHostEventMailbox {
    fn default() -> Self {
        Self::new(MAX_HOST_EVENTS)
    }
}

impl NativeHostEventMailbox {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "native host event capacity must be positive");
        Self {
            events: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push(&mut self, event: NativeHostEvent) -> Result<(), HostEventOverflow> {
        if self.events.len() >= self.capacity {
            return Err(HostEventOverflow {
                capacity: self.capacity,
            });
        }
        self.events.push_back(event);
        Ok(())
    }

    pub fn drain(&mut self) -> Vec<NativeHostEvent> {
        self.events.drain(..).collect()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }
}

/// Bounded owner-local mailbox.  The producer remains in the runtime and can
/// report `HostEventOverflow` to its caller; no event is silently dropped.
#[derive(Debug)]
pub struct HostEventMailbox {
    events: VecDeque<HostEvent>,
    capacity: usize,
}

impl Default for HostEventMailbox {
    fn default() -> Self {
        Self::new(MAX_HOST_EVENTS)
    }
}

impl HostEventMailbox {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "host event capacity must be positive");
        Self {
            events: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn push(&mut self, event: HostEvent) -> Result<(), HostEventOverflow> {
        if event.is_state_snapshot() {
            if let Some(previous) = self.events.back_mut() {
                if event.same_snapshot_kind(previous) {
                    *previous = event;
                    return Ok(());
                }
            }
        }
        if self.events.len() >= self.capacity {
            return Err(HostEventOverflow {
                capacity: self.capacity,
            });
        }
        self.events.push_back(event);
        Ok(())
    }

    /// Restore a detached batch ahead of events produced reentrantly while
    /// its callback was running. The caller supplies the batch in delivery
    /// order; capacity is checked before mutating the queue.
    pub fn prepend(&mut self, events: Vec<HostEvent>) -> Result<(), HostEventOverflow> {
        if self.events.len().saturating_add(events.len()) > self.capacity {
            return Err(HostEventOverflow {
                capacity: self.capacity,
            });
        }
        for event in events.into_iter().rev() {
            self.events.push_front(event);
        }
        Ok(())
    }

    pub fn drain(&mut self) -> Vec<HostEvent> {
        self.events.drain(..).collect()
    }

    pub fn clear(&mut self) {
        self.events.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::ownership::OwnerKey;

    #[test]
    fn bounded_mailbox_coalesces_only_state_snapshots() {
        let mut mailbox = HostEventMailbox::new(2);
        mailbox.push(HostEvent::FrameChanged { frame: 1 }).unwrap();
        mailbox.push(HostEvent::FrameChanged { frame: 2 }).unwrap();
        mailbox
            .push(HostEvent::MovieLoadFailed {
                path: "a.dir".into(),
                error: "bad".into(),
            })
            .unwrap();
        assert_eq!(mailbox.len(), 2);
        assert_eq!(
            mailbox.push(HostEvent::ScriptErrorCleared),
            Err(HostEventOverflow { capacity: 2 })
        );
        assert_eq!(
            mailbox.drain(),
            vec![
                HostEvent::FrameChanged { frame: 2 },
                HostEvent::MovieLoadFailed {
                    path: "a.dir".into(),
                    error: "bad".into()
                },
            ]
        );
    }

    #[test]
    fn coalescing_does_not_cross_ordered_transitions_or_error_edges() {
        let mut mailbox = HostEventMailbox::new(8);
        mailbox.push(HostEvent::FrameChanged { frame: 1 }).unwrap();
        mailbox
            .push(HostEvent::MovieLoaded {
                version: 5,
                cast_names: vec!["main".into()],
            })
            .unwrap();
        mailbox.push(HostEvent::FrameChanged { frame: 2 }).unwrap();
        mailbox
            .push(HostEvent::ScriptError {
                message: "first".into(),
            })
            .unwrap();
        mailbox.push(HostEvent::ScriptErrorCleared).unwrap();
        mailbox
            .push(HostEvent::ScriptError {
                message: "second".into(),
            })
            .unwrap();
        assert_eq!(
            mailbox.drain(),
            vec![
                HostEvent::FrameChanged { frame: 1 },
                HostEvent::MovieLoaded {
                    version: 5,
                    cast_names: vec!["main".into()]
                },
                HostEvent::FrameChanged { frame: 2 },
                HostEvent::ScriptError {
                    message: "first".into()
                },
                HostEvent::ScriptErrorCleared,
                HostEvent::ScriptError {
                    message: "second".into()
                },
            ]
        );
    }

    #[test]
    fn native_mailbox_preserves_owner_identity_for_sibling_players() {
        let first = OwnerToken::new(OwnerKey {
            session: 1,
            player: 1,
            generation: 1,
        });
        let second = OwnerToken::new(OwnerKey {
            session: 1,
            player: 2,
            generation: 1,
        });
        let mut mailbox = NativeHostEventMailbox::default();
        mailbox
            .push(NativeHostEvent {
                player_id: 1,
                owner: first.clone(),
                event: HostEvent::FrameChanged { frame: 3 },
            })
            .unwrap();
        mailbox
            .push(NativeHostEvent {
                player_id: 2,
                owner: second.clone(),
                event: HostEvent::FrameChanged { frame: 8 },
            })
            .unwrap();
        let events = mailbox.drain();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].player_id, 1);
        assert!(events[0].owner.same_identity(&first));
        assert!(!events[0].owner.same_identity(&second));
        assert_eq!(events[1].player_id, 2);
        assert!(events[1].owner.same_identity(&second));
    }

    #[test]
    fn native_mailbox_overflow_can_retry_in_order_after_drain() {
        let owner = OwnerToken::transitional();
        let mut mailbox = NativeHostEventMailbox::new(1);
        let first = NativeHostEvent {
            player_id: 1,
            owner: owner.clone(),
            event: HostEvent::FrameChanged { frame: 1 },
        };
        let second = NativeHostEvent {
            player_id: 1,
            owner,
            event: HostEvent::MovieLoaded {
                version: 5,
                cast_names: vec!["main".into()],
            },
        };
        mailbox.push(first).unwrap();
        assert_eq!(
            mailbox.push(second.clone()),
            Err(HostEventOverflow { capacity: 1 })
        );
        assert_eq!(mailbox.drain().len(), 1);
        mailbox.push(second).unwrap();
        assert!(matches!(
            mailbox.drain().as_slice(),
            [NativeHostEvent {
                event: HostEvent::MovieLoaded { version: 5, .. },
                ..
            }]
        ));
    }

    #[test]
    fn detached_host_batch_is_prepended_ahead_of_reentrant_events() {
        let mut mailbox = HostEventMailbox::new(4);
        mailbox
            .push(HostEvent::FrameChanged { frame: 4 })
            .unwrap();
        mailbox
            .prepend(vec![
                HostEvent::MovieLoaded {
                    version: 1,
                    cast_names: vec![],
                },
                HostEvent::MovieLoadFailed {
                    path: "movie.dir".into(),
                    error: "failed".into(),
                },
            ])
            .unwrap();
        assert!(matches!(
            mailbox.drain().as_slice(),
            [
                HostEvent::MovieLoaded { version: 1, .. },
                HostEvent::MovieLoadFailed { .. },
                HostEvent::FrameChanged { frame: 4 },
            ]
        ));
    }
}
