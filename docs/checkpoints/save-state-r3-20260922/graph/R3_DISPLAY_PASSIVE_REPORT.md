# R3 passive display capture and allocation discriminator

Date: 2026-09-22

Status: **accepted partial milestone**. This milestone passively captures the
actual frame-371 Stage tree and its AVM1 identity links. It does not allocate a
fresh display tree, restore topology, normalize a candidate, prove candidate
cleanup, continue execution, or complete R3.

## Pinned inputs

| Item | Identity |
| --- | --- |
| Parent baseline | `7090796e83940adae97f5532d4ff62c3124577ec` |
| Ruffle baseline | `979ca0ef98769a29f8d7b466a2133d45780afa9d` |
| Accepted Ruffle commit | `d735dfa811ee415500199a97b701bf9172523b0d` on `iExalt/ruffle:r3-display-passive` |
| Asset | `opening_anim.swf`, SHA-256 `d964a7e594109f8923004333e129f9c655fabde0533a4b4bf74dce238bf3215f` |
| Tooling | lane `mise.toml`, Cargo `--locked --offline` |

## Passive actual-profile result

The read-only capture at frame 371 contains 11 display nodes: one Stage, five
MovieClips, and five Graphics. It records 10 render-tree edges, 10 depth-index
edges, 10 direct-parent edges, four AVM1 execution-list edges, five exact AVM1
object links, no mask edges, and no weak entries, for 39 strong edges total.
The MovieClip AVM links resolve to census graph IDs 72 through 76. Every
captured direct base, interactive, container, MovieClip, Graphic, Stage, and
owner-receipt field is represented or causes capture to reject.

The Graphic definition receipt includes its exact `DefineShape` source range;
the prior whole-asset placeholder was removed. The actual fixture confirms the
source census receipt, frame 371, and callback FIFO are unchanged across the
read-only capture. This fixture does not compare rendered RGBA or execute a
continued frame, so it is evidence only for passive capture behavior.

Two focused negative tests reject a populated unsupported field and a
direct-parent/topology mismatch. Capture retains the 10,000-node,
50,000-strong-edge, and 10,000-weak-entry bounds.

## Fresh-allocation discriminator

The first connected-subtree attempt selected Stage/root plus display IDs 1
through 6. It failed before its first candidate mutation with:

```text
AssetParse("decode: Couldn't read SWF: failed to fill whole buffer")
```

Two bounded repairs advanced the same gate: root character 0 stopped requiring
a `DefineSprite` tag, and tag payload slicing stopped truncating the reader. A
source review then found the remaining scanner defect: it continued past the
logical `End` tag, unlike Ruffle's normal preload path.

The separately authorized parser-only check exits at `TagCode::End`, treats
root MovieClip character 0 as the root `SwfSlice`, and resolves this exact
selection from the pinned SWF:

| Kind | Character | Source range | Frames |
| --- | ---: | ---: | ---: |
| MovieClip | 0 | `0..39012` | 419 |
| MovieClip | 3 | `7102..7114` | 1 |
| MovieClip | 6 | `22239..22251` | 1 |
| MovieClip | 8 | `22768..22780` | 1 |
| MovieClip | 10 | `22844..22862` | 2 |
| Graphic | 11 | `22980..23013` | n/a |

The parser-only fixture passed with `logical_end_reached=true` and
`data_len=39012`; a malformed/truncated three-byte `FWS` input rejected with
`AssetParse`. The check constructed no candidate, attached no display object,
and invoked no preload, seek, frame, action, or getter path. The corrected
parser was deliberately not followed by another allocation attempt because the
authorized discriminator stopped after this hypothesis result.

## Split and retained work

The published child branch contains only the passive DTOs/adapters,
capture, validation, negative tests, and actual passive fixture. The unfinished parser,
candidate allocation, topology mutation, partial normalization, and fixture
assertions remain recoverable in local task-owned WIP commits:

- Ruffle `b048f1598c25f9f70a34e2936f7efa6cbbda7528`
- parent `7bd7289b07c1a3d53a85426d0e9e4b6d67262aa7`

Neither WIP commit is proposed for publication. The attempted topology path
uses normal `replace_at_depth`, has no accepted no-event attachment contract,
normalizes only a partial field set, and never reached its cleanup-token or
source-after assertions.

## Verification

The exact commands and concise results are recorded in
[`r3-passive-receipt.txt`](r3-passive-receipt.txt). The split passive tree
passes the Ruffle core check, both focused negative tests, the actual frame-371
display fixture, and the accepted R2 actual 16-node AVM1 fixture. Parent and
child `git diff --check` pass.

## Remaining route and preliminary estimate

R3 still needs a guarded fresh candidate path that allocates a smallest
connected subtree without constructors, scripts, events, or ordinary topology
side effects; links display IDs to exact AVM graph IDs; fixes every captured
field; compares the complete normalized receipt; and asserts atomic cleanup.
That smallest-subtree step is preliminarily 0.5 to 1 active day. Expanding it to
all 11 actual display nodes and their full canonical field comparison is another
1 to 2 active days.

Arrays/accessors, the remaining AVM1 graph, timelines, render resources,
clocks/queues/callback generations, exact continued RGBA/callback equality,
fresh-worker operation, Director audio, and C1 remain outside this partial
result. The broader remaining fresh-Player experiment is preliminarily 4 to 7
additional active days and requires a new scope decision; this is an agent
estimate, not measured delivery time or approval.
