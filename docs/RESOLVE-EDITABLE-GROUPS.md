# Internal Resolve editable Group writer

The internal `resolve-group-probe` accepts one graph/2 sequence that uses a
single Group with two continuous, same-rate 1080p23.976 DNxHR LB cameras. The
first camera provides two independent PCM24/48 kHz mono lanes. Each camera
needs an independently verified OP1a binding. The harness accepts the retained
Rec.709 inputs and the zero-timecode calibration case with unknown transfer
and primaries. Each profile preserves the corresponding observed radiometry.

```text
resolve-group-probe GRAPH SEQUENCE-ID 1920x1080 ORIGIN|source A.json B.json SKELETON FFPROBE OUTPUT.drp
```

The harness uses the same binding schema as the
[straight-cut writer](RESOLVE-PROJECT-CORE.md). A and B follow the saved
Group's picture alternative order. The known direct source clock wins.
If the source start is wholly unresolved, an explicit destination frame
count is required and reported. An explicit value conflicting with a known
clock refuses. The caller supplies the private, hash-pinned same-build
synthetic Group skeleton.

The writer constructs the two camera bindings, bounded inner Group spans,
and every saved outer selection and edit range. It validates each saved use
against its one bound leaf placement. Audio selections are interpreted by
saved lane and channel identity. The archive contains a native editable
multicam item with separate V1, A1 and A2 edits. Group marks and raw selector
metadata remain in graph/2. Only the Group referenced by the selected
sequence is written.

## Verification

Resolve Studio 21.2.0.0006 reopened the saved Avid Group conversion with
the qualified 01:00:00:00 start, nine edits and both media files online.
All 145 frames matched the selected source cameras at A/B/A cuts. A native
re-export and second import preserved both mono routes and a fresh insert
produced picture plus two independently mapped mono edits.

A synthetic companion with the same cuts distinguished four source signals.
Both isolated reopened lanes retained only camera A's intended signal and
repeated byte for byte over 290,290 samples. They match the native Group
control from sample 10,000 through the end. The native Group's startup
envelope differs during the first 10,000 samples. Direct file controls differ
only at 142 sample positions per lane near the ends. No gain or offset
correction was fitted. This verifies the saved selection, routing and interior
sample placement. Exact startup-envelope equivalence remains open.

Switching audio, unresolved identities, multipart and mixed-rate members,
other codecs, and product exposure remain outside this profile. The Resolve
route remains internal.

Resolve evidence: RESOLVE-0004 at 5df27091448cfad6de6667357615788612c49bea. SHA-256 11564bb16ace6f62f2c36e2d3fb7e6d4a6068df4314f4e07509391bca80d6fae.
