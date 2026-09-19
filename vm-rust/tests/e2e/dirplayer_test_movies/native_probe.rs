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
        player.init_movie_at(0).await;

        let initial_frame = player.current_frame();
        assert_eq!(initial_frame, 1);
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
        let initial_frame_state = player
            .eval_datum("value(\"frameState\")")
            .await
            .map_err(|error| format!("frameState evaluation failed: {}", error.message))?;
        assert_eq!(initial_frame_state, StaticDatum::Int(2));
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

        player
            .eval_datum("timeout().new(\"timerA\", 100, #timerA)")
            .await
            .map_err(|error| format!("timerA schedule failed: {}", error.message))?;
        player
            .eval_datum("timeout().new(\"timerB\", 100, #timerB)")
            .await
            .map_err(|error| format!("timerB schedule failed: {}", error.message))?;
        assert!(
            player.take_native_timeout_host_unsupported().is_empty(),
            "native timeout scheduling must not publish unsupported host actions"
        );

        let first_advance = player.advance_to(34).await;
        assert_eq!(first_advance.frames, 1);
        assert_eq!(player.current_frame(), 2);
        assert_eq!(
            player
                .eval_datum("value(\"frameState\")")
                .await
                .map_err(|error| format!("frameState evaluation failed: {}", error.message))?,
            StaticDatum::Int(3)
        );

        let second_advance = player.advance_to(67).await;
        assert_eq!(second_advance.frames, 1);
        assert_eq!(player.current_frame(), 3);
        assert_eq!(
            player
                .eval_datum("value(\"frameState\")")
                .await
                .map_err(|error| format!("frameState evaluation failed: {}", error.message))?,
            StaticDatum::Int(4)
        );

        let equal_deadline = player.advance_to(100).await;
        assert_eq!(equal_deadline.frames, 1);
        assert_eq!(equal_deadline.timeouts, 2);
        assert_eq!(player.current_frame(), 1);
        assert_eq!(
            player
                .eval_datum("value(\"frameState\")")
                .await
                .map_err(|error| format!("frameState evaluation failed: {}", error.message))?,
            StaticDatum::Int(141),
            "equal deadline timers must fire before the frame handler"
        );
        assert!(
            player.take_native_timeout_host_unsupported().is_empty(),
            "native timeout callbacks must not emit unsupported host actions"
        );

        let recurring = player.advance_to(300).await;
        assert_eq!(recurring.frames, 6);
        assert_eq!(recurring.timeouts, 1);
        assert_eq!(
            player
                .eval_datum("value(\"frameState\")")
                .await
                .map_err(|error| format!("frameState evaluation failed: {}", error.message))?,
            StaticDatum::Int(157),
            "recurring exact deadline and callback period mutation changed state"
        );
        assert!(
            player.take_native_timeout_host_unsupported().is_empty(),
            "native timeout cancellation/rescheduling must stay local"
        );

        let subframe = player.try_advance_to(301).await.unwrap();
        assert_eq!(subframe, Default::default(), "subframe advancement must not execute work");
        assert_eq!(player.try_advance_to(301).await.unwrap(), Default::default());
        let reversal = player.try_advance_to(300).await.expect_err("reversal unexpectedly succeeded");
        assert!(reversal.message.contains("reversed"));
        let range = player
            .try_advance_to(9_007_199_254_740_992)
            .await
            .expect_err("out-of-range time unexpectedly succeeded");
        assert!(range.message.contains("exact supported range"));

        println!("native probe state=after-advances frames=1,2,3 frame_state=2,3,4 timers=2+1");
        Ok::<(), String>(())
    })
    .await
    .map_err(|_| "native probe exceeded 30 seconds".to_owned())??;

    Ok(())
});

fn expected_probe_rgba(left: u32) -> Vec<u8> {
    let mut data = Vec::with_capacity(32 * 32 * 4);
    for y in 0..32 {
        for x in 0..32 {
            if (left..left + 24).contains(&x) && (4..28).contains(&y) {
                data.extend_from_slice(&[0, 0, 0, 255]);
            } else {
                data.extend_from_slice(&[255, 255, 255, 255]);
            }
        }
    }
    data
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn test_native_director_probe_input() {
    vm_rust::player::testing::run_test(async {
        let mut player = TestPlayer::new();
        player.load_movie(PROBE_MOVIE).await;
        player.init_movie_at(0).await;

        let initial_report = player.try_advance_to(0).await.unwrap();
        let initial_frame = player.current_frame();
        let initial_state = player.eval_datum("value(\"frameState\")").await.unwrap();
        let before = StageSnapshot::from_output(player.snapshot_stage());
        assert_eq!((before.width, before.height), (32, 32));
        assert_eq!(before.data, expected_probe_rgba(4));

        player.native_mouse_down(8, 8).await;

        assert_eq!(player.current_frame(), initial_frame);
        assert_eq!(
            player.eval_datum("value(\"inputH\")").await.unwrap(),
            StaticDatum::Int(8)
        );
        assert_eq!(
            player.eval_datum("value(\"inputV\")").await.unwrap(),
            StaticDatum::Int(8)
        );
        assert_eq!(
            player.eval_datum("value(\"frameState\")").await.unwrap(),
            initial_state
        );
        let after = StageSnapshot::from_output(player.snapshot_stage());
        assert_eq!((after.width, after.height), (32, 32));
        assert_eq!(after.data, expected_probe_rgba(8));

        let repeated_frame = player.current_frame();
        let repeated_state = player.eval_datum("value(\"frameState\")").await.unwrap();
        let repeated_input_h = player.eval_datum("value(\"inputH\")").await.unwrap();
        let repeated_input_v = player.eval_datum("value(\"inputV\")").await.unwrap();
        let repeated = StageSnapshot::from_output(player.snapshot_stage());
        assert_eq!(repeated.data, after.data);
        assert_eq!(player.current_frame(), repeated_frame);
        assert_eq!(
            player.eval_datum("value(\"frameState\")").await.unwrap(),
            repeated_state
        );
        assert_eq!(player.eval_datum("value(\"inputH\")").await.unwrap(), repeated_input_h);
        assert_eq!(player.eval_datum("value(\"inputV\")").await.unwrap(), repeated_input_v);
        assert_eq!(player.try_advance_to(0).await.unwrap(), initial_report);

        let advance = player.advance_to(34).await;
        assert_eq!(advance.frames, 1);
        assert_eq!(advance.timeouts, 0);
        assert_eq!(player.current_frame(), 2);
    });
}

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
fn test_native_director_probe_split_and_combined_advances_match() {
    vm_rust::player::testing::run_test(async {
        let split = {
            let mut player = TestPlayer::new();
            player.load_movie(PROBE_MOVIE).await;
            player.init_movie_at(0).await;
            player
                .eval_datum("timeout().new(\"timerA\", 100, #timerA)")
                .await
                .unwrap();
            player
                .eval_datum("timeout().new(\"timerB\", 100, #timerB)")
                .await
                .unwrap();
            let first = player.advance_to(34).await;
            let second = player.advance_to(67).await;
            let third = player.advance_to(100).await;
            let state = player.eval_datum("value(\"frameState\")").await.unwrap();
            let snapshot = StageSnapshot::from_output(player.snapshot_stage());
            (first.frames + second.frames + third.frames, third.timeouts, state, snapshot)
        };
        let combined = {
            let mut player = TestPlayer::new();
            player.load_movie(PROBE_MOVIE).await;
            player.init_movie_at(0).await;
            player
                .eval_datum("timeout().new(\"timerA\", 100, #timerA)")
                .await
                .unwrap();
            player
                .eval_datum("timeout().new(\"timerB\", 100, #timerB)")
                .await
                .unwrap();
            let report = player.advance_to(100).await;
            let state = player.eval_datum("value(\"frameState\")").await.unwrap();
            let snapshot = StageSnapshot::from_output(player.snapshot_stage());
            (report.frames, report.timeouts, state, snapshot)
        };
        assert_eq!(split.0, combined.0);
        assert_eq!(split.1, combined.1);
        assert_eq!(split.2, combined.2);
        assert_eq!(split.3.width, combined.3.width);
        assert_eq!(split.3.height, combined.3.height);
        assert_eq!(split.3.data, combined.3.data);
    });
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
