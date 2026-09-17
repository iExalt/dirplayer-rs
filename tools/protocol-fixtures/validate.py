#!/usr/bin/env python3
"""Validate v1 protocol fixtures and exercise a worker transport boundary.

This intentionally uses only the Python standard library.  In particular,
Python's JSON decoder preserves integer tokens above JavaScript's safe range;
the validators below still distinguish an integer token such as 1 from a
floating-point token such as 1.0 where the wire type is unsigned integer.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import selectors
import signal
import struct
import subprocess
import sys
import threading
import time
import tempfile
from pathlib import Path
from typing import Any, Iterable


MAX_LINE_BYTES = 8 * 1024 * 1024
U16_MAX = (1 << 16) - 1
U32_MAX = (1 << 32) - 1
U64_MAX = (1 << 64) - 1
CAPTURE_KINDS = {"rgba", "pcm"}
ERROR_CODES = {
    "unsupported",
    "invalid_request",
    "invalid_handle",
    "runtime",
    "not_ready",
    "cancelled",
    "timeout",
    "protocol",
    "internal",
}
CAPABILITIES = {
    "state_inspection",
    "state_mutation",
    "invocation",
    "virtual_input",
    "controlled_time",
    "rgba_capture",
    "pcm_capture",
}
_active_stderr_drainers = 0
_active_stderr_drainers_lock = threading.Lock()


class FixtureError(ValueError):
    pass


class TransportError(RuntimeError):
    pass


def fail(where: str, message: str) -> FixtureError:
    return FixtureError(f"{where}: {message}")


def parse_json(raw: bytes | str, where: str) -> Any:
    def reject_constant(token: str) -> Any:
        raise fail(where, f"non-finite JSON constant {token} is not allowed")

    try:
        value = json.loads(raw, parse_constant=reject_constant)
    except FixtureError:
        raise
    except (TypeError, ValueError, json.JSONDecodeError) as error:
        raise fail(where, f"invalid JSON: {error}") from error
    reject_nonfinite(value, where)
    return value


def reject_nonfinite(value: Any, where: str) -> None:
    if type(value) is float and not math.isfinite(value):
        raise fail(where, "JSON number overflows to a non-finite value")
    if type(value) is list:
        for index, child in enumerate(value):
            reject_nonfinite(child, f"{where}[{index}]")
    elif type(value) is dict:
        for key, child in value.items():
            reject_nonfinite(child, f"{where}.{key}")


def read_jsonl(path: Path) -> list[tuple[int, bytes, Any]]:
    records: list[tuple[int, bytes, Any]] = []
    with path.open("rb") as stream:
        for line_number, raw in enumerate(stream, 1):
            if len(raw) > MAX_LINE_BYTES:
                raise fail(
                    f"{path}:{line_number}",
                    f"line is {len(raw)} bytes; maximum is {MAX_LINE_BYTES} including newline",
                )
            if not raw.endswith(b"\n"):
                raise fail(f"{path}:{line_number}", "JSONL line has no trailing newline")
            payload = raw[:-1]
            if not payload.strip():
                raise fail(f"{path}:{line_number}", "blank lines are not fixtures")
            try:
                payload.decode("utf-8")
            except UnicodeDecodeError as error:
                raise fail(f"{path}:{line_number}", f"line is not UTF-8: {error}") from error
            records.append((line_number, raw, parse_json(payload, f"{path}:{line_number}")))
    return records


def object_value(value: Any, where: str) -> dict[str, Any]:
    if type(value) is not dict:
        raise fail(where, "expected a JSON object")
    return value


def integer_token(value: Any, where: str, minimum: int, maximum: int) -> int:
    # bool is a subclass of int in Python, but is not a JSON integer token for
    # an integer-valued serde field.
    if type(value) is not int:
        raise fail(where, "expected an integer JSON token (1.0 is not an integer token)")
    if not minimum <= value <= maximum:
        raise fail(where, f"integer {value} is outside [{minimum}, {maximum}]")
    return value


def f32_value(value: Any, where: str) -> float:
    if type(value) not in (int, float):
        raise fail(where, "expected a numeric JSON token")
    try:
        packed = struct.pack("<f", float(value))
        converted = struct.unpack("<f", packed)[0]
    except OverflowError:
        # serde_json can decode a finite JSON number such as 1e40 into an
        # overflowing f32.  The protocol's VirtualInput fields do not add a
        # finite-value check; capture PCM validation does that separately.
        converted = math.inf if value >= 0 else -math.inf
    except (struct.error, ValueError) as error:
        raise fail(where, f"value cannot be represented as finite f32: {error}") from error
    return converted


def finite_f32(value: Any, where: str) -> float:
    converted = f32_value(value, where)
    if not math.isfinite(converted):
        raise fail(where, "value is not finite after f32 conversion")
    return converted


def require_keys(
    value: dict[str, Any],
    required: set[str],
    allowed: set[str],
    where: str,
    *,
    reject_extra: bool = False,
) -> None:
    missing = required - value.keys()
    extra = value.keys() - allowed
    if missing:
        raise fail(where, f"missing fields: {', '.join(sorted(missing))}")
    if extra and reject_extra:
        raise fail(where, f"unknown fields: {', '.join(sorted(extra))}")


def validate_handle(value: Any, where: str) -> None:
    handle = object_value(value, where)
    require_keys(handle, {"id", "generation"}, {"id", "generation"}, where)
    integer_token(handle["id"], f"{where}.id", 0, U64_MAX)
    integer_token(handle["generation"], f"{where}.generation", 0, U64_MAX)


def validate_object_handle(value: Any, where: str) -> None:
    handle = object_value(value, where)
    require_keys(
        handle,
        {"session_id", "generation", "object_id"},
        {"session_id", "generation", "object_id"},
        where,
    )
    for field in ("session_id", "generation", "object_id"):
        integer_token(handle[field], f"{where}.{field}", 0, U64_MAX)


def validate_target(value: Any, where: str) -> None:
    if value == "root":
        return
    target = object_value(value, where)
    if set(target) == {"root"}:
        if target["root"] is not None:
            raise fail(f"{where}.root", "unit root target must contain null")
        return
    require_keys(target, {"object"}, {"object"}, where, reject_extra=True)
    validate_object_handle(target["object"], f"{where}.object")


def validate_input_event(value: Any, where: str) -> None:
    event = object_value(value, where)
    kind = event.get("kind")
    if not isinstance(kind, str):
        raise fail(where, "event.kind must be a string")
    if kind == "pointer":
        require_keys(event, {"kind", "value"}, {"kind", "value"}, where)
        pointer = object_value(event["value"], f"{where}.value")
        require_keys(pointer, {"space", "x", "y"}, {"space", "x", "y"}, f"{where}.value")
        if not isinstance(pointer["space"], str) or pointer["space"] not in {"stage", "window"}:
            raise fail(f"{where}.space", "unknown pointer space")
        # VirtualInput's serde f32 fields have no finite-value validator;
        # serde can decode an in-range JSON number such as 1e40 to +inf.
        f32_value(pointer["x"], f"{where}.value.x")
        f32_value(pointer["y"], f"{where}.value.y")
    elif kind == "button":
        require_keys(event, {"kind", "value"}, {"kind", "value"}, where)
        button = object_value(event["value"], f"{where}.value")
        require_keys(button, {"button", "state"}, {"button", "state"}, f"{where}.value")
        if not isinstance(button["button"], str) or button["button"] not in {"left", "right", "middle"}:
            raise fail(f"{where}.value.button", "unknown mouse button")
        if not isinstance(button["state"], str) or button["state"] not in {"down", "up"}:
            raise fail(f"{where}.value.state", "unknown button state")
    elif kind == "key":
        require_keys(event, {"kind", "value"}, {"kind", "value"}, where)
        key = object_value(event["value"], f"{where}.value")
        require_keys(key, {"key", "state"}, {"key", "state"}, f"{where}.value")
        if not isinstance(key["key"], str):
            raise fail(f"{where}.value.key", "key must be a string")
        if not isinstance(key["state"], str) or key["state"] not in {"down", "up"}:
            raise fail(f"{where}.value.state", "unknown key state")
    elif kind == "focus":
        require_keys(event, {"kind", "value"}, {"kind", "value"}, where)
        focused = object_value(event["value"], f"{where}.value")
        require_keys(focused, {"focused"}, {"focused"}, f"{where}.value")
        if type(focused["focused"]) is not bool:
            raise fail(f"{where}.value.focused", "focused must be boolean")
    elif kind == "leave":
        require_keys(event, {"kind"}, {"kind"}, where)
    elif kind == "resize":
        require_keys(event, {"kind", "value"}, {"kind", "value"}, where)
        resize = object_value(event["value"], f"{where}.value")
        require_keys(
            resize,
            {"width_physical", "height_physical", "scale"},
            {"width_physical", "height_physical", "scale"},
            f"{where}.value",
        )
        integer_token(resize["width_physical"], f"{where}.value.width_physical", 0, U32_MAX)
        integer_token(resize["height_physical"], f"{where}.value.height_physical", 0, U32_MAX)
        f32_value(resize["scale"], f"{where}.value.scale")
    else:
        raise fail(f"{where}.kind", f"unknown virtual input kind {kind!r}")


def validate_request(value: Any, where: str, require_v1: bool = True) -> None:
    request = object_value(value, where)
    require_keys(
        request,
        {"version", "request_id", "op"},
        {"version", "request_id", "op", "args"},
        where,
        reject_extra=True,
    )
    integer_token(request["version"], f"{where}.version", 0, U16_MAX)
    if require_v1 and request["version"] != 1:
        raise fail(f"{where}.version", "expected protocol version 1")
    integer_token(request["request_id"], f"{where}.request_id", 0, U64_MAX)
    operation = request["op"]
    if not isinstance(operation, str):
        raise fail(f"{where}.op", "operation must be a string")
    args = request.get("args")
    unit_operations = {"discover", "reset", "shutdown"}
    if operation in unit_operations:
        if "args" in request and request["args"] is not None:
            raise fail(where, f"unit operation {operation} may only contain null args")
        return
    if operation not in {
        "start",
        "inspect",
        "modify",
        "invoke",
        "input",
        "advance",
        "capture",
    }:
        raise fail(f"{where}.op", f"unknown operation {operation!r}")
    args_object = object_value(args, f"{where}.args")
    if operation == "start":
        require_keys(args_object, {"config"}, {"config"}, f"{where}.args")
    elif operation == "inspect":
        require_keys(args_object, {"target", "path"}, {"target", "path"}, f"{where}.args")
        validate_target(args_object["target"], f"{where}.args.target")
        if not isinstance(args_object["path"], str):
            raise fail(f"{where}.args.path", "path must be a string")
    elif operation == "modify":
        require_keys(args_object, {"target", "path", "value"}, {"target", "path", "value"}, f"{where}.args")
        validate_target(args_object["target"], f"{where}.args.target")
        if not isinstance(args_object["path"], str):
            raise fail(f"{where}.args.path", "path must be a string")
    elif operation == "invoke":
        require_keys(
            args_object,
            {"target", "function", "arguments"},
            {"target", "function", "arguments"},
            f"{where}.args",
        )
        validate_target(args_object["target"], f"{where}.args.target")
        if not isinstance(args_object["function"], str):
            raise fail(f"{where}.args.function", "function must be a string")
        if type(args_object["arguments"]) is not list:
            raise fail(f"{where}.args.arguments", "arguments must be an array")
    elif operation == "input":
        require_keys(args_object, {"event"}, {"event"}, f"{where}.args")
        validate_input_event(args_object["event"], f"{where}.args.event")
    elif operation == "advance":
        require_keys(args_object, {"duration_us"}, {"duration_us"}, f"{where}.args")
        integer_token(args_object["duration_us"], f"{where}.args.duration_us", 0, U64_MAX)
    elif operation == "capture":
        require_keys(args_object, {"kind"}, {"kind"}, f"{where}.args")
        if not isinstance(args_object["kind"], str) or args_object["kind"] not in CAPTURE_KINDS:
            raise fail(f"{where}.args.kind", "unknown capture kind")


def validate_rgba(value: Any, where: str) -> None:
    frame = object_value(value, where)
    require_keys(
        frame,
        {"timestamp_us", "width", "height", "pixels"},
        {"timestamp_us", "width", "height", "pixels", "sha256"},
        where,
        reject_extra=True,
    )
    integer_token(frame["timestamp_us"], f"{where}.timestamp_us", 0, U64_MAX)
    width = integer_token(frame["width"], f"{where}.width", 1, U32_MAX)
    height = integer_token(frame["height"], f"{where}.height", 1, U32_MAX)
    if type(frame["pixels"]) is not list:
        raise fail(f"{where}.pixels", "pixels must be an array")
    expected = width * height * 4
    if len(frame["pixels"]) != expected:
        raise fail(f"{where}.pixels", f"expected {expected} RGBA bytes, got {len(frame['pixels'])}")
    for index, pixel in enumerate(frame["pixels"]):
        integer_token(pixel, f"{where}.pixels[{index}]", 0, 255)
    if "sha256" in frame and frame["sha256"] is not None and not isinstance(frame["sha256"], str):
        raise fail(f"{where}.sha256", "sha256 must be a string or null when present")


def validate_pcm(value: Any, where: str) -> None:
    capture = object_value(value, where)
    require_keys(
        capture,
        {"timestamp_us", "sample_rate", "channels", "samples"},
        {"timestamp_us", "sample_rate", "channels", "samples", "sha256"},
        where,
        reject_extra=True,
    )
    integer_token(capture["timestamp_us"], f"{where}.timestamp_us", 0, U64_MAX)
    if integer_token(capture["sample_rate"], f"{where}.sample_rate", 0, U32_MAX) != 48000:
        raise fail(f"{where}.sample_rate", "PCM sample rate must be 48000")
    if integer_token(capture["channels"], f"{where}.channels", 0, U16_MAX) != 2:
        raise fail(f"{where}.channels", "PCM capture must have exactly two channels")
    if type(capture["samples"]) is not list:
        raise fail(f"{where}.samples", "samples must be an array")
    for index, sample in enumerate(capture["samples"]):
        if type(sample) is not list or len(sample) != 2:
            raise fail(f"{where}.samples[{index}]", "each sample must contain two channels")
        finite_f32(sample[0], f"{where}.samples[{index}][0]")
        finite_f32(sample[1], f"{where}.samples[{index}][1]")
    if "sha256" in capture and capture["sha256"] is not None and not isinstance(capture["sha256"], str):
        raise fail(f"{where}.sha256", "sha256 must be a string or null when present")


def validate_session_response(value: Any, where: str) -> None:
    response = object_value(value, where)
    require_keys(response, {"session"}, {"session"}, where)
    validate_handle(response["session"], f"{where}.session")


def validate_response(value: Any, where: str, require_v1: bool = True) -> None:
    response = object_value(value, where)
    require_keys(
        response,
        {"version", "request_id", "kind"},
        {"version", "request_id", "kind", "value"},
        where,
        reject_extra=True,
    )
    integer_token(response["version"], f"{where}.version", 0, U16_MAX)
    if require_v1 and response["version"] != 1:
        raise fail(f"{where}.version", "expected protocol version 1")
    integer_token(response["request_id"], f"{where}.request_id", 0, U64_MAX)
    kind = response["kind"]
    if not isinstance(kind, str):
        raise fail(f"{where}.kind", "result kind must be a string")
    if kind == "capabilities":
        capabilities = object_value(response.get("value"), f"{where}.value")
        require_keys(
            capabilities,
            {"runtime", "runtime_version", "operations"},
            {"runtime", "runtime_version", "operations"},
            f"{where}.value",
        )
        if not isinstance(capabilities["runtime"], str) or not isinstance(capabilities["runtime_version"], str):
            raise fail(f"{where}.value", "runtime fields must be strings")
        if type(capabilities["operations"]) is not list:
            raise fail(f"{where}.value.operations", "operations must be an array")
        for index, operation in enumerate(capabilities["operations"]):
            if not isinstance(operation, str) or operation not in CAPABILITIES:
                raise fail(f"{where}.value.operations[{index}]", "unknown capability")
    elif kind in {"started", "reset"}:
        validate_session_response(response.get("value"), f"{where}.value")
    elif kind in {"observation", "invoked"}:
        if "value" not in response:
            raise fail(where, f"{kind} response requires value")
    elif kind == "captured":
        capture = object_value(response.get("value"), f"{where}.value")
        require_keys(capture, {"kind", "value"}, {"kind", "value"}, f"{where}.value")
        if not isinstance(capture["kind"], str):
            raise fail(f"{where}.value.kind", "capture kind must be a string")
        if capture["kind"] == "rgba":
            validate_rgba(capture["value"], f"{where}.value.value")
        elif capture["kind"] == "pcm":
            validate_pcm(capture["value"], f"{where}.value.value")
        else:
            raise fail(f"{where}.value.kind", "unknown capture kind")
    elif kind == "acknowledged":
        if "value" in response and response["value"] is not None:
            raise fail(f"{where}.value", "acknowledged content must be null when present")
    elif kind == "error":
        error = object_value(response.get("value"), f"{where}.value")
        require_keys(error, {"code", "message"}, {"code", "message", "details"}, f"{where}.value")
        if not isinstance(error["code"], str) or error["code"] not in ERROR_CODES:
            raise fail(f"{where}.value.code", "unknown error code")
        if not isinstance(error["message"], str):
            raise fail(f"{where}.value.message", "error message must be a string")
    else:
        raise fail(f"{where}.kind", f"unknown response kind {kind!r}")


def validate_compatibility(path: Path) -> int:
    allowed_statuses = {"accept", "reject", "n_a", "emits", "omits"}
    records = read_jsonl(path)
    for line_number, _raw, value in records:
        where = f"{path}:{line_number}"
        record = object_value(value, where)
        require_keys(
            record,
            {"case", "direction", "wire", "rust_decoder", "v1_semantic", "browser_worker", "reason"},
            {"case", "direction", "wire", "rust_decoder", "v1_semantic", "browser_worker", "reason"},
            where,
            reject_extra=True,
        )
        for field in ("case", "wire", "rust_decoder", "v1_semantic", "browser_worker", "reason"):
            if not isinstance(record[field], str):
                raise fail(f"{where}.{field}", "must be a string")
        if record["direction"] not in {"request", "response"}:
            raise fail(f"{where}.direction", "must be request or response")
        if record["rust_decoder"] not in allowed_statuses:
            raise fail(f"{where}.rust_decoder", "unknown status")
        if record["v1_semantic"] not in allowed_statuses:
            raise fail(f"{where}.v1_semantic", "unknown status")
        if record["browser_worker"] not in allowed_statuses:
            raise fail(f"{where}.browser_worker", "unknown status")
        wire = parse_json(record["wire"], f"{where}.wire")
        if record["direction"] == "request":
            structural = validate_request
        else:
            structural = validate_response
        try:
            structural(wire, f"{where}.wire", require_v1=False)
            structural_status = "accept"
        except FixtureError:
            structural_status = "reject"
        if structural_status != record["rust_decoder"]:
            raise fail(
                where,
                f"rust_decoder says {record['rust_decoder']}, structural reference model says {structural_status}",
            )
        try:
            structural(wire, f"{where}.wire", require_v1=True)
            semantic_status = "accept"
        except FixtureError:
            semantic_status = "reject"
        if semantic_status != record["v1_semantic"]:
            raise fail(
                where,
                f"v1_semantic says {record['v1_semantic']}, reference model says {semantic_status}",
            )
    return len(records)


def validate_fixtures(root: Path) -> int:
    count = 0
    for filename, validator in (
        ("discover.jsonl", validate_request),
        ("requests.jsonl", validate_request),
        ("responses.jsonl", validate_response),
        ("errors.jsonl", validate_response),
    ):
        path = root / filename
        records = read_jsonl(path)
        for line_number, _raw, value in records:
            validator(value, f"{path}:{line_number}")
        count += len(records)
    count += validate_compatibility(root / "compatibility.jsonl")
    return count


def raw_line_boundary_self_tests() -> None:
    prefix = b'{"version":1,"request_id":0,"op":"start","args":{"config":"'
    suffix = b'"}}\n'
    exact = prefix + b"x" * (MAX_LINE_BYTES - len(prefix) - len(suffix)) + suffix
    utf8 = b'{"version":1,"request_id":0,"op":"start","args":{"config":{"text":"caf\xc3\xa9"}}}\n'
    with tempfile.TemporaryDirectory(prefix="protocol-fixture-boundary-") as directory:
        exact_path = Path(directory) / "exact.jsonl"
        exact_path.write_bytes(exact)
        exact_records = read_jsonl(exact_path)
        if len(exact_records) != 1:
            raise FixtureError("exact line-limit fixture was not read")
        validate_request(exact_records[0][2], "exact line-limit fixture")
        utf8_path = Path(directory) / "utf8.jsonl"
        utf8_path.write_bytes(utf8)
        utf8_records = read_jsonl(utf8_path)
        validate_request(utf8_records[0][2], "UTF-8 line fixture")
        over_path = Path(directory) / "over.jsonl"
        over_path.write_bytes(exact[:-1] + b"xx\n")
        try:
            read_jsonl(over_path)
        except FixtureError as error:
            if "maximum" not in str(error):
                raise FixtureError(f"over-limit fixture returned wrong error: {error}") from error
        else:
            raise FixtureError("over-limit fixture unexpectedly succeeded")
    malformed_values = [
        b'{"version":1,"request_id":true,"op":"discover"}',
        b'{"version":1,"request_id":1.0,"op":"discover"}',
        b'{"version":1,"request_id":1,"op":"input","args":{"event":{"kind":"pointer","value":{"space":[],"x":0,"y":0}}}}',
        b'{"version":1,"request_id":1,"op":"discover","value":1e400}',
    ]
    for index, raw in enumerate(malformed_values):
        try:
            value = parse_json(raw, f"malformed self-test {index}")
            validate_request(value, f"malformed self-test {index}")
        except FixtureError:
            pass
        else:
            raise FixtureError(f"malformed self-test {index} unexpectedly succeeded")


class BoundedLineReader:
    def __init__(self, stream: Any) -> None:
        self.stream = stream
        self.selector = selectors.DefaultSelector()
        self.selector.register(stream, selectors.EVENT_READ)
        self.buffer = bytearray()

    def close(self) -> None:
        self.selector.close()

    def read(self, deadline: float, timeout: float) -> bytes:
        while True:
            newline = self.buffer.find(b"\n")
            if newline >= 0:
                line = bytes(self.buffer[: newline + 1])
                del self.buffer[: newline + 1]
                if len(line) > MAX_LINE_BYTES:
                    raise TransportError(f"response exceeds {MAX_LINE_BYTES} byte limit")
                return line
            if len(self.buffer) > MAX_LINE_BYTES:
                raise TransportError(f"response exceeds {MAX_LINE_BYTES} byte limit")
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TransportError(f"worker response timed out after {timeout:.3f}s")
            events = self.selector.select(remaining)
            if not events:
                raise TransportError(f"worker response timed out after {timeout:.3f}s")
            chunk = os.read(self.stream.fileno(), 65536)
            if not chunk:
                if self.buffer:
                    raise TransportError("worker closed stdout with an unterminated response line")
                raise TransportError("worker closed stdout")
            self.buffer.extend(chunk)

    def wait_eof(self, deadline: float, timeout: float) -> None:
        if self.buffer:
            raise TransportError("unexpected buffered data before expected pipe EOF")
        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TransportError(f"worker pipe EOF timed out after {timeout:.3f}s")
            if not self.selector.select(remaining):
                raise TransportError(f"worker pipe EOF timed out after {timeout:.3f}s")
            chunk = os.read(self.stream.fileno(), 65536)
            if not chunk:
                return
            self.buffer.extend(chunk)
            if len(self.buffer) > MAX_LINE_BYTES:
                raise TransportError(f"pipe data exceeds {MAX_LINE_BYTES} byte limit")


def _drain_stderr(stream: Any, retained: bytearray, lock: threading.Lock) -> None:
    global _active_stderr_drainers
    with _active_stderr_drainers_lock:
        _active_stderr_drainers += 1
    try:
        while True:
            chunk = stream.read(4096)
            if not chunk:
                return
            with lock:
                retained.extend(chunk)
                if len(retained) > 65536:
                    del retained[: len(retained) - 65536]
    finally:
        with _active_stderr_drainers_lock:
            _active_stderr_drainers -= 1


def active_stderr_drainers() -> int:
    with _active_stderr_drainers_lock:
        return _active_stderr_drainers


def terminate_child(process: subprocess.Popen[bytes]) -> None:
    group_signal_available = os.name == "posix"
    if os.name == "posix":
        # start_new_session gives this worker and descendants a private group.
        # Signal it even when the direct child has already exited; descendants
        # may still own the pipes and keep the transport alive.
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except (PermissionError, ProcessLookupError):
            group_signal_available = False
            if process.poll() is None:
                process.terminate()
    elif process.poll() is None:
        process.terminate()
    try:
        process.wait(timeout=0.5)
    except subprocess.TimeoutExpired:
        pass
    if group_signal_available:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except (PermissionError, ProcessLookupError):
            group_signal_available = False
            if process.poll() is None:
                process.kill()
    elif process.poll() is None:
        process.kill()
    try:
        process.wait(timeout=0.5)
    except subprocess.TimeoutExpired:
        # The process group has been signalled twice; never make fixture
        # validation wait indefinitely for a broken child.
        pass


def write_request(process: subprocess.Popen[bytes], raw: bytes, deadline: float) -> None:
    if os.name != "posix":
        raise TransportError("worker transport requires nonblocking POSIX pipes")
    file_descriptor = process.stdin.fileno()
    os.set_blocking(file_descriptor, False)
    selector = selectors.DefaultSelector()
    selector.register(file_descriptor, selectors.EVENT_WRITE)
    offset = 0
    try:
        while offset < len(raw):
            if time.monotonic() >= deadline:
                raise TransportError("worker request write timed out")
            try:
                offset += os.write(file_descriptor, raw[offset:])
            except BlockingIOError:
                pass
            if offset == len(raw):
                if time.monotonic() > deadline:
                    raise TransportError("worker request write timed out")
                return
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TransportError("worker request write timed out")
            if not selector.select(remaining):
                raise TransportError("worker request write timed out")
    finally:
        selector.close()


def run_worker(command: list[str], requests: Iterable[bytes], timeout: float) -> list[dict[str, Any]]:
    if not command:
        raise TransportError("worker command is empty")
    process = subprocess.Popen(
        command,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=(os.name == "posix"),
    )
    stderr = bytearray()
    stderr_lock = threading.Lock()
    stderr_thread = threading.Thread(
        target=_drain_stderr,
        args=(process.stderr, stderr, stderr_lock),
        daemon=True,
    )
    stderr_thread.start()
    reader = BoundedLineReader(process.stdout)
    results: list[dict[str, Any]] = []
    try:
        for request_raw in requests:
            if not request_raw.endswith(b"\n") or len(request_raw) > MAX_LINE_BYTES:
                raise TransportError("request stream contains an invalid or oversized raw line")
            try:
                request = parse_json(request_raw[:-1], "worker request")
                request_id = integer_token(request["request_id"], "worker request_id", 0, U64_MAX)
            except (KeyError, TypeError) as error:
                raise TransportError(f"request stream has no usable request_id: {error}") from error
            deadline = time.monotonic() + timeout
            write_request(process, request_raw, deadline)
            response_raw = reader.read(deadline, timeout)
            try:
                response_text = response_raw[:-1].decode("utf-8")
            except UnicodeDecodeError as error:
                raise FixtureError(f"worker response is not UTF-8: {error}") from error
            response = parse_json(response_text, "worker response")
            validate_response(response, "worker response")
            if response["request_id"] != request_id:
                raise TransportError(
                    f"response id {response['request_id']} does not match request {request_id}"
                )
            results.append(response)
    except (BrokenPipeError, OSError) as error:
        raise TransportError(f"worker stdin failed: {error}") from error
    finally:
        reader.close()
        try:
            process.stdin.close()
        except OSError:
            pass
        terminate_child(process)
        # A process-group descendant can inherit stderr.  Close descriptors
        # only after the bounded drainer reaches EOF; a locked inherited pipe
        # must never turn cleanup into an unbounded close operation.
        stderr_thread.join(timeout=1.0)
        if not stderr_thread.is_alive():
            process.stderr.close()
        process.stdout.close()
        if stderr_thread.is_alive():
            raise TransportError("stderr drainer survived worker cleanup")
    return results


def transport_self_tests() -> None:
    request = b'{"version":1,"request_id":0,"op":"discover"}\n'
    python = sys.executable
    malformed = [python, "-c", "import sys; sys.stdout.write('not-json\\n'); sys.stdout.flush()"]
    try:
        run_worker(malformed, [request], 0.5)
    except FixtureError:
        pass
    else:
        raise FixtureError("malformed responder self-test unexpectedly succeeded")

    utf16 = [
        python,
        "-c",
        "import sys; sys.stdout.buffer.write('{\"version\":1,\"request_id\":0,\"kind\":\"acknowledged\"}\\n'.encode('utf-16')); sys.stdout.buffer.flush()",
    ]
    try:
        run_worker(utf16, [request], 0.5)
    except FixtureError as error:
        if "UTF-8" not in str(error):
            raise FixtureError(f"UTF-16 responder self-test returned wrong error: {error}") from error
    else:
        raise FixtureError("UTF-16 responder self-test unexpectedly succeeded")

    overflowing_json = [
        python,
        "-c",
        "import sys; sys.stdout.write('{\"version\":1,\"request_id\":0,\"kind\":\"observation\",\"value\":1e400}\\n'); sys.stdout.flush()",
    ]
    try:
        run_worker(overflowing_json, [request], 0.5)
    except FixtureError as error:
        if "non-finite" not in str(error):
            raise FixtureError(f"overflowing JSON responder self-test returned wrong error: {error}") from error
    else:
        raise FixtureError("overflowing JSON responder self-test unexpectedly succeeded")

    timeout = [python, "-c", "import time; time.sleep(2)"]
    try:
        run_worker(timeout, [request], 0.1)
    except TransportError as error:
        if "timed out" not in str(error):
            raise FixtureError(f"timeout responder self-test returned wrong error: {error}") from error
    else:
        raise FixtureError("timeout responder self-test unexpectedly succeeded")

    oversized = [
        python,
        "-c",
        f"import sys; sys.stdout.write('x' * {MAX_LINE_BYTES + 1}); sys.stdout.flush()",
    ]
    try:
        run_worker(oversized, [request], 2.0)
    except TransportError as error:
        if "exceeds" not in str(error):
            raise FixtureError(f"oversized responder self-test returned wrong error: {error}") from error
    else:
        raise FixtureError("oversized responder self-test unexpectedly succeeded")

    exact_response = [
        python,
        "-c",
        (
            "import sys; "
            f"p=b'{{\"version\":1,\"request_id\":0,\"kind\":\"observation\",\"value\":\"'; "
            "s=b'\"}\\n'; "
            f"sys.stdout.buffer.write(p+b'x'*({MAX_LINE_BYTES}-len(p)-len(s))+s); "
            "sys.stdout.buffer.flush()"
        ),
    ]
    exact_results = run_worker(exact_response, [request], 3.0)
    if len(exact_results) != 1 or exact_results[0]["kind"] != "observation":
        raise FixtureError("exact-limit responder self-test returned the wrong response")

    # The child neither reads stdin nor stops writing stderr.  The request is
    # intentionally close to the protocol limit so the harness must enforce
    # its deadline during nonblocking write, while the stderr drainer prevents
    # the child from deadlocking on its diagnostic pipe.  The child also
    # leaves a descendant holding inherited pipes; process-group cleanup must
    # signal descendants and reap the direct child without waiting on an
    # unbounded reader close.
    nonreading = [
        python,
        "-c",
        (
            "import os,subprocess,sys,time; "
            "subprocess.Popen([sys.executable,'-c','import time; time.sleep(10)']); "
            "os.write(2,b'e'*1000000); time.sleep(10)"
        ),
    ]
    prefix = b'{"version":1,"request_id":0,"op":"start","args":{"config":"'
    suffix = b'"}}\n'
    large_request = prefix + b"x" * (MAX_LINE_BYTES - len(prefix) - len(suffix)) + suffix
    started = time.monotonic()
    try:
        run_worker(nonreading, [large_request], 0.2)
    except TransportError as error:
        if "timed out" not in str(error):
            raise FixtureError(f"nonreading responder self-test returned wrong error: {error}") from error
    else:
        raise FixtureError("nonreading responder self-test unexpectedly succeeded")
    if time.monotonic() - started > 2.0:
        raise FixtureError("nonreading responder cleanup exceeded its bounded time")

    if os.name == "posix":
        process = None
        reader = None
        descendant_code = (
            "import signal,sys,time; "
            "signal.signal(signal.SIGTERM,signal.SIG_IGN); "
            "sys.stdout.write('descendant-ready\\n'); sys.stdout.flush(); "
            "sys.stderr.write('descendant-stderr\\n'); sys.stderr.flush(); "
            "time.sleep(30)"
        )
        parent_code = (
            "import subprocess,sys; "
            f"subprocess.Popen([sys.executable,'-c',{descendant_code!r}], "
            "stdin=sys.stdin,stdout=sys.stdout,stderr=sys.stderr)"
        )
        try:
            process = subprocess.Popen(
                [python, "-c", parent_code],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                start_new_session=True,
            )
            reader = BoundedLineReader(process.stdout)
            ready = reader.read(time.monotonic() + 2.0, 2.0)
            if ready != b"descendant-ready\n":
                raise FixtureError("descendant cleanup fixture did not report readiness")
            process.wait(timeout=2.0)
            terminate_child(process)
            reader.wait_eof(time.monotonic() + 2.0, 2.0)
        finally:
            if process is not None:
                terminate_child(process)
            if reader is not None:
                reader.close()
            if process is not None:
                for stream in (process.stdin, process.stdout, process.stderr):
                    try:
                        stream.close()
                    except OSError:
                        pass

    if active_stderr_drainers() != 0:
        raise FixtureError("a transport self-test left a stderr drainer alive")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--stream",
        type=Path,
        default=Path(__file__).with_name("discover.jsonl"),
        help="raw request stream for --worker (default: discover.jsonl)",
    )
    parser.add_argument("--timeout", type=float, default=5.0, help="per-response worker timeout")
    parser.add_argument("--no-self-tests", action="store_true", help="skip transport-only responder tests")
    parser.add_argument(
        "--worker",
        nargs=argparse.REMAINDER,
        help="worker executable and arguments; no shell is used",
    )
    args = parser.parse_args(argv)
    if not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error("--timeout must be finite and positive")
    root = Path(__file__).resolve().parent
    try:
        count = validate_fixtures(root)
        self_tests_run = not args.no_self_tests
        if self_tests_run:
            raw_line_boundary_self_tests()
            transport_self_tests()
        if args.worker is not None:
            requests = [raw for _line, raw, _value in read_jsonl(args.stream.resolve())]
            for line_number, _raw, value in read_jsonl(args.stream.resolve()):
                validate_request(value, f"{args.stream}:{line_number}")
            responses = run_worker(args.worker, requests, args.timeout)
            suffix = " and transport self-tests" if self_tests_run else ""
            print(f"validated {count} fixtures and {len(responses)} worker responses{suffix}")
        else:
            suffix = " and transport self-tests" if self_tests_run else ""
            print(f"validated {count} protocol fixtures{suffix}")
    except (FixtureError, TransportError, OSError) as error:
        print(f"protocol fixture validation failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
