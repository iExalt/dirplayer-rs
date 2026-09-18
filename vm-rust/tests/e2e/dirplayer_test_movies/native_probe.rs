use std::time::Duration;
use std::{any::Any, panic::AssertUnwindSafe};

use vm_rust::director::static_datum::StaticDatum;
use vm_rust::native_e2e_test;
use vm_rust::player::testing::StageSnapshot;
use vm_rust::player::testing::TestPlayer;
use vm_rust::player::testing_shared::TestHarness;

const PROBE_MOVIE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/native_director_probe.dcr"
);
const MISSING_PRIMARY: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/native_director_probe_missing_primary.dcr"
);

native_e2e_test!(test_native_director_probe_shape, |player| async move {
    async_std::future::timeout(Duration::from_secs(30), async {
        player.load_movie(PROBE_MOVIE).await;
        player.init_movie().await;

        let initial_frame = player.current_frame();
        for (handler, global) in [
            ("prepareMovie", "prepareState"),
            ("startMovie", "startState"),
            ("enterFrame", "enterState"),
            ("exitFrame", "exitState"),
        ] {
            let state = player
                .eval_datum(&format!("value(\"{global}\")"))
                .await
                .map_err(|error| format!("{global} evaluation failed: {}", error.message))?;
            assert_eq!(state, StaticDatum::Int(1), "{handler} handler did not run");
        }
        let snapshot = StageSnapshot::from_output(player.snapshot_stage());
        assert_eq!((snapshot.width, snapshot.height), (32, 32));
        assert_eq!(
            snapshot.data.len(),
            (snapshot.width * snapshot.height * 4) as usize
        );
        let pixel = |x: u32, y: u32| {
            let offset = ((y * snapshot.width + x) * 4) as usize;
            [
                snapshot.data[offset],
                snapshot.data[offset + 1],
                snapshot.data[offset + 2],
                snapshot.data[offset + 3],
            ]
        };
        assert_eq!(pixel(0, 0), [255, 255, 255, 255], "stage corner changed");
        assert_eq!(pixel(16, 16), [0, 0, 0, 255], "shape center missing");
        let black_pixels = snapshot
            .data
            .chunks_exact(4)
            .filter(|rgba| **rgba == [0, 0, 0, 255])
            .count();
        assert_eq!(black_pixels, 576, "inset shape pixel count changed");
        if let Ok(output_path) = std::env::var("NATIVE_PROBE_PNG") {
            std::fs::write(output_path, snapshot.to_png()).map_err(|error| error.to_string())?;
        }
        println!(
            "native probe state=initialized frame={} snapshot={}x{} black_pixels={}",
            initial_frame, snapshot.width, snapshot.height, black_pixels
        );

        println!(
            "native probe state=after-startup frame={} phases=prepare,start,enter,exit",
            initial_frame
        );
        Ok::<(), String>(())
    })
    .await
    .map_err(|_| "native probe exceeded 30 seconds".to_owned())??;

    Ok(())
});

fn panic_text(payload: Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else {
        "unknown panic payload".to_owned()
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_native_director_probe_step_frame_renderer_boundary() {
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        vm_rust::player::testing::run_test(async {
            let mut player = TestPlayer::new();
            player.load_movie(PROBE_MOVIE).await;
            player.init_movie().await;
            let _ = player.snapshot_stage();
            let _ = player.step_frame().await;
        });
    }));
    let message = result.expect_err("step_frame unexpectedly acquired a renderer");
    let message = panic_text(message);
    assert!(
        message.contains("owned player has no renderer"),
        "unexpected step_frame boundary: {message}"
    );
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_native_director_probe_missing_primary_boundary() {
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| {
        vm_rust::player::testing::run_test(async {
            let mut player = TestPlayer::new();
            player.load_movie(MISSING_PRIMARY).await;
        });
    }));
    let message = result.expect_err("missing primary unexpectedly loaded");
    let message = panic_text(message);
    assert!(
        message.contains("Failed to read"),
        "unexpected missing-file boundary: {message}"
    );
    assert!(
        message.contains("No such file") || message.contains("cannot find"),
        "missing-file error lacked OS boundary: {message}"
    );
}
