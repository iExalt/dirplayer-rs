use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/native_director_probe.dcr"
);

fn fixture_hash() -> String {
    let bytes = std::fs::read(FIXTURE).expect("native probe fixture must be readable");
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn start_worker(writable_root: &Path) -> (Child, ChildStdin, BufReader<ChildStdout>) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_native_dirplayer_parity_worker"))
        .env("PARITY_SESSION_ROOT", &writable_root)
        .env("TMPDIR", &writable_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("native parity worker must spawn");
    let stdin = child.stdin.take().expect("worker stdin");
    let stdout = BufReader::new(child.stdout.take().expect("worker stdout"));
    let mut worker = (child, stdin, stdout);
    let discover = send_request(&mut worker.1, &mut worker.2, 1, json!({"op": "discover"}));
    assert_eq!(discover["version"], 1);
    assert_eq!(discover["request_id"], 1);
    assert_eq!(discover["kind"], "capabilities");
    (worker.0, worker.1, worker.2)
}

fn send_request(
    stdin: &mut ChildStdin,
    stdout: &mut BufReader<ChildStdout>,
    request_id: u64,
    request: Value,
) -> Value {
    let mut envelope = request;
    envelope["version"] = json!(1);
    envelope["request_id"] = json!(request_id);
    writeln!(stdin, "{}", envelope).expect("write worker request");
    stdin.flush().expect("flush worker request");
    let mut line = String::new();
    stdout.read_line(&mut line).expect("read worker response");
    assert!(
        !line.is_empty(),
        "worker ended before response {request_id}"
    );
    serde_json::from_str(&line).expect("worker response must be JSON")
}

#[test]
fn native_worker_wire_preserves_global_runtime_and_capture_contracts() {
    // Cargo does not set CARGO_TARGET_TMPDIR for every integration-test
    // runner. Prefer it when available, then stay under the configured target
    // directory so clean-clone runs never use ambient temporary storage.
    let target_tmp = std::env::var_os("CARGO_TARGET_TMPDIR")
        .or_else(|| std::env::var_os("CARGO_TARGET_DIR"))
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target"));
    let writable_root = target_tmp.join(format!("native-parity-worker-{}", std::process::id()));
    std::fs::create_dir_all(&writable_root).expect("create worker writable root");
    let (mut child, mut stdin, mut stdout) = start_worker(&writable_root);
    let fixture_root = Path::new(FIXTURE).parent().expect("fixture parent");
    let started = send_request(
        &mut stdin,
        &mut stdout,
        2,
        json!({
            "op": "start",
            "args": {
                "config": {
                    "adapter": {
                        "backend": "dirplayer-native",
                        "resource_root": fixture_root,
                        "movie": "native_director_probe.dcr",
                        "source_dcr_sha256": fixture_hash(),
                        "loading_policy": "single-dcr"
                    },
                    "seed": 0,
                    "clock": {"epoch_us": 0, "frame_rate_num": 30, "frame_rate_den": 1},
                    "loading": {"fixture": "native-director-probe"}
                }
            }
        }),
    );
    assert_eq!(started["request_id"], 2);
    assert_eq!(started["kind"], "started");
    assert_eq!(
        started["value"]["session"],
        json!({"id": 1, "generation": 1})
    );

    let current_frame = send_request(
        &mut stdin,
        &mut stdout,
        3,
        json!({"op": "inspect", "args": {"target": "root", "path": "current_frame"}}),
    );
    assert_eq!(current_frame["request_id"], 3);
    assert_eq!(current_frame["value"], 1);

    let simulation_time = send_request(
        &mut stdin,
        &mut stdout,
        4,
        json!({"op": "inspect", "args": {"target": "root", "path": "simulation_time_us"}}),
    );
    assert_eq!(simulation_time["value"], 0);

    let global = send_request(
        &mut stdin,
        &mut stdout,
        5,
        json!({"op": "inspect", "args": {"target": "root", "path": "globals.frameState"}}),
    );
    assert_eq!(global["request_id"], 5);
    assert_eq!(global["value"], 2);

    let malformed = send_request(
        &mut stdin,
        &mut stdout,
        6,
        json!({"op": "inspect", "args": {"target": "root", "path": "globals.bad-name"}}),
    );
    assert_eq!(malformed["request_id"], 6);
    assert_eq!(malformed["kind"], "error");
    assert_eq!(malformed["value"]["code"], "invalid_request");

    let capture = send_request(
        &mut stdin,
        &mut stdout,
        7,
        json!({"op": "capture", "args": {"kind": "rgba"}}),
    );
    assert_eq!(capture["request_id"], 7);
    assert_eq!(capture["kind"], "captured");
    assert_eq!(capture["value"]["kind"], "rgba");
    assert_eq!(
        capture["value"]["value"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>(),
        ["height", "pixels", "sha256", "timestamp_us", "width"]
            .into_iter()
            .map(str::to_owned)
            .collect(),
    );
    assert_eq!(capture["value"]["value"]["timestamp_us"], 0);
    assert_eq!(capture["value"]["value"]["width"], 32);
    assert_eq!(capture["value"]["value"]["height"], 32);

    let shutdown = send_request(&mut stdin, &mut stdout, 8, json!({"op": "shutdown"}));
    assert_eq!(shutdown["request_id"], 8);
    assert_eq!(shutdown["kind"], "acknowledged");
    drop(stdin);
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("worker stderr")
        .read_to_string(&mut stderr)
        .expect("read worker stderr");
    let status = child.wait().expect("wait for worker");
    assert!(status.success(), "worker failed: {status:?}");
    assert!(stderr.is_empty(), "worker wrote to stderr: {stderr}");
    std::fs::remove_dir_all(writable_root).expect("clean worker writable root");
}
