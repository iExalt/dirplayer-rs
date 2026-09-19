# Native GPU discovery

Checkpoint: 2026-09-19. This is discovery evidence for the native Flash
execution route. It does not change the renderer, backend selection, or Flash
runtime.

The retained WGPU diagnostic used `wgpu 27.0.1` with Ruffle's primary mask
(`VULKAN | METAL | DX12 | BROWSER_WEBGPU`), `HighPerformance`, no surface, and
`force_fallback_adapter=false`.

The sandbox result enumerated no adapters. Its raw WGPU error was:
`NotFound { active_backends: METAL, requested_backends: VULKAN | METAL | DX12 |
BROWSER_WEBGPU, no_adapter_backends: METAL }`, with display text saying that
Metal found no adapters. The outside-sandbox result using the identical probe
enumerated an Apple M4 integrated Metal adapter; `request_adapter` and
`request_device` both succeeded. The sandbox and outside environment snapshots
were otherwise identical. `system_profiler` reported the Apple M4 in both
runs, but only the outside run exposed the active Metal 4 display topology.

Raw retained files:

- `result-sandbox.json` — SHA-256
  `3386bb6471ca9887b9d1c25814866bce4d6d974b264ca0eca1ad4d227d400ebb`
- `result-escalated.json` — SHA-256
  `196dae1ad682b4a3a9746a5c6dc47dadd5f5d15753323a1f1d0f0b51dba40eee`
- `system-profiler-sandbox.json` — SHA-256
  `135c2331a42f072d98677ce33c7cf9737dba2d0b899b1d9ef8800f961a84f557`
- `system-profiler-escalated.json` — SHA-256
  `25990d2fd9e4579e7e2f351e2210e2f9b843ca350fc11a4981e35768df25361f`

The evidence supports an execution route with Metal access outside the
restricted sandbox. It does not justify substituting another backend. The
Apple OpenGL wording from the generic Ruffle error mapper should be treated as
an imprecise mapping until a runtime change is separately approved.
