//! Bevy-owned lifecycle host for the native parity engine.
//!
//! The JSON-lines worker keeps protocol parsing and validation at its boundary.
//! This module receives typed engine operations and runs exactly one operation
//! per Bevy update.  The player remains a non-send resource because its
//! session, renderer, and command pumps are owner-local.

use bevy::prelude::*;

#[cfg(not(target_arch = "wasm32"))]
use std::{cell::RefCell, rc::Rc};

use crate::native_parity_worker::{
    CaptureKind, Response, StartConfig, StructuredError, VirtualInput, Worker,
};
#[cfg(not(target_arch = "wasm32"))]
use crate::player::session::NativeFlashCallbackObservation;

#[derive(Debug)]
pub(crate) enum HostOperation {
    Start(StartConfig),
    Inspect(String),
    Invoke {
        function: String,
        arguments: Vec<crate::player::testing::NativeInvokeArgument>,
    },
    Input(VirtualInput),
    Advance(u64),
    Capture(CaptureKind),
    Shutdown,
}

#[derive(Default)]
struct HostMailbox {
    pending: Option<HostOperation>,
    result: Option<Result<Response, StructuredError>>,
}

/// The native worker's single-threaded Bevy application.
pub(crate) struct NativeBevyHost {
    app: App,
}

impl NativeBevyHost {
    pub(crate) fn new() -> Self {
        let mut app = App::new();
        app.insert_non_send_resource(Worker::new());
        app.insert_non_send_resource(HostMailbox::default());
        app.add_systems(
            Update,
            (
                start_system,
                input_system,
                invoke_system,
                advance_system,
                inspect_system,
                capture_system,
                retire_system,
            )
                .chain(),
        );
        Self { app }
    }

    pub(crate) fn submit(&mut self, operation: HostOperation) -> Result<Response, StructuredError> {
        {
            let mut mailbox = self.app.world_mut().non_send_resource_mut::<HostMailbox>();
            debug_assert!(mailbox.pending.is_none());
            debug_assert!(mailbox.result.is_none());
            mailbox.pending = Some(operation);
        }
        self.app.update();
        self.app
            .world_mut()
            .non_send_resource_mut::<HostMailbox>()
            .result
            .take()
            .expect("native Bevy host operation did not produce a result")
    }

    pub(crate) fn is_started(&mut self) -> bool {
        self.app
            .world()
            .non_send_resource::<Worker>()
            .player
            .is_some()
    }

    pub(crate) fn validate_advance(&mut self, duration_us: u64) -> Result<(), StructuredError> {
        self.app
            .world()
            .non_send_resource::<Worker>()
            .validate_advance(duration_us)
    }

    pub(crate) fn is_shutdown(&mut self) -> bool {
        self.app.world().non_send_resource::<Worker>().shutdown
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn install_callback_observer(
        &mut self,
    ) -> Rc<RefCell<Vec<NativeFlashCallbackObservation>>> {
        let observer = Rc::new(RefCell::new(Vec::new()));
        self.app
            .world_mut()
            .non_send_resource_mut::<Worker>()
            .callback_observer = Some(observer.clone());
        observer
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn install_test_player(
        &mut self,
        player: crate::player::testing::TestPlayer,
    ) -> crate::player::testing::NativeTeardownWitness {
        let witness = player.native_teardown_witness();
        self.app
            .world_mut()
            .non_send_resource_mut::<Worker>()
            .player = Some(player);
        witness
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn take_start_failure_witness(
        &mut self,
    ) -> Option<crate::player::testing::NativeTeardownWitness> {
        self.app
            .world_mut()
            .non_send_resource_mut::<Worker>()
            .start_failure_witness
            .take()
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn test_live_teardown_witness(
        &self,
    ) -> Option<crate::player::testing::NativeTeardownWitness> {
        self.app
            .world()
            .non_send_resource::<Worker>()
            .player
            .as_ref()
            .map(crate::player::testing::TestPlayer::native_teardown_witness)
    }

    #[cfg(test)]
    pub(crate) fn test_elapsed_us(&mut self) -> u64 {
        self.app.world().non_send_resource::<Worker>().elapsed_us
    }

    #[cfg(test)]
    pub(crate) fn test_set_elapsed_us(&mut self, elapsed_us: u64) {
        self.app
            .world_mut()
            .non_send_resource_mut::<Worker>()
            .elapsed_us = elapsed_us;
    }

    #[cfg(test)]
    pub(crate) fn test_pointer_is_none(&mut self) -> bool {
        self.app
            .world()
            .non_send_resource::<Worker>()
            .pointer
            .is_none()
    }

    #[cfg(test)]
    pub(crate) fn test_has_pending_operation(&mut self) -> bool {
        self.app
            .world()
            .non_send_resource::<HostMailbox>()
            .pending
            .is_some()
    }
}

fn take_operation(
    mailbox: &mut HostMailbox,
    matches: impl FnOnce(&HostOperation) -> bool,
) -> Option<HostOperation> {
    if mailbox.pending.as_ref().is_some_and(matches) {
        mailbox.pending.take()
    } else {
        None
    }
}

fn start_system(mut mailbox: NonSendMut<HostMailbox>, mut worker: NonSendMut<Worker>) {
    let Some(HostOperation::Start(config)) = take_operation(&mut mailbox, |operation| {
        matches!(operation, HostOperation::Start(_))
    }) else {
        return;
    };
    mailbox.result = Some(worker.start(config));
}

fn input_system(mut mailbox: NonSendMut<HostMailbox>, mut worker: NonSendMut<Worker>) {
    let Some(HostOperation::Input(event)) = take_operation(&mut mailbox, |operation| {
        matches!(operation, HostOperation::Input(_))
    }) else {
        return;
    };
    mailbox.result = Some(worker.input(event));
}

fn invoke_system(mut mailbox: NonSendMut<HostMailbox>, worker: NonSendMut<Worker>) {
    let Some(HostOperation::Invoke {
        function,
        arguments,
    }) = take_operation(&mut mailbox, |operation| {
        matches!(operation, HostOperation::Invoke { .. })
    })
    else {
        return;
    };
    mailbox.result = Some(worker.invoke(function, arguments));
}

fn advance_system(mut mailbox: NonSendMut<HostMailbox>, mut worker: NonSendMut<Worker>) {
    let Some(HostOperation::Advance(duration_us)) = take_operation(&mut mailbox, |operation| {
        matches!(operation, HostOperation::Advance(_))
    }) else {
        return;
    };
    mailbox.result = Some(worker.advance(duration_us));
}

fn inspect_system(mut mailbox: NonSendMut<HostMailbox>, worker: NonSendMut<Worker>) {
    let Some(HostOperation::Inspect(path)) = take_operation(&mut mailbox, |operation| {
        matches!(operation, HostOperation::Inspect(_))
    }) else {
        return;
    };
    mailbox.result = Some(worker.inspect(&path));
}

fn capture_system(mut mailbox: NonSendMut<HostMailbox>, worker: NonSendMut<Worker>) {
    let Some(HostOperation::Capture(kind)) = take_operation(&mut mailbox, |operation| {
        matches!(operation, HostOperation::Capture(_))
    }) else {
        return;
    };
    mailbox.result = Some(match kind {
        CaptureKind::Rgba => worker.capture(),
        CaptureKind::Pcm => worker.pcm_capture(),
    });
}

fn retire_system(mut mailbox: NonSendMut<HostMailbox>, mut worker: NonSendMut<Worker>) {
    let Some(HostOperation::Shutdown) = take_operation(&mut mailbox, |operation| {
        matches!(operation, HostOperation::Shutdown)
    }) else {
        return;
    };
    mailbox.result = Some(worker.shutdown());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_update_does_not_create_a_result_or_operation() {
        let mut host = NativeBevyHost::new();
        host.app.update();
        let mailbox = host.app.world().non_send_resource::<HostMailbox>();
        assert!(mailbox.pending.is_none());
        assert!(mailbox.result.is_none());
        assert_eq!(host.app.world().non_send_resource::<Worker>().elapsed_us, 0);
    }

    #[test]
    fn invalid_start_is_routed_once_to_the_initialize_system() {
        let mut host = NativeBevyHost::new();
        let config = StartConfig {
            adapter: crate::native_parity_worker::AdapterConfig {
                backend: "invalid".to_owned(),
                resource_root: String::new(),
                movie: String::new(),
                source_dcr_sha256: String::new(),
                loading_policy: String::new(),
                resource_aliases: None,
                presentation_viewport: None,
            },
            seed: 0,
            clock: crate::native_parity_worker::ClockConfig {
                epoch_us: 0,
                frame_rate_num: 1,
                frame_rate_den: 1,
            },
            loading: crate::native_parity_worker::LoadingConfig {
                fixture: String::new(),
            },
        };
        let result = host.submit(HostOperation::Start(config));
        assert!(result.is_err());
        assert!(!host.is_started());
    }

    #[test]
    fn advance_budget_is_rejected_before_submission() {
        let mut host = NativeBevyHost::new();
        assert!(host.validate_advance(60_000_001).is_err());
        assert!(
            host.app
                .world()
                .non_send_resource::<HostMailbox>()
                .pending
                .is_none()
        );
    }

    #[test]
    fn shutdown_is_acknowledged_by_the_retire_system() {
        let mut host = NativeBevyHost::new();
        assert!(matches!(
            host.submit(HostOperation::Shutdown),
            Ok(Response::Acknowledged)
        ));
        assert!(host.is_shutdown());
    }
}
