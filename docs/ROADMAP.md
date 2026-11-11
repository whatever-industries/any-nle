# Any NLE: roadmap / to-do

Checked items identify the completed scope. Test results distinguish converter
regressions, editor read-back and User acceptance. The four areas put on hold on
2026-10-28 remain in their own section.

Reviewed on 2026-11-08. The converter includes the bounded editable Group
implementation and its native checks. Research references are pinned in
`research-dependencies.json`.

## Declared multichannel media (User, 2026-11-07)

The 7.1 review retains eight-channel Adaptive plus explicit routing; it is not a
dedicated 7.1 speaker bus. Surround timelines now preserve the selected preset’s
DF/NDF display policy. Review and correction.

- [x] Add explicit 5.1 and eight-channel Adaptive group preparation for DNx OP1a and
      bounded ProRes LT MOV, preserving reordered Avid labels and PCM16/PCM24 values.
      Converted timelines, fresh bin insertion and actual native save/reopen pass
      the bounded Mac cohort. Scope, CLI and verification.
- [x] Reconcile current acceptance inputs separately from the immutable historical
      manifest: reviewed CONV-0034, explicit 60-model selection and a fresh native
      reference reproducing retained PCM/pictures. The historical return's provenance
      remains unresolved. Qualification and acceptance record.
- [ ] Resolve **59.94 short-loop playback performance**: selected DNx Adaptive8
      runs report 69 and 47 dropped frames, including a repeat without offline work.
      Audio and sampled pictures remain exact. No converter/container cause is established;
      retain this gate rather than making a runtime change from counters alone.
- [ ] Finish User listening/acceptance. Broader ProRes geometry/rates, Windows and
      long-source streaming remain pending. Three other selected ten-minute loops
      report zero drops; these short synthetic cases do not qualify long footage.
- [ ] Qualify mixed-width surround timeline output before adding routing/downmix policy.
      Explicit refusals remain. The native wizard/default menu is implemented; its wider
      application and platform qualification remains separate.

## Original links, native wizard and release preparation (2026-11-08)

Implementation, evidence and remaining gates.

- [x] Typed linked-original source/writer route, explicit stream/channel selection,
      locator/root-map resolution, content pins and bounded physical probing.
- [x] Shared prepare-once GUI/CLI job engine, project inventory/dependencies, destination
      folder layout, explicit losses, settings replay and safe publication reporting.
- [x] Five-page native Rust wizard, presets, color/effect review, cancellable work and
      verified adjacent LGPL-only backend resolution.
- [x] Six native target definitions, MIT/third-party notices/source collection, private
      candidate gates and non-overwriting draft-release tooling.
- [x] A generated H.264/AAC master matches sampled picture and PCM from Premiere's
      native import in a two-second macOS fixture.
- [x] **Link picture-only 1080p24 ProRes HQ MOV** (CONV-0054). Use the
      saved Avid locator with the observed zero-origin timecode track and
      a verified picture packet grid. The original and generated freeze
      stayed online after Save As and reopen in Premiere 26.5.2. The
      linked original matched all 240 direct-import pictures. The
      generated freeze matched all 120 Avid pictures.
- [x] Preserve stereo left/right on fresh inserts for the scoped linked AAC bin
      route. The mapped two-second fixture stayed linked and played after
      Premiere Save As and reopen. Existing sequence audio keeps its source grouping.
- [x] Block Premiere conversion before writing when unsupported source rate
      routing or a recognized automatic Avid Motion Adapter would omit timeline edits.
      Review names the affected sequence and tracks. A same-rate UME/OP1a control
      stays online after Premiere Save As and reopen.
- [x] Translate the qualified automatic 24 fps to 23.976 Motion Adapter on
      linked H.264/AAC. Premiere Save As and reopen matched the Avid picture cadence
      and both Direct Out PCM lanes on one trimmed mixed-media sequence.
      [Scope and refusals](MOTION-ADAPTER-24-TO-23976.md).
- [x] Translate the qualified automatic 23.976 to 24 Motion Adapter on linked
      H.264/AAC. Premiere Save As and reopen matched the picture repeat and
      both PCM lanes within the MP4's declared extent. The conversion reports
      silence beyond that boundary where Avid can render decoded AAC padding.
      [Scope and refusals](MOTION-ADAPTER-24-TO-23976.md#automatic-23976-to-24).
- [ ] Broaden native AVB-to-UME playback qualification with representative saved
      bins. The supplied long-form MP4 also failed Premiere's direct import.
- [x] **Read back linked PCM16 and PCM24 WAV in Premiere.** Synthetic stereo
      writer controls retained exact source trims, left/right samples and fresh
      insertions after Save As and reopen. In this audio-only control, the
      encoder began the export at the first occupied edit. The Premiere
      sequence retained the leading silence.
- [x] **Saved Avid-linked PCM16 and PCM24 WAV via the job engine.** Two
      WaveAiff links at 48 kHz retained A1/A2 as separate mono placements. With
      Avid default pan selected, Premiere matched both 48,000-sample trimmed
      Direct Out channels after Save As and reopen. Fresh inserts retained
      both source selectors. Premiere centers new mono clips by default.
      Explicit clip hard pans matched all 96,000 samples per source channel.
- [ ] **Align original-file resolution across entry points.** The wizard and
      saved-job command share original-file verification. Conventional
      `graph` and `convert` use native media lookup and report these linked
      WAV file mobs missing. Share the verified route before expanding
      descriptor, legacy AMA and platform coverage.
- [x] Remove the identified private diagnostics from reachable GitHub export history.
      The reviewed private export and cleanup evidence are retained.
- [x] Publish [Any NLE v0.1.0-alpha.1](https://github.com/whatever-industries/any-nle/releases/tag/v0.1.0-alpha.1)
      from the reviewed public source. All six platform builds, extracted-package audits
      and download checksum checks passed. [Release gates](RELEASE.md).

## Cataloged audio configuration gaps (User, 2026-11-07)

Complete audit and evidence boundaries.
Grouping/order, master output mix, timeline track capacity and native qualification
are separate. No held mixing/speaker-policy work is resumed by this list.

- [x] **Priority 1 — Preserve and diagnose Master Clip Output Mix Format.**
      Raw absence/type/value now survives extraction and graph/2, including unresolved
      masters and bounded opaque values. Preparation reports present settings as unapplied;
      legacy model/1 warns without a schema change. Missing never implies Stereo.
- [x] **Priority 1 — Plan destination audio-track capacity before writing.**
      Both writers allocate to the highest event/group/level/fade lane within 1..99,
      preserving the selected clock and output bus. Mono-discrete bus expansion refuses.
      Eight-mono, complete Phase25-like six-track and sparse A99 controls pass structurally.
      Fresh native qualification of expanded outputs remains pending below.
- [x] **Priority 2 — Complete permanent named-layout regression coverage.**
      5.1 AAC and 7.1 SMPTE DS now have reader, graph, preparation and MXF/MOV writer
      cases; all six named wide layouts have literal saved-selector checks. Mono/stereo,
      custom sparse/reversed order and composite controls remain covered.
- [ ] **Priority 2 — Qualify each cataloged layout end to end.**
      Native cohorts cover selected 5.1 SMPTE and 7.1 EXT declarations, not all named
      variants. Use admitted media, distinct channel signals, complete PCM, fresh insertion
      and save/reopen; keep source codec, grouping, output mix, speaker monitoring and
      audio-only admission separate. No long native test is scheduled by this audit.
- [x] **Priority 2 — Reconcile CONV-0034's grouping provenance.**
      Added the reviewed exact AVB-0005 revision/hash for Phase24/25; prior pins and
      confidence unchanged. CONV-0035 records preservation/capacity boundaries.
- [ ] **Priority 2 — Native-check expanded destination track counts.**
      Fresh insertion, routing and save/reopen for full eight-mono and two-pair/four-mono
      timelines; sparse high lanes need their own application check. Structural tests
      preserve buses and DF/NDF; they do not qualify editor playback.

Implemented fixes, tests and remaining native gates.

## Project import (User, 2026-10-14)

- [x] **Central source-graph migration implementation** (2026-11-05). Reliability,
      graph/2, direct Avid source extraction, explicit Premiere preparation and
      prepare-once publication are integrated locally. Required fixtures, full
      suites, project/LUT differentials, bounded memory comparisons and a separate
      final canonical verification pass are complete; private receipts retain
      the frozen baseline, exact corrections and qualification limits.
      Corrected native timing and wider application qualification remain separate.
      [Design and gates](SOURCE-GRAPH-MIGRATION.md),
      correction ledger.

- [ ] **Finish split-edit listening and User checks**
      (2026-11-05). Native Avid control/fade fixtures, graph/2 preparation and
      Premiere save/reopen preserve split ranges, mono routes and asymmetric
      audio transitions. The omitted TC1 marker correction (`07fe580`) is integrated
      locally. The 2026-11-06 native audio renders
      pass sample timing/routing and fade-window checks. The
      2026-11-07 follow-up adds three normal
      passes and over ten minutes of native looping per sequence, unchanged post-loop
      PCM and a native Avid reference. The measured Avid envelope differs from the
      current Constant Power approximation. User listening, faithful fade mapping
      and acceptance remain pending. See the release checks below for absolute TC1.
      Evidence and limits.

- [x] **Point the app at a whole Avid project folder.** Walk it recursively, select
      `.avb` (case-insensitive: real projects contain `.AVB`), and skip everything else
      (`.avp`, `.log`, `.lck`, `.awf` waveform caches, `.xml`, `Statistics/`, `WaveformCache/`,
      `.DS_Store`, …).
- [x] **Keep selected bin folders** as nested Premiere bins under one UTC
      `AVID_IMPORT_{YYYY-MM-DD_hh:mm:ss}` root.
- [x] **Show import progress** by stage, with the current bin and count during bin
      processing. Preflight losses and results appear in the report.
- [ ] Show a running problem count during bin processing.
- [x] **Read one decoded AVB at a time**, including folder imports through the
      shared job engine. Compact per-bin facts support cross-bin references.
      Streaming measurements and limits.

- [ ] **Reduce large-bin decoding and retained project/output memory.** The whole-file decoder
      still has per-bin temporaries, and summaries/output grow with project size. The
      standalone legacy model API retains its 16 MiB limit; direct source facts and
      encoded graph/2 have distinct 64 MiB bounds. Historical private selections reached the 4 GiB benchmark
      guard. Reference scale: one real project had 2,986 bins in 246 folders, 11 GB, largest
      bin 136 MB; reading one decoded bin at a time is not a constant-memory project importer.
- [x] **Trash/Attic opt-in** in the project step. The implemented default is off.
- Duplicate bin names in one folder: not possible (Avid forbids two same-named bins in a
  folder, User 2026-10-14); a case-only clash on a case-insensitive disk is the one thing to
  report rather than assume.
- Lock files (`.lck`): ignored; the bin is read as saved (User, 2026-10-14).

## Media linking (User, 2026-10-14)

- [x] **OP-Atom → OP1a CLI preparation** (User, 2026-11-04; CONV-0030).
      `any-nle convert ... --rewrap-op1a NEW_DIR` displays **“Creates new media.”**,
      estimates disk use and uses external FFmpeg/ffprobe 8.1 with no encoding. Optional
      `--rewrap-dry-run` writes nothing. The 2026-11-04 extension covers 20 exact MXF
      rates, including 24000/1001, common broadcast rates and experimental extended
      rates through 120; extended rates use reported frame-count display. Video-only
      or 1–32 discrete mono 48 kHz PCM tracks at one 16/24-bit depth per output,
      require exact matching source clocks, spans and sample counts. Inputs stay in
      place. PCM bytes, stream hashes and exact packet clocks are verified. Default
      conversion behavior and model schema are unchanged. Existing timeline-layout
      limits are checked before media creation. [Rate/audio scope](MEDIA-REWRAP.md).
- [x] **Experimental native OP1a-backed AV-master route** (CONV-0031).
      Explicit `--rewrap-native-master` replaces bin mode with ordinary file-backed
      masters, preserves mono/ordered stereo groups, source routing, exact clocks and
      effective gains, and shares physical media across distinct logical masters.
      Matched Rust/Python and structural/backend checks; bounded native trials are recorded
      below. User acceptance and wider qualification remain pending.
      [Use, refusals and tests](NATIVE-OP1A-MASTERS.md).
- [x] **Correct OP1a channel assignments and native bin grouping (bounded checks).**
      Full-source templates and physical-channel vectors preserve mono, normal/reversed
      stereo and non-adjacent stereo plus mono on fresh bin insertion. Timeline offsets,
      group gains, zero overrides and save/reopen exports pass in retained Premiere 26.5.2
      PCM16x2 / PCM24x2/x3 fixtures at 23.976 fps. Legacy constructions use corrected
      flattened channel views. Mixed-depth output refuses without normalization.
      [Evidence and remaining qualification](NATIVE-OP1A-MASTERS.md).
- [x] **Synthetic AVB end-to-end OP1a check** (2026-11-04). Actual CLI extraction,
      media identity matching and strict rewrap through Premiere save/reopen pass for
      three PCM16 or three PCM24 streams at 23.976 fps. Complete rendered timeline PCM,
      grouping/routing, picture offset/gap and refusal checks pass. Agent-operated,
      same-author double-check; no User acceptance or production-bin claim.
      Scope and evidence pins.
- [ ] **Widen OP1a native qualification.** User acceptance, additional Avid-created
      production bins, more channels, profiles and Premiere versions remain open.
      Selected declared 5.1 and Adaptive8 cohorts are covered above. Sparse labels
      and 32-source cases have backend coverage. Audio-only preparation, flexible
      source spans, derived mono-from-stereo, arbitrary MCA routing, unequal stereo
      gains and verified resume/reuse remain open. [Current scope](MEDIA-REWRAP.md).
- [ ] **Resolve and qualify direct OP-Atom alternatives** (User, 2026-11-06).
      A fresh DNxHR import supplies the missing 23.976 format110 and AVdh witnesses,
      with three pictures matching the OP1a return. Separate, Merged and Grouped
      still refuse that cohort before writing. Direct ProRes OP-Atom playback also
      remains unresolved. If direct playback cannot be qualified, revisit the User's
      conditional decision to offer only rewrap for managed OP-Atom. The current
      rewrap destinations are DNx to OP1a and admitted ProRes LT to MOV.
      Evidence and decision gate.

- [ ] **Resolve the retained anamorphic ProRes fixture and OP1a alternative**
      (2026-11-07). Its 446/447 duration/index conflict, 21952-byte terminal nonframe,
      SAR16:15/model geometry and identity mismatch remain outside automatic admission.
      The separate bounded correction preserves 446 pictures/clocks offline; native
      playback of that original-footage candidate remains pending. The implemented
      square-pixel ProRes LT MOV slice does not reuse a DNx terminal exception or
      silently normalize this source. Investigation.
- [x] **Rewrap admitted Avid ProRes LT OP-Atom to MOV** (User, 2026-11-07).
      Premiere originally raised “Frame substitution recursion” on a ProRes LT
      OP-Atom source. MOV stream copy played and exported in the retained trial.
      The converter now prepares admitted progressive square-pixel 1080p24 ProRes LT
      with PCM16/PCM24. Selected 5.1 and Adaptive8 macOS controls passed native render,
      reopen and ten-minute loops. Implementation.
      Playback checks.
- [ ] **Expand ProRes MOV qualification.** Wider rates and profiles, long production
      footage, User acceptance and Windows remain open. The retained anamorphic
      fixture has its own duration, identity and geometry task above.

- [x] **Implement bounded MOV audio bindings and compliant PCM descriptions**
      (2026-11-07). Discrete interleaved PCM has version-2 `lpcm` descriptions;
      wide MOVs use per-channel mono source views instead of OP1a's flattened views.
      AVB grouping controls vectors, including multiple pairs, 5.1 and eight-channel
      Adaptive. Separate mono MOV tracks and generic layouts remain unqualified.
      Implementation and read-back.
      Earlier failed controls remain retained.
- [x] **Native MXF preflight corrections** (2026-11-04; CONV-0032).
      Exact RI00/DNxHR LB label, verified sibling-package placeholders, explicit
      demuxer indexes, bounded private-header framing and source-offset timecode
      calculation. Scope, evidence and remaining blocker.
- [x] **Investigate the selected native DNxHR terminal packet.** A physical/frame-hash
      check and generated counterexamples distinguish the zero tail from a real extra
      picture and from index-only changes. A bounded copy prototype preserves coded
      video and complete PCM. Evidence and implementation requirements.
- [x] **Implement narrowly validated native DNxHR zero-tail preparation** (2026-11-05;
      CONV-0033). Scoped LB 1080p23.976 coded/index/packet checks, singleton zero-tail
      exclusion and separate original/copied/excluded hash receipts. The retained
      Avid-saved36-frame fixture preserves every coded picture and both complete PCM24
      channels through the converter. Guards, evidence and limits.
- [x] **Finish the bounded native Premiere read-back of the guarded native-source
      conversion** (2026-11-05). Actual Avid-saved fixture through CLI, native
      open/save/close/reopen, exact complete PCM24 channels, stable picture positions
      and playback endpoint pass in Premiere26.5.2 build5. Evidence and limits.
      User/wider acceptance remains pending. The full mixed native/UME bin's separate
      PCMA02 decoder gap remains open.
- [x] **Clarify the CLI construction summary** (2026-11-04). With
      `--rewrap-native-master`, the final summary identifies the experimental native
      OP1a AV-master route, consistent with writer notes and `conversion.json`.
      Ordinary conversions retain their selected bin-mode summary.
- [x] **Resolve the effective Premiere construction once** (2026-11-04).
      One internal route now drives preflight, writer selection, receipts and CLI
      reporting after policy overrides. Public interfaces, model/policy schemas,
      refusal ordering and project construction remain unchanged.
- [x] **Separate media-preparation responsibilities** (2026-11-04).
      The existing `Plan` interface now fronts focused modules for backend inspection,
      source fingerprints/space checks, clock/group validation, read-only planning,
      execution and receipt writes. Existing tests moved alongside them. Validation
      order, exclusive output handling, retained failures and receipt schemas stay
      unchanged. Entry point: [rewrap module](../crates/app/src/rewrap.rs).
- [x] **Media output folder prompt (macOS CLI app).** `--rewrap-op1a-prompt` opens a native
      parent-folder picker, then reviews the new child folder, “Creates new media.” notice,
      estimated additional bytes and available volume space before any outputs. Cancellation
      writes nothing. The CLI also supports explicit destination and dry-run.
      [CLI and wizard review](MEDIA-REWRAP.md#import-option-and-review).

- [ ] **General media repair for Premiere decode failures** (User, 2026-10-20).
      Scope this as an opt-in repair for media outside the current managed OP-Atom
      routes. Candidate inputs include XAVC or other long-GOP MXF, m2ts and H.264.
      Establish each failure in a native import and verify that a container-only
      change fixes it. Check packet timing, timecode, reel, color and pixel aspect.
      Record original identity, path and hash in a repair receipt for later relink.
      Keep the original files in place and report supported recovery steps.

- [ ] **Pixel aspect from Avid** (AniMatte read-back 2026-10-20): Avid stored 256:135 on a
      1920x1080 raster and showed it square; Premiere honours the file's 16:15 and stretches the
      picture (and its masks) 1.09x. Model/1 has no pixel-aspect field (Codex reports it as a loss).
      Confirmed for this one clip: with PAR 1:1 the AniMatte matches Avid (IoU 0.993 vs 0.907).
      Codex (2026-10-21): no KB rule says what makes Avid display it square (spatial adapter,
      project format, source interpretation or descriptor precedence); BCOL-039/070 are
      counterexamples to a direct descriptor rule. So: keep the descriptor value, report the
      display as unresolved, no automatic override. Needs a controlled Avid adapter/project-format
      comparison before a rule.

- The 2026-10-20 Premiere Replace Footage return recorded the QuickTime FourCC,
  color-space label and CCFileModTime. The typed linked-original route now covers
  qualified saved locators and explicit path mappings. Attaching Avid MXF as proxies
  to those originals is a separate task. Linked-original scope.

- [ ] Avid video MXF imported as **proxies** by default (user can override); the clip's
      **Source File / Source Path** linked as the full-resolution media when found.
- [ ] Import option **"Link Audio to source file (if found)"**.
- [ ] MDB/PMR media-database folder input (from the KB's findings, not Filmcraft code).
- [ ] **Convert offline, then relink.** Allow an explicit bin-only conversion when
      media is absent. Keep unresolved masters, edit ranges, and audio lanes as
      relinkable Premiere items where the bin establishes their identity and clocks.
      Show which facts and media are missing. When the drive arrives, verify relink
      against saved identity and timing before accepting it. Test a moved media root,
      duplicate names, split OP-Atom audio, and a mix of online and offline clips.
      Keep current online preflight and verified rewrap as separate paths.

## Explaining decisions (User, 2026-10-14)

- [x] Show in-app “Why?” guidance for conversion choices and report entries.
      [Decision reference](DECISIONS-EXPLAINED.md).
- [ ] Complete the side-by-side clip-construction comparison and add more specific
      explanations for report entries that still use general guidance.

## Audio

- [ ] Stereo edits with custom per-channel pans (e.g. 50L/25R): plain stereo + report today;
      decide on per-edit nested sequences after the User listens in Avid.
- [x] **Editable clip volume keys for direct same-rate mono edits** (CONV-0040).
      Read the PVOL volume control lane from inline CTRL points or the qualified
      subframe payload. Preserve the media-relative clock and write editable Premiere
      keys. Premiere read back both four-point controls using synthetic media.
- [ ] **Match PVOL rendered volume envelopes.** A reopened Premiere
      control on the verified dry WAV matched the flat Avid export exactly.
      Inline and subframe keys retained their times. Both keyed renders
      differed from Avid, including a −6.0 dB versus −6.1 dB plateau and
      different ramp shapes. Separate native gain and interpolation evidence
      is needed before changing the mapping. The historical managed-media
      conversion, additional layouts and nested owners remain open.
- [ ] Pan keyframes; master-clip pan (Premiere has no master-clip pan).

## Clip, marker and correction color

- [ ] **Color correction extensions**: keyframes and other Avid tabs (Channels, Levels,
      HSL Hue Offsets, Secondary). The measured HSL Controls (hue, saturation, brightness,
      contrast, gain, gamma, setup) and curves already feed the LUT writer; do not reimplement
      those as missing formulas. Decoding the retained Phase 52 10-bit 4:2:2
      source supports a Y′CbCr component limit near the physical code range
      before RGB clipping on the tested export path. Across the 12 flat
      patches, this reduces the maximum Gain 150 residual from 48.83 to 1.94
      8-bit-equivalent RGB codes. Contrast +30 improves from 19.18 to 2.66
      codes on the same patches. The hue sweep shows the same improvement.
      A later eight-patch export stayed inside the physical Y′CbCr limits
      and matched unclamped 1.5× gain within 1.06 RGB codes. Compare a
      rail-crossing chart in both Y′CbCr and RGB to locate the clipping and
      distinguish exact bounds before changing the LUT. Clip Low, Clip High,
      Invert Luma and Invert Chroma have saved parameter mappings and still
      need renders that vary one control at a time. Editable Lumetri controls
      are on hold below. [Source packing reference](https://pub.smpte.org/pub/rdd50/rdd50-2019.pdf#page=15).

## Timeline and effects

The format research worklists are `knowledge/avid/docs/effects-codex.md` and
`knowledge/premiere/docs/effects-codex.md`. Use their property evidence and the
User's priority tiers to scope converter tasks.

- [x] **SubCap → Premiere captions**, bounded content/timing path (2026-11-08).
      Exact native identities, strict UTF8/LF, source occurrence ownership and qualified
      record spans feed embedded subtitle cues. Native synthetic accented/two-line controls
      survive Premiere save/close/reopen. Styling remains an explicit destination-default
      loss; this is not full-media playback or general Unicode qualification.
      [Scope and evidence](CAPTION-TEXT-TRANSLATION-2026-11-08.md).
- [x] **Retain local SubCap appearance evidence** (CONV-0058). Graph/2
      preserves explicit FontSize, FONTFACE and ExtraLeading carriers plus
      opaque synchronization presence. Unqualified appearance receipts report
      a reason while independently qualified caption text survives.
- [ ] **SubCap font-size and layout mapping.** The Avid 20 px case omitted the
      field. Native 40, 50 and 60 px saves stored their displayed integers.
      Premiere cue-local style overrides survived reopen with unchanged
      neighboring cues. The direct shared-font mapping missed the two-pixel
      full two-line block-height gate at 40 px, and line widths differed. Resolve font
      metrics, line spacing and synchronization before applying source styling.
- [x] **Current Titler+ static single-flow text → editable Premiere text** (2026-11-08).
      Character and native Flow/Text paragraph carriers are supported separately. Retain
      native appearance values by property-list role; do not use cached `_TTEXT` as text.
- [ ] **Title appearance.** A native static fixed-box horizontal move
      changed only its local position value and shifted the glyphs 318 px.
      A separate saved-carrier width probe preserved the left edge and wrapped
      two lines into four in Avid. The current Premiere writer renders all
      three as the same point text with reported ArialMT/48pt defaults.
      Qualify a Premiere paragraph box, shared font metrics and anchor
      before mapping fixed-box appearance. Multiple flows/layers, dynamic
      modes, mixed carriers, Classic Title and Titler+ Legacy remain unsupported.

- [ ] **Alpha (keyed graphics)** (User 2026-10-15; Avid side: Codex alpha-test intake, provisional,
      KB 5bba92c, AVB-0006/AVB-0012 context). Avid facts: an image imported with alpha becomes a
      Matte Key clip (composition usage 3, EFF2_2DMATTE): track 2 = fill, track 3 = matte, as
      separate MXFs; "Ignore" gives a plain clip. Polarity is in the source's import settings
      (_IGNOREALPHA, _INVERTALPHA), not in the Matte Key parameters. Measured on a straight-alpha
      PNG: "Invert on import (white = opaque)" -> opacity = a; "Do not invert (black = opaque)" ->
      opacity = 1 - a; ordinary straight-alpha blending (MAE ~0.3 codes).
      Plan:
      1. Reader: resolve Matte Key clips (fill + matte file mobs, Source File, import alpha
         settings) instead of skipping usage-3 compositions; Matte Key edits in sequences.
      2. Preferred Premiere result: the original source file (via Source File, see Media
         linking) with Interpret Footage alpha set from Avid: Invert-on-import -> Premiere
         default; Do-not-invert -> Invert Alpha; Ignore -> Ignore Alpha (py_premiere writes
         these interpretation fields).
      3. Fallback when the original is missing: fill MXF with the matte MXF as a Track Matte
         Key (luma) in a small build, polarity from Avid. Needs: which essence each MXF holds
         (Codex: RSET routing open) and a Premiere Track Matte Key sample (storage unknown).
      4. Wizard option: "Keyed graphics: link original file / rebuild from Avid matte".
      5. Test: the Alpha Test fixture (black/white backgrounds, 3 modes + control) rebuilt in
         Premiere and compared pixel-for-pixel with Avid's TIFFs, as for colour.
      Open: premultiplied sources, other codecs, alpha in video files (ProRes 4444 etc.).

- [x] **Saved Timewarp ramp cadence** (CONV-0051). The reader carries the
      retained two-key Linear rise/fall and Spline rise profiles as saved
      picture samples. The Premiere writer uses one editable remap key per frame.
      On the rewrapped DNx OP1a chart, Premiere 26.5.2 retained the remap keys,
      curve In/Out points and media links after Save As, close and reopen.
      All 144 individually exported frames in each sequence matched Avid's frame selections.
      The Spline rise retained all 35 differences from the Linear rise.
- [x] **Default Speed Boost and Speed Bump picture timing.** For the two
      verified 1666-frame 1080p23.976 profiles, evaluate the four saved Position
      keys and handles and write one Premiere Time Remapping key per frame.
      After Save As, close and reopen, Premiere 26.5.2 retained the editable
      keys and DNx OP1a link and played both sequences through to the end.
      All 3,332 frame selections from the Premiere renders matched the Avid
      exports at 24000/1001. This fixture qualifies picture cadence.
- [ ] **Extend Timewarp coverage.** Qualify other durations, modified presets
      and inherited Avid render settings. Resolve other anchors, editable and
      generated map authority, exact constant 200% cadence, trims, nonzero media
      offsets, mixed rates, blending, optical flow and sound separately.
      Constant full-speed reverse already uses Premiere Reverse Speed.
- [x] **Traditional Freeze Frame, five-second 1080p24 profile** (CONV-0053). Preserve the
      generated ProRes HQ picture as a regular Premiere clip on the native
      media route. The converter checks the saved clip and sequence binding,
      copies all 120 coded pictures to MOV and excludes the verified zero-only
      packet beyond the picture span. Premiere 26.5.2 played the five-second
      sequence after Save As, close and reopen. All 120 rendered pictures
      matched the held Avid picture. Other durations, field modes, trimmed
      placements and editable Timewarp holds need separate qualification.
- [x] **Saved AudioSuite Gain, one- and two-second mono PCM24 at 24 fps**
      (CONV-0055, CONV-0057).
      Native media preparation follows the saved Gain render binding and copies
      its 48,000 or 96,000 playable samples to WAV after verifying the two extra stored
      samples are zero. The saved render remains separate from PVOL volume and
      pan. Premiere 26.5.2 matched all 144,000 stereo sample frames of each
      three-second Avid Direct Out after Save As, close and reopen using the Avid default-pan
      option. Other render spans, channel layouts and missing caches remain
      outside this profile.
- [x] **Linear animated Resize position and scale** (CONV-0041). Convert two
      aligned whole-frame keys on 1920x1080 same-rate edits to editable Premiere
      Motion, including independent X/Y scale. Premiere rendered start, middle,
      and end geometry in the synthetic source control.
- [x] **Linear animated 3D Warp Position and Scale** (CONV-0046). Exact
      promoted control identities and neutral crop/background feed the existing
      editable Motion writer on an admitted 1080p23.976 picture edit. The
      synthetic DNx comparison matched the Resize output across all 48 frames
      and kept the keys after Premiere Save As and reopen.
- [x] **Linear animated Resize and 3D Warp crop** (CONV-0047). Translate four
      source-space crop curves alongside position and independent scale to
      editable Motion. The paired 1080p23.976 synthetic DNx renders matched
      each other across 48 frames before and after Save As and reopen. Measured
      edges stayed within one pixel of the retained Avid reference.
- [x] **Saved Linear 3D Warp keys after head or tail trim** (CONV-0048).
      Preserve the saved three-key profile, including independent Fixed Width
      keys outside one visible edge where the source media has the handle.
      Five Fixed/Elastic cases preserve their editable keys and render after
      Premiere Save As and reopen. Premiere's own editing rules govern later trims.
- [x] **Saved Linear Resize keys after head or tail trim** (CONV-0049).
      Preserve three saved Position and Scale keys, including Fixed and Elastic
      keys beyond one visible edge when the source media contains them. Both
      trim rounds and the baseline retain editable keys after Premiere Save As
      and reopen. All 288 frames preserve visible geometry within one pixel of
      the Avid reference on the synthetic DNx route.
- [x] **Saved Linear animated crop on three-key trims** (CONV-0050, CONV-0056).
      Preserve four editable Motion crop curves on a shared crop clock.
      Same-rate progressive square-pixel 1080p23.976 only, with a neutral
      background and source media coverage for every key. Native Avid head and
      tail trims for Resize and the compact 18-control promoted Warp retained
      exact bounds and three Elastic keys after Premiere Save As and reopen.
      Across all 144 active trim frames, visible edges over black were within
      one pixel of the Avid export. The two untrimmed controls each had two
      two-pixel left-edge differences at the lowest luma threshold.
- [ ] **Extend animated Resize and 3D Warp.** Qualify other rates, interpolation,
      compact Fixed clocks and additional key shapes separately. Track anchor,
      rotation and perspective in the Avid codex.

- [x] **Single-shape AniMatte inversion and opacity in Premiere.** Key In
      and Key Out at full and half opacity survived Save As and reopen.
      The 72-frame synthetic comparison verified inside and outside pixels.
- [ ] **Match partial AniMatte compositing.** The native Avid single-shape
      Key Out 50% control attenuated the inside and preserved the outside.
      Premiere's single inverted mask attenuated the outside. A two-mask
      probe corrected which region is attenuated, and matching the measured
      midpoint also required changing the sequence-wide blend domain.
      Keep the current editable approximation scoped until a local solution
      is verified. Feather, topology changes, multiple trackers and
      stabilization remain open.
- [ ] **Luma Key and Chroma Key.** Avid's Luma Key Gain change moved a sharp
      cutoff on a 16-step grayscale ramp. Premiere reproduced those two
      sampled splits using paired editable Cutoff and Threshold controls.
      Qualify the source formula, soft/colored cases and layer ownership
      before mapping. The installed Avid palette exposed RGBKeyer and
      SpectraMatte. The exact Chroma Key entry still needs a native fixture.
- [ ] **Opacity** (Avid opacity/levels, fade-to-black etc.) -> Premiere Opacity (and blend modes
      where Avid has an equivalent); keyframes.
- [x] **Ordinary two-sided video Dissolve** (CONV-0040). Same-rate cuts with
      qualified source handles become editable Premiere Cross Dissolve (Legacy)
      transitions at the original window and cut offset. A synthetic asymmetric
      control rendered in Premiere and kept both endpoint references on save.
- [ ] **Video transition fidelity and audio fade curves.** Compare ordinary
      Dissolve with a native Avid render and qualify additional video identities
      and control values. Other qualified video transitions retain the cut approximation. The native
      Avid reference now demonstrates a bounded mismatch with the current mapping;
      near-linear evidence is not an exact interpolation rule. Qualify a same-window
      Constant Gain comparison before considering a new default.
      Measurements and setup limit.
- [x] **Flip, Flop, and Flip-Flop** (CONV-0041). Exact Avid IDs map to editable
      Vertical Flip, Horizontal Flip, or both. All three reflected synthetic
      Premiere frames match the expected pixels. A native Avid Warp outside
      Flop control retained both effects and left the asymmetric crop on
      its original side after reflection of the source.
- [x] **Share picture reflection in graph/2.** Avid and Resolve emit one
      axis-based effect, including the Flop stage inside an ordered chain.
      The native Avid effect ID stays with the source receipt.
      [Representation and scope](SOURCE-GRAPH-MIGRATION.md#picture-reflection).
- [x] **Read simple Avid picture chains into graph/2** (CONV-0065).
      Preserve the input-to-output order for qualified full-span
      Flip, Flop, Resize, and 3D Warp stages. The retained Avid
      Flop-inside-Warp test keeps both effects and Warp's animated crop.
      [Source scope](SOURCE-GRAPH-MIGRATION.md#nested-avid-picture-effects).
- [x] **Write bounded ordered picture chains to Premiere and Resolve**
      (CONV-0066). Reflections can occur on either side of one supported
      Resize or 3D Warp stage. Native Save As and reopen verified Flop
      before animated Warp in both editors using the retained Avid control.
      [Scope and measurements](SOURCE-GRAPH-MIGRATION.md#writing-ordered-picture-chains).
- [ ] **Expand destination picture chains.** Qualify a construction that
      can apply multiple Motion stages or other effects in saved order.

- [ ] Avid Start shown on clips whose MXF timecode is 0 (Alternate Timecode, User decision).
- [x] **Editable two-camera Groups over native OP1a** (User, 2026-11-08).
      Graph/2 retains angle layouts, offsets, gaps and
      independent selectors. The writer converts continuous 1080p23.976 DNx camera
      sources with two fixed mono PCM24 lanes. Generated output passed scoped native
      insertion, angle switching, playback and picture/PCM checks. Explicit inner
      channel routing removes the measured 3 dB level loss.
      [Implementation and native checks](EDITABLE-GROUPS-2026-11-08.md).
- [ ] **Widen editable Group qualification.** User acceptance, independent audio
      selection, missing or wider audio, mixed rates, discontinuous MultiGroups
      and exact record-start clocks remain open.
      The saved fixed-audio control still matches its pixel and PCM references.
      A same-rate synthetic control now confirms a switched A1 clip can play
      camera B while editable picture and A2 stay on A. The A1 switch uses a
      direct audio edit in Premiere. Qualify a saved same-rate Avid selector
      and decide how to represent alternate audio inside a Premiere Group.
      Source contract and acceptance plan.

- [ ] Subclips as bin items (supported source references resolve inside sequences).

## More programs: the neutral graph (ADR-0004, User 2026-10-26)

- [x] Direct Avid source extraction into graph/2 and explicit Premiere preparation.
      Folder imports preserve destination bin hierarchy through the shared job path.
      Graph/1 and model/1 remain compatibility interfaces.
      [Current architecture](SOURCE-GRAPH-MIGRATION.md).
- [x] Preserve exact Avid effect parameter receipts and their placement links,
      including controls on events without a usable media route. Preserve
      Premiere caption content and timing as shared text facts, with per-cue
      Resolve omissions reported during destination review.
- [ ] Promote color correction and AniMatte to neutral effects when their
      cross-editor semantics are qualified. Track subclips and nests separately
      from the editable Group scope above.

### Adobe After Effects (User, 2026-11-09)

- [ ] **Translate to and from After Effects.** Plan both a source adapter and a
      destination writer through the neutral graph. Start with a native project
      containing one composition, linked footage and a trimmed layer. Establish
      layer timing, frame rate, transform and media identity before choosing the
      first editor pair to qualify. Both direction menus list After Effects as
      unavailable until a route is implemented and checked.

## DaVinci Resolve (User 2026-10-26: bookmark and research)

**Direction decided by the User (2026-10-27): Avid -> Resolve goes straight to Resolve's native
project (`.drp`), read and written by Any NLE, through the graph. FCPXML is a separate, optional
export, never the route to Resolve: Avid can already export interchange files itself, and the
point of Any NLE is to go further than they do.** Route (1) below is the target; route (2) is
only for comparing results; route (3) (scripting API) is for test read-back.

- [x] **Resolve format authority and shared effects audit.** Native archives and
      findings have a versioned catalog under the workspace Resolve knowledgebase.
      The [cross-editor audit](SHARED-EFFECTS-AUDIT.md) traces shared and source-specific
      semantics without changing effect behavior.
- [x] **Qualify the internal straight-cut writer.** A standalone graph/2 writer
      emits the bounded 1080p23.976 split edit with exact TC1, one linked
      DNxHR OP1a item, and two PCM24 mono lanes. Native reopen, re-export and
      render comparisons passed for the baseline, origin change and source trim.
      [Profile, evidence and limits](RESOLVE-PROJECT-CORE.md).
- [x] **Write the bounded editable Group.** The saved A/B/A sequence
      reopened in Resolve Studio 21.2 with exact cuts and both mono lanes
      fixed to camera A. A fresh insertion and distinct-signal tests passed.
      [Scope and audio-envelope comparison](RESOLVE-EDITABLE-GROUPS.md).
- [x] **Read the bounded Resolve project into graph/2.** The internal reader
      covers registered timelines, cached linked media, exact clocks and
      straight cuts, fixed-audio Groups, sequence markers, static X
      position, bounded static mono clip volume, two-point source-time volume
      curves and source receipts for default clip fades. [Source profile and limits](RESOLVE-SOURCE-READER.md).
- [x] **Write the bounded Resolve clip envelopes.** The internal writer
      recreates bounded static clip volume, default fades, and two-point
      linear-dB volume automation on qualified direct mono edits.
      [Writer profile and limits](RESOLVE-PROJECT-CORE.md).
- [x] **Read and write bounded static Resolve track faders.** The qualified
      two-mono mixer preserves track level independently of clip volume.
      Both lanes matched native PCM after Save As and reopen.
- [x] **Read and write the bounded Resolve main bus fader.** The qualified
      stereo bus preserves its level independently of track and clip gain.
      Both mono lanes matched native PCM after Save As and reopen.
- [x] **Read and write bounded Resolve track mute.** The qualified two-mono
      mixer and timeline flags agree after Save As and reopen. The muted
      lane is silent, the enabled lane matches its reference, and all gain
      settings are preserved. [Scope and evidence](RESOLVE-SOURCE-READER.md).
- [ ] **Qualify the linked Resolve project foundation.** The internal project
      writer handles ordered timeline selection, nested media bins and shared
      original MOV/WAV bindings. Fresh stereo PCM16 and PCM24 insertions
      matched both reopened reference lanes. Complete the authorized Save
      Project As checks in the [linked project profile](RESOLVE-PROJECT-CORE.md#pending-editor-checks).
- [ ] **Qualify the combined Resolve grading turnover.** The internal writer
      combines direct DNxHR LB OP-Atom, linked MOV/WAV and a fixed-audio Group
      in one nested project. Scripted reopen, re-export and picture-switch
      checks passed on Resolve Studio 21.2.0.0006. File > Save Project As
      remains pending. See the [combined profile](RESOLVE-PROJECT-CORE.md#combined-grading-turnover).
- [x] **Read and write bounded static Resolve picture controls.** Same-rate
      1080p position, independent scale, and four crop edges matched eight
      reopened native controls. See the [static profile](RESOLVE-PROJECT-CORE.md#static-picture-controls).
- [x] **Translate isolated Flip, Flop, and Flip-Flop in the linked Resolve
      writer.** The 1080p24 Avid UME control preserved all three editable
      axis choices after Save Project As and reopen. All 144 decoded frames
      matched a native Resolve control. Picture effect stacks need separate
      qualification. [Scope and evidence](RESOLVE-PROJECT-CORE.md#flip-flop-and-flip-flop).
- [x] **Translate two-key Linear Resize and 3D Warp to Resolve.** The
      retained 48-frame Avid pair keeps editable position and independent
      zoom keys after Save Project As and reopen. All decoded Resize and
      Warp frames match each other. Sampled chart geometry agrees with
      Avid within two pixels horizontally and one vertically.
      [Scope and evidence](RESOLVE-PROJECT-CORE.md#linear-animated-resize-and-3d-warp).
- [x] **Translate three-key Fixed and Elastic Resize and 3D Warp to
      Resolve.** Four editable curves retain their saved clocks, including
      keys outside the visible edit with verified source media handles.
      Save Project As and reopen preserved all 468 decoded frames across
      the 14 retained cases. [Scope and evidence](RESOLVE-PROJECT-CORE.md#three-key-linear-resize-and-3d-warp-after-trimming).
- [x] **Translate three-key Elastic Resize and 3D Warp crop to Resolve**
      (CONV-0064). Four editable source-space crop curves run before Position
      and Zoom. Reopened control and trim renders preserve the edit ranges,
      keys, and edges within one pixel of Avid at the primary threshold.
      [Scope and evidence](RESOLVE-PROJECT-CORE.md#three-key-linear-resize-and-3d-warp-with-animated-crop).
- [ ] **Extend Resolve crop beyond the shared Elastic grid.** Qualify
      Fixed Width or separate crop clocks and mixed effect stacks with
      distinct timing and order controls.
- [x] **Translate the bounded Avid UME profile internally.** The Avid
      adapter resolves and verifies originals through graph/2. Resolve
      links qualified ProRes HQ, PCM WAV and 1080p24 AVC in MOV and MP4.
      The AVC audio profile preserves two independent mono selections
      from embedded stereo AAC. See the [UME profile](RESOLVE-PROJECT-CORE.md#avid-ume-originals).
- [ ] **Widen Avid UME turnover coverage.** Qualify representative production
      bins, other H.264 profiles and rates, and combined automatic binding
      of managed Avid media and UME originals.
- [ ] **Qualify an asymmetric Resolve video dissolve.** The native
      left-aligned witness saved eight frames from a twelve-frame request
      with eight incoming handle frames. Qualify graph/2 endpoint and trim
      projection from a saved Avid control before writing a dissolve.
- [x] **Implement the internal link-in-place turnovers.** One preparation and
      publication path handles automatic Avid bindings and the bounded Premiere
      ProRes HQ MOV/PCM WAV source. The synthetic Premiere control matched the
      fresh native Resolve edit in picture and isolated PCM.
      [Inputs and scope](RESOLVE-TURNOVER-IMPLEMENTATION.md).
- [x] **Capture additional Resolve source fields in graph/2.** Retain
      transitions, timing maps, Fairlight controls, custom fade points,
      timeline marks, and clip color codes with their saved owners.
      [Capture status](RESOLVE-TURNOVER-IMPLEMENTATION.md#capture-in-graph2).
- [ ] **Qualify a native Premiere source turnover.** Author the same-rate
      ProRes HQ MOV and PCM WAV project natively in Premiere, including exact
      source selectors and nonzero trims. Run the package and product route gates.
      Use the [sequence inventory](RESOLVE-TURNOVER-IMPLEMENTATION.md#inputs-and-review)
      to select the saved timeline by ObjectUID and check its rate, canvas,
      start ticks and active track counts before checking the linked media.
- [ ] **Qualify editable 24 ↔ 23.976 adaptation in Resolve.** Native default
      picture cadence differs from the retained Avid phase controls. Author a
      one-change editable timing control per direction, then verify the full
      numbered-frame sequence and the independent 48 kHz first sample and tail.
      See [pending editor checks](RESOLVE-PROJECT-CORE.md#pending-editor-checks).
- [x] **Compare direct Avid OP-Atom playback in Resolve.** Resolve Studio
      21.2.0.0006 reopened and played the tested DNxHR LB with two mono PCM24
      siblings and the picture-only ProRes LT fixture. The DNx pictures and both
      isolated PCM lanes matched the OP1a control. The ProRes render covered all
      446 pictures with small pixel differences from the MOV control. See
      RESOLVE-0014 in the local Resolve knowledgebase.
- [x] **Qualify the internal direct DNxHR LB OP-Atom writer.** One graph/2
      split edit links the original picture and two verified mono PCM24
      siblings. Resolve Studio 21.2.0.0006 preserved both routes and all nine
      edits after Save Project As and reopen. See the [direct-link profile](RESOLVE-PROJECT-CORE.md#direct-avid-op-atom).
- [x] **Read the qualified direct DNxHR LB OP-Atom Resolve archive.** The
      internal reader preserves both mono cache locators, selectors and
      package identities from the reopened project. See the
      [reader profile](RESOLVE-SOURCE-READER.md).
- [x] **Bind direct DNxHR LB OP-Atom automatically from an Avid bin.** The
      internal Avid probe resolves native media from supplied roots and links
      both mono siblings by saved track and file package. The split edit
      retained both assignments after Save Project As and reopen.
- [x] **Bind multiple direct DNxHR LB OP-Atom clips in one timeline.** The
      internal Avid probe resolves each clip's own picture and mono siblings.
      The two-clip test retained all six paths and all nine edits after Resolve
      Save Project As and reopen. Separate signal renders confirmed each mono
      route. See the [direct-link profile](RESOLVE-PROJECT-CORE.md#direct-avid-op-atom).
- [ ] **Extend direct Avid OP-Atom beyond the core profile.** Qualify more
      media and timeline layouts before product exposure.
- [ ] **Qualify older Resolve archives as source inputs.** A private version 15
      production archive supplies a hierarchy, identity and locator comparison.
      Capture a bounded version 15 structural and render cohort before admitting
      that format. Version 17 remains the current writer target.
- [ ] **Widen Resolve source and destination evidence.** Qualify other media
      and versions, mixed-rate clocks, custom fade curves, portable envelope
      rendering, other transforms, and independent Group audio before extending support.
- [ ] **Qualify product exposure separately.** Resolve remains unavailable in
      the CLI and wizard. Expand media, project structure and version coverage
      before enabling a route.

Premiere → Resolve and Avid → Resolve are planned primarily for grading
turnovers. The preferred media menu default is **Link to originals**. Preserve
saved camera-original locators, source timecode and reel or file identity when
qualified, and diagnose unresolved relinks. Offer rewrap as a separate choice
for a verified workflow that needs it, with the **Creates new media.** notice.

### Pinned offline turnover references

- [drp-format](https://github.com/samuelgursky/davinci-resolve-mcp/tree/1e2bf0342ad1eee4efc69b4d2d1e89e8dc493048/resolve-advanced/vendor/drp-format)
  (JavaScript, MIT, `1e2bf0342ad1eee4efc69b4d2d1e89e8dc493048`).
  The offline package in davinci-resolve-mcp covers DRP/DRT packaging, timeline
  edits, media relinking, markers, transitions and grades. Use it for format
  comparisons. Its new-media path uses an H.264 template, and our DNxHR OP1a
  and editable Group cases need their own native fixtures.
- [pr2resolve](https://github.com/Ersiter/pr2resolve/tree/61c8c9ec2a76496721718130f250c5b64fccd789)
  (Python, MIT, `61c8c9ec2a76496721718130f250c5b64fccd789`).
  Use its [native format notes](https://github.com/Ersiter/pr2resolve/blob/61c8c9ec2a76496721718130f250c5b64fccd789/NATIVE_FORMAT.md)
  to cross-check timeline registration, source trims, media bindings and Fairlight
  source-channel selection. Its controlled PCM16 tests leave our DNxHR OP1a
  PCM24 dual-stream and editable Group cases to qualify.
- [drpkit](https://github.com/higuchiki/drpkit/tree/af3cedc0f40ae2192929158399667b4d28863dd6)
  (Python, MIT, `af3cedc0f40ae2192929158399667b4d28863dd6`).
  Use its [field inventory](https://github.com/higuchiki/drpkit/blob/af3cedc0f40ae2192929158399667b4d28863dd6/FORMAT.md)
  for Fairlight mixer and edit read-back, including routing, EQ, dynamics,
  fades and keyframes. Its template composer is audio-focused.
- [PRPROJ-READER](https://github.com/sergeiventurinov/PRPROJ-READER/tree/a51a2a5b1095b8bdead114c812e85132a5eb26e0)
  (Python, CC0-1.0, Premiere research source SRC-011).
  Use its object-reference map and Premiere 14.3.2 fixture to compare saved
  sequence, bin, clip, and keyframe structures. The fixture is a legacy
  parser control. Current media binding and audio routing require modern
  Premiere saves with verified originals.

Review licence and provenance before code adoption.

For broader integration, use the Resolve knowledgebase comparison matrix
and integration sequence. Start by widening archive, media and clock
coverage, then expand editable timeline controls and Fairlight on verified
bindings. Titles, Color and Fusion need distinct rendered controls.

- **Bookmark: storytold/filmcraft PR #670** "Interchange: import DaVinci
  Resolve projects (.drp)" (by nikpottbecker; fork branch `feat/davinci-resolve-drp-import`,
  `crates/interchange/src/drp/` ~1,900 lines Rust). States clean-room: layout read off Resolve
  19/20 exports, no Blackmagic software or other code cited. Facts it records: a `.drp` is a ZIP of
  XML documents mirroring Resolve's project database (`project.xml`, `MediaPool/**/MpFolder.xml`,
  `SeqContainer/<id>.xml`; classes `Sm2MpFolder`, `Sm2Timeline`, `Sm2Sequence`, `Sm2TiTrack`,
  `Sm2TiVideoClip`, ...); binary fields hex (protobuf / Zstandard inside); `Start` = timeline frame
  from the timeline start TC, `In` = source offset in TIMELINE frames (fractional for other-rate
  media, `64|<double>`). Imports bins, clips, timelines, tracks, Text/Text+ titles, Transform,
  Volume, fades, transitions, compound and multicam (flattened); reports grades, Fusion, retiming,
  keyframes, markers. Filmcraft is production code: read it as a lead only, never depend on it.
- **Other open-source readers found (GitHub code search on the class names):**
  | Project | Language / licence | Scope |
  |---|---|---|
  | hfiguiere/drp-extractor | Rust, MPL-2.0 | parse .drp (2018-2023) |
  | samuelgursky/davinci-resolve-mcp | Python/JS, MIT, ~3.8k stars | Resolve scripting API + offline .drp/DB tools (`drp-timeline-clips.mjs`, `project_db.mjs`) |
  | CiprianSpiridon/davinci-resolve-mcp | Python, MIT | `formats/drp.py`, `formats/drt.py` (.drt timelines) |
  | koosoli/DaVinci-Resolve-DRP-Toolkit | Python, GPL-3.0 | analyse/compare/downgrade .drp (read only for us) |
  | donutinit/sqsolve | Go, AGPL-3.0 | project-library server, `internal/drp/export.go` (read only for us) |
  | LucaL1fe/davinci-project-repair | JS, MIT | repairs a corrupted UI-state blob in the project DB |
- **Routes to evaluate:** (1) native `.drp` read/write (most fidelity, undocumented, version
  drift 19 -> 20 -> 21); (2) Resolve's own interchange imports (FCPXML, FCP7 XML, AAF, EDL, OTIO):
  FCPXML is an optional export, and further FCPXML work is on hold; these imports may be used
  for comparisons only; (3) Resolve's scripting API (Studio; the free
  edition via the in-app bridge on 21.0.x) for automated read-back in tests, like the Avid API
  item below; samuelgursky/davinci-resolve-mcp is that route.

## App

- [x] **Avid to Premiere wizard** (User design, 2026-10-15). The five pages cover
      project and dependency selection, clips and media, markers and colors,
      effects, and review and convert. Native rewrap is first and the default,
      followed by Grouped, Merged and Separate. The app remembers choices, supports
      presets and job replay, and shows space estimates and **“Creates new media.”**
      before execution. The completion page offers the output folder, searchable
      report and Open in Premiere when installed. [Current wizard](GUI-WIZARD.md).
- [x] **Run the published Apple Silicon package through Premiere.** The wizard and
      bundled backend converted a small 23.976 Avid split-edit bin. Both sequences
      opened online and played to the end in Premiere 26.5.2. Sampled picture and
      rendered PCM matched the retained reference after save and reopen, including
      separate checks of both mono lanes. This release predates the TC1 correction below.
- [ ] **Next platform check: Windows x64.** Install the published package, run the
      same bin through the wizard and repeat the Premiere checks. Keep the remaining
      platform checks open until they have their own evidence.
- [x] **Correct the qualified 23.976 non-drop sequence start in the converter.**
      For a unique, sequence-spanning primary TC1, write its count at the exact
      record rate. The split-edit bin now writes the `01:00:00:00` origin
      (`915372057600000` ticks); relative edit and transition ranges are unchanged.
      Unknown or unsupported sequence clocks retain the compatibility fallback.
- [ ] **Read back the corrected zero point in Premiere.** Check both split-edit
      sequences at the first frame and after save/reopen. The old release's native
      read-back displayed `00:59:56:09`; it does not qualify the corrected output.
- [ ] **Extend the wizard for new directions and destination versions.** Reuse the
      direction-specific flow when another route is implemented. Offer a target
      Premiere version choice when the output differs by version. Proxy and
      additional original-audio linking remain under Media linking.

## On hold (User, 2026-10-28)

### Further FCPXML work

- FCPXML read-back in Resolve (User, or Resolve's scripting API in a throwaway project), then
      the losses in docs/FEATURE-MATRIX.md one by one: pan (`adjust-panner`), volume keys,
      transform (`adjust-transform`/`adjust-crop`, units to confirm), speed (`timeMap`),
      crossfades, drop-frame, stereo as one 2-channel clip.

### Premiere 27 Color mode and editable Lumetri controls

- **Premiere 27 Color mode** (User 2026-10-15; Premiere 27 beta installed): document how the
      new color effects (per clip, group and sequence operations; Color Controls; Color
      Management panel) are stored in a project, from User-made beta fixtures. Adobe: the
      Lumetri panel is retired in 27 but the Lumetri Color effect stays and old projects open
      unchanged; beta projects "may impact project formats" and show offline filters in 26.x
      (helpx "Color mode basics", 2026-08-24). Converter targets Lumetri Color until 27 ships
      and its format settles; then offer Color mode via the wizard's target-version choice.

- Option: editable Lumetri sliders/curves instead of a LUT (approximate; earlier CONV-0025 route).

### Additional surround work

- The User resumed declared mono/stereo/5.1/7.1 rewrap grouping on 2026-11-07;
  see the active multichannel item above. Further surround mixing, speaker/MCA
  semantics and output formats beyond that bounded slice remain held.

### Avid local API test automation

- **Avid local API as a read-back oracle (test tool only, never a product dependency).**
      Media Composer embeds an undocumented local gRPC service `mcapi.MCAPI` (third-party
      research: github.com/leancoderkavy/avid-media-composer-mcp, MIT, docs/NATIVE_API_RESEARCH.md;
      49 methods on port 9100 in the Windows 2024.12 report). The User's Mac build 2025.12.0.58863
      contains the same `MCAPI.proto` / `MCAPI_Types.proto` descriptors (GetMarkers, ExportFile, ...).
      Plan: confirm the listener with Avid open; read-only probe (reflection, GetAppInfo, GetBins,
      GetMobInfo, GetMobTrackInfo, GetMarkers, GetBinColumnInfo) on a disposable copy of an existing
      test bin; compare with avid-reader's model. Later: ImportFile/ExportFile for the
      Premiere -> Avid direction. Unsupported API; behaviour varies by version.

## Completed capabilities

Retained here with their evidence and limitations; these are no longer active to-dos.

### Explaining decisions (User, 2026-10-14)

- [x] Started `docs/DECISIONS-EXPLAINED.md`: plain-language reasoning for every import option
      and automatic decision, with finding/decision IDs.

### Bin layout: builds and parts (User, 2026-10-14)

- [x] Top level of each converted bin = builds (grouped/merged clips, stereo audio clips,
      unpaired video, mono audio); the parts (source V/A items) go into a per-bin
      **Processed Clips** subfolder.

### Audio

- [x] Stereo pairs (`_CHANNEL_GROUP_LIST`) and stereo edits on stereo tracks, grouped mode
      (PRPROJ-0019, User-confirmed).

- [x] Stereo in Separate mode uses an audio-only stereo nest built from the A01/A02
      pair. It places one stereo item on the Avid track number with picture linked
      when present. The nest superseded the earlier zero-video multicam
      (PRPROJ-0021/0022).

- [x] Stereo in Merged mode: a stereo pair is built as a group instead; reported (User).

- [x] Clip pan storage and value scale (PRPROJ-0021, PRPROJ-0023).

- [x] Master-clip gain from PVOL (PRPROJ-0020, User-confirmed).

- [x] Edit pan and gain from sequence PVOLs; Avid default panning option (PRPROJ-0023, CONV-0022).

### Clip, marker and correction color

- [x] **Color correction** (CONV-0026, User-confirmed render match): Avid HSL Controls + Curves tab ->
      one embedded Lumetri Look LUT computed from the Phase 52 Avid model; .cube beside the project.

- [x] **Markers** (PRPROJ-0025, CONV-0024, User-confirmed except merged mode): Avid marker color *name* (`_ATN_CRM_COLOR_EXTENDED`, 16 names; the stored RGB
      is only the legacy 8-color value) -> Premiere's 8 marker colors, defaults per
      DECISIONS-EXPLAINED; name (`_ATN_CRM_MARKNAME`), comment (`_ATN_CRM_COM`), spanned
      length (`_ATN_CRM_LENGTH`); option to append Avid user + color name to the comment.
      Placement: master/subclip markers -> bin clip markers; sequence copies of them skipped;
      markers on video filler -> one-frame adjustment layer; markers on audio filler -> sequence
      markers. Markers added on a timeline clip in Avid -> markers on that edit in Premiere
      (a per-edit subclip carrying the source markers plus the edit's own), the edit's marker
      replacing a source marker at the same frame. Import options: source marker wins instead;
      put the losing marker on the sequence instead of dropping it (User 2026-10-15).

- [x] Avid clip color (User-confirmed labels) (read: `_COLOR_R/G/B`, KB BCOL-019) -> nearest of Premiere's 16 label
      colors (`Column.PropertyText.Label` = `BE.Prefs.LabelColors.N`, plus the item's
      `asl.clip.label.*`), using Premiere's default label RGBs; custom Avid colors that only
      approximate a label are listed in the report. One generate-then-confirm round.

### Timeline and effects

- [x] Constant speed and reverse (PRPROJ-0024, User-confirmed: 50 %, 198 %, reverse).

- [x] **Motion, static** (CONV-0027): Avid Resize and 3D Warp position / scale / Z rotation /
      axis / crop -> Premiere Motion. Render-compared 2026-10-20: within 1 px in Premiere's H.264
      export; its TIFF export keeps ~3 px more where crop and scale combine (render path).

- [x] **AniMatte baseline**: reader and Premiere Opacity-mask writer built; default Key In,
      1920x1080, fixed topology and one tracked point. 46/46 frames matched Avid after the
      confirmed square-pixel interpretation (PRPROJ-0029 / CONV-0029). Other cases remain above.

- [x] **ProRes LT in Avid OP-Atom MXF**: reader checks both the Avid compression and MXF
      picture-coding identity; both Premiere writers have the observed `apcs` media profile.
      This does not cover all ProRes profiles or solve Premiere's MXF playback issue.

- [x] **Time-remapping writers** (2026-10-20): supplied frame maps reproduced
      192/192 Avid picture selections in the retained Premiere H.264 exports
      (PRPROJ-0028 / CONV-0028). The bounded source path is tracked above.

### More programs: the neutral graph (ADR-0004, User 2026-10-26)

- [x] **Graph-to-model placement validation** (2026-10-29): the bridge shares model/1's
      cross-object checks with the JSON loader (required video/audio streams, sequence record
      frame grid, source span within media). Invalid sequence rates and item-ID collisions
      during projection are refused. Valid existing models retain their output bytes.

- [x] **FCPXML writer from the graph** (2026-10-27, `crates/fcpxml`, `any-nle convert --to fcpxml`):
      DTD-valid on all 36 golden models and 3 real bins. Test files in
      `local/to-test/2026-10-27 fcpxml/`.

### App

- [x] **Port the Premiere writer (py_premiere + converter Premiere logic) to Rust** (done 2026-10-20,
      ADR-0003; Rust `prproj-write` byte-identical to Python, User read-back passed) (ADR-0001;
      started 2026-10-19 at the User's request). Plan: Phase 0 deterministic Python oracle + golden
      corpus (done, `scripts/make_golden.py`); 1 XML core; 2 builders; 3 graph operations;
      4 converter logic; 5 switch-over with a User read-back. New standalone crate `prproj/`; the
      Avid reader is not touched (Codex's Avid research continues there); the JSON model stays the
      boundary. Acceptance: byte-identical output to the Python writer on the golden corpus.

### Reliability (2026-10-29)

- [x] **Grow the ordinary AVB object table after accepted objects** (2026-10-31): avoid
      reserving the advertised full table before malformed chunks are checked. Grow with
      accepted decoded/unsupported objects, capped at the declared final slot count. Keep
      parser values, error order and ordinary conversion results. Actual large-bin decoded
      storage and broader project-memory work remain open. Structural/differential checks only.

- [x] **Decode straight into editable AVB records** (2026-10-31): remove the temporary
      extraction object table and raw-effect copies during document loading. Keep original
      chunks, decoded values, snapshots, errors and edit/copy/save behavior; ordinary
      extraction retains its decoded/raw data. This reduces loading peaks, while retained
      document data and broader project-memory work remain open. Structural/differential
      checks only.

- [x] **Release saved-bin validation objects individually** (2026-10-31): use the normal
      full-file decoder checks without retaining a second complete decoded bin or raw-effect
      payload copies. Original output bytes, refusal rules and ordinary extraction stay the
      same. One large object, the editable document and output buffer still need memory;
      broader project-memory work remains open. Structural/differential checks only.

- [x] **Share unchanged decoded AVB snapshots** (2026-10-31): original/editable states share
      read-only node data until mutable access detaches an object's working copy. Preserve
      value-based change detection, rollback, source-only checks and copy/save behavior.
      Large mutable objects and output validation still need memory; ordinary extraction
      and whole-project memory remain separate.

- [x] **Avoid duplicate preserved AVB chunk buffers** (2026-10-31): loaded documents read
      original chunks from their retained index; copied graphs keep independent owned bytes.
      Preserve source checks, unknown chunks, IDs, repeated saves and copy lifetimes in both
      byte orders. Decoded node snapshots and ordinary conversion/project memory remain.

- [x] **Lower edited AVB save memory** (2026-10-31): borrow preserved chunks while assembling
      output and validate the completed buffer without cloning the whole file. Release
      temporary index spans before decoded validation. Preserve bytes, repeated saves,
      unknown chunks, edit checks and size limits. Savings depend on payload structure;
      ordinary conversion/project memory remain separate.

- [x] **Lower editable AVB loading memory** (2026-10-31): reuse the original input buffer
      and transfer decoded objects into the document, while retaining original/editable
      states and exact unchanged chunks. Existing edit validation and opaque/trailing bytes
      are preserved. This is the editable-document API; ordinary extraction and whole-project
      memory remain separate. Measurements.

- [x] **Enforce AVB input byte limits during reads** (2026-10-31): decoded, indexed and editable
      readers share a bounded read of the same file handle. Refuse known oversize inputs
      before loading their bodies, and stop growth/underreported inputs at the existing 1 GiB
      limit. Valid bytes and conversion behavior stay unchanged; decoded/project memory
      remains a separate limit.

- [x] **Repeated bin selections**: both CLIs decode each canonical input path once, report
      skipped repeats, and retain first-selected names/order. Distinct files and normal input
      errors remain distinct; sequence inspection uses the same rule. Whole-project folder
      discovery and bounded-memory streaming remain separate active work.

- [x] **Repeated identities in one MXF**: count each package identity once per file when
      building the media index. Keep all distinct identities and preserve ambiguity between
      different file paths; raw header inspection and codec/timecode/duration rules are unchanged.

- [x] **Stereo transition spans**: trim all matching channels together before assembling
      stereo edits, coalesce identical track-transition records and reject unsafe trim bounds.
      Give generated stereo sources their actual content bound for writer handle checks. Keep
      existing Constant Power audio fades and reported video cuts; mono and partly missing
      stereo controls remain valid. No new transition shape or application read-back.

- [x] **Protect project staging files**: new and in-place saves exclusively allocate an unused
      temporary sibling, preserve occupied files/directories/symlinks, and clean up their own
      stage after write/rename failures when possible. Snapshots at the old `.tmp` name survive;
      Rust/Python project bytes and media paths stay unchanged.

- [x] **Protect and distinguish LUT exports**: keep existing `.cube` files, use numbered
      filenames for collisions (including simultaneous edits and sanitized sequence names),
      and link each edit to its chosen file. Refuse symbolic-link LUT folders. Rust/Python
      writer parity is required; colour math and editable Lumetri controls are unchanged.

- [x] **Protect graph snapshot outputs**: refuse existing entries (including symbolic links),
      missing output folders and colliding project/snapshot names before writing; create new
      snapshots exclusively. Valid conversion bytes stay unchanged. No application read-back.

- [x] **Overlapping media folders**: scan repeated folder selections and discovered file paths
      once, so selecting a parent and child folder cannot make the same MXF falsely ambiguous.
      Separate file copies remain ambiguous; explicit child selections retain their search depth.

- [x] **Partly missing stereo media**: remove incomplete bin pairs; keep the available timeline
      channel as mono with its original gain, pan and record position, reporting the fallback.
      Missing-media status follows the selected bin copy. Larger incomplete channel groups are
      omitted/reported; surround support stays on hold. Structural/differential checks only.

- [x] Skip/report a clip with no usable media instead of emitting an invalid no-media clip
      that refuses the project. Its unconverted edits are reported; other clips still convert.
- [x] Rust/Python writer fixes: Unicode sequence names with colour correction; reject duplicate
      curve x coordinates at validation; positional source-track fallback for audio edit markers;
      reject unsavable marker comments at validation; keep absolute Windows media paths instead
      of making them relative to a POSIX cwd. Structural/differential checks only.
