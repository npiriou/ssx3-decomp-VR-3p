# SSX 3 VR: renderer redesign after the first headset test

Status: implementation in progress, 2026-10-30. This supersedes the implementation order in
[`vr.md`](vr.md); its scope and original acceptance goals still apply.

## Measured implementation status (2026-10-30)

- The VR executable now uses OpenXR's recommended eye extent by default
  (1920 × 1920 on the local PICO), with no automatic descent to 512. It logs
  per-stage p50/p95/max timing windows. These new counters still need a live
  headset capture; CPU queue-wait time is not a per-eye GPU timestamp.
- Nash retains a headset-directed 75° cone of loaded patches and instances,
  plus nearby objects in every direction, before the game's destructive
  forward cull. Headless ARA1 captures at frames 500, 1000, 1500 confirm
  side/rear course objects. The all-direction 30,000-unit selection fell from
  47 to 15 headless frames/s on frames 400–500; the gaze-directed selection
  measured 38 frames/s at 90° and 35 frames/s at 180° on the same range.
  These are **not** a SteamVR image-rate measurement.
  The Nash VR tree scan now reads its RAM-resident nodes directly, with a
  generic-bus fallback for an unexpected root address. ARA1 frame 500 with
  a simulated 90° gaze produced byte-identical 32 MiB RAM dumps and PNGs
  against the prior bus-read path. Sampled scans took roughly 1.1–2.0 ms
  through the bus and 1.1–1.7 ms through the direct RAM path; whole-frame
  timing varied too much to attribute an FPS improvement to this change.
- A full-resolution bug was fixed: the GS flip packet is screen-space and
  must not receive 3D reprojection. A frozen 1920-square frame now has
  3,686,345 and 3,686,340 nonblack pixels in the left and right eyes,
  respectively. The images differ, and the HUD is absent from both.
- Original HUD draws are extracted to a transparent image and placed on a
  camera-anchored plane (default 1.3 × 1.3 m at 2 m). The offline race
  preview is readable in the central field. A menu frame without a 3D
  camera becomes a spatial card. The OpenXR compositor shader parses and
  validates. Headset comfort and menu interaction remain unverified.
- **G1 is still open.** At ARA1 frame 1500, ±90° views show native skybox
  seams and hard mountain-panel edges. The same defects occur with all course
  patches resident, so the Nash view cone is not their cause. The blue dome
  (native draw 0) leaves its lower band uncovered; the existing VR-only lower
  band closes the black gap but exposes the source dome's coarse polygons.
  A new opt-in `SSX3_VR_SKY_SPHERE=1` path replaces those four coarse VU
  meshes with a complete sphere sampled from the **original** 256×256 polar
  texture. A VR-only Mat3 shader mode uses linear UVs for that synthetic mesh:
  the original GS STQ formula divides by the old camera's `w`, which crosses
  zero on a complete sphere and caused the earlier zigzag seams. Offline
  ±90° captures show a continuous blue dome. The Snow Jam VR path initially
  mapped its six native 256×128 textures onto a camera-space shell. Inspection
  of the decoded assets showed that the first four contain only mountains,
  without any sky. Extending them around a tracked view made a huge wall of
  stretched snow in the upper sky. The VR path now omits those four panels,
  keeps the two panels with blue sky, and fills uncovered directions with the
  original polar dome. Frame-500 and frame-1500 forward/side captures with the
  four panels omitted retain the riders and course but lose the snow wall;
  the distant sky is plainer and still uses low-resolution original assets.
  The native sky's final draw varies with the scene (draw 7 exists at ARA1 frames 500
  and 1000, but not 1500); the replacement identifies the full contiguous
  sky draw prefix so that extra flat panel no longer cuts a triangle into a
  side view. Offline ARA1 captures at frames 500, 1000 and 1500 cover 0°,
  ±90° and 180°. At frame 1000, the previously visible triangle beside the
  Take 2 sign is gone. Direct and retained-scene +90° stereo PNGs match
  byte-for-byte, with different images in the two eyes. The remaining native
  backdrop is still soft in places; headset presentation is unverified.
  Snow Jam uses this path by default in
  VR (`SSX3_VR_SKY_PANORAMA=0` disables it). Metro City's different sky is
  matched by its own texture sequence and now uses the same native-sector
  reconstruction as Ruthless Ridge by default
  (`SSX3_VR_SKY_CITY_PANO=0` disables it). The frame-500 side view no longer
  stretches the city skyline into vertical bars. Offline captures at frames
  500, 1000 and 1500 cover forward, ±90° and rear. At later points buildings
  and terrain fill most side views; some close surfaces remain visibly coarse,
  and the blue dome's original cloud texture has a dark diagonal region.
  At frame 500, direct and retained-scene side-view stereo PNGs match exactly
  for both eyes; 952,910 of 1,048,576 pixels differ between the eyes.
  Ruthless Ridge's native sky has a large triangular slab at side angles.
  Its sphere-only variant removes the slab but also removes the distant
  mountain. The Snow Jam mapping discarded Ruthless Ridge's separate
  six-vertex peak strips and therefore lost the prominent front peak. A
  course-specific VR path now tessellates both the five lower panels and
  three peak strips from their native geometry and UVs, maps their original
  screen rays to a distant sphere, and fades panel edges into the blue dome.
  This path is on by default for the matched Ruthless Ridge sky sequence
  (`SSX3_VR_SKY_RIDGE_PANO=0` disables it). Offline captures at frames 500,
  1000 and 1500 cover forward, ±90° and rear views. The front mountain is
  retained and the huge side-view triangle is gone; a faint purple blend
  boundary remains during the frame-1500 camera roll. A unit test checks
  the upper strip's native screen sector, UVs and edge alpha. The earlier
  native-triangle wrap remains available for diagnosis when the panorama
  is disabled (`SSX3_VR_SKY_WRAP=1`). Other event variants and the
  low-resolution asset treatment remain open.
  Intimidator and Gravitude have now been surveyed through frame 1500 with
  90° VR visibility; both runs ended without a game halt or frame-memory
  miss. Their eight-panel sky sequences use the same mesh layout as Metro
  City, and exact texture-sequence guards enable the native-sector sphere
  reconstruction for each course. At Intimidator frame 1000, the +90° sky's
  large polygons and hard seams become a continuous cloudy dome; 0°, -90°
  and 180° captures retain course objects. Gravitude frame 1500 had a large
  angular sky patch at +90°. Its reconstruction removes that patch; keeping
  the native panel's upper edge opaque also preserves the forward mountain
  backdrop without a horizontal blend band. Both treatments are now the VR
  defaults (`SSX3_VR_SKY_INTIMIDATOR_PANO=0` and
  `SSX3_VR_SKY_GRAVITUDE_PANO=0` disable them). Close terrain and object
  textures remain visibly coarse, especially in enclosed side views. The
  texture sequences came from the frame dump's matrix/draw inspection
  (`SSX3_VR_INSPECT_MATRICES=1` now prints each sky draw's texture IDs).
- **G3 is still open.** A repeated frozen 1920-square stereo frame with UI
  took about 20 ms with the original bloom. Disabling that bloom reduced it
  to 13.89 ms; moving the UI pass to a separate 1024-square target reduced it
  to 13.36 ms. With the complete blue sky band, repeated measurements rose to
  15.45–16.19 ms. A 1600-square eye measured 13.11 ms and a 1280-square eye
  13.61 ms in adjacent runs, so resolution alone is not the dominant cost.
  Waiting for the GPU after each frozen frame measured 14.72 ms at 1920 and
  14.83 ms at 1600 in adjacent runs; this further points to repeated CPU
  preparation/submission rather than pixel fill as the immediate limit.
  An A/B pass test found little benefit from omitting the UI pass, while
  omitting the second eye saved roughly 2.6 ms. Keep both eyes and optimize
  their shared preparation; lowering UI refresh would not address the limit.
  Suppressing all remaining original post passes experimentally saved only
  about 1.4 ms; it was not adopted. These are renderer measurements on one frozen frame, before
  the live game, compositor and headset costs. The real game loop still
  serializes game ticks, renderer flush, OpenXR acquire/blit/release and
  end_frame. The 60 new stereo images/s target has not been met or measured
  on the headset with this build.
  The first shared-scene input pass now uploads GS vertices, VU call data,
  mesh indices, shadow uniforms, and static caster data once per game frame
  rather than three times for left eye, right eye, and UI. On the same frozen
  1920-square stereo replay with a GPU wait after every frame, adjacent runs
  measured 16.36 ms before and 14.24 ms after (frame extraction was 3.01 ms
  and 2.78 ms). The left, right, and UI PNG files have identical SHA-256
  hashes before and after. This is a local renderer improvement, not a live
  SteamVR frame-rate result; the remaining duplicated draw planning and
  serialized OpenXR loop are still open.
  The renderer also retains the latest published frame's texture pages and
  ordered GS batches. When OpenXR asks for an image between game ticks, it
  can draw two new tracked eyes from that scene without rerunning the
  simulation or stateful VIF/GS build; the HUD texture and game depth queries
  remain on the original tick. Offline direct and retained-scene renders at
  +90° have byte-identical left and right PNGs at both 1024² and 1920²;
  the eyes differ from one another. One isolated 1920² view-only refresh
  with a GPU wait measured 5.92 ms. The live loop now reports submitted
  OpenXR frames separately from fresh stereo images and times the refresh.
  Game ticks and XR submission still share one thread, so 60 fresh stereo
  images/s on hardware remains unproven.
  On one frozen ARA1 frame at 1920² with GPU waits and bloom off, an adjacent
  20-repeat comparison measured 15.30 ms/frame with the current lower-band
  dome and 14.69 ms/frame with the opt-in full sphere. This is only an offline
  renderer measurement; it does not establish headset frame pacing.
  With the sphere enabled, direct and retained-scene 1024² +90° stereo PNGs
  match byte-for-byte in both eyes; the eyes differ from each other. The
  isolated retained-scene refresh measured 5.02 ms in that run.
  With the Snow Jam 360° panorama, a separate adjacent 1920², bloom-off,
  GPU-waited 20-repeat run measured 17.11 ms/frame on the current path and
  17.60 ms/frame with the panorama. The latter remains above the 16.67 ms
  budget before live game and compositor costs. The 1024² retained-scene
  stereo refresh with this panorama measured 4.95 ms in one isolated run.
  VR now omits the game's final full-screen frame blend by default and
  presents each eye's sharp draw buffer; `SSX3_VR_MOTION_BLUR=1` restores
  the original. At ARA1 frame 1500 the original option produced byte-identical
  left/right/UI PNGs to the earlier renderer, while the sharp path retained
  two nonblank, different eyes. The menu UI PNG was unchanged. Direct and
  retained-scene sharp stereo side views matched byte-for-byte in both eyes.
  Three alternating 1920², bloom-off, GPU-waited 30-repeat pairs measured
  13.02–14.15 ms render time with the blend and 12.82–13.78 ms without it;
  this is too variable to claim a frame-rate gain. The change removes a
  repeated full-screen copy and improves edge clarity, but G3 remains open.
  A new `SSX3_VR_OFFLINE_STEREO=<yaw>` headless mode runs the native game,
  gaze-directed visibility and two 1920² eye renders together without
  OpenXR. On ARA1 frames 420–500 with 90° gaze and bloom off, this measured
  12.58 ms per game frame (p95 16.35 ms); the normal mono headless path with
  the same visibility and bloom setting measured 14.89 ms (p95 18.44 ms).
  On CRA3 frames 420–500, the full-resolution stereo producer measured
  10.56 ms per frame (p95 11.89 ms) with the course sky path enabled.
  After omitting Snow Jam's four mountain-only backdrop panels, a fresh ARA1
  frames-420–500 run at 1920² per eye and 90° gaze measured 11.69 ms/frame
  (p95 13.12 ms), with `halted: None` and no frame-memory misses. At ARA1
  frame 1500, direct and retained-scene 1024² side-view images matched
  byte-for-byte for both eyes; 901,406 pixels differed between eyes. A 1920²
  side-view stereo render had full nonblack coverage in both eyes and
  3,182,734 differing pixels. These results validate the offline renderer,
  not a headset frame rate.
  At 1024², Intimidator frame 1000 and Gravitude frame 1500 both produced
  full nonblack left/right images at +90°. Direct and retained-scene images
  matched byte-for-byte for each eye; 927,838 and 681,348 pixels differed
  between the eyes, respectively. Separate live-game, offline-stereo
  1920² runs at frames 420–500 measured 13.22 ms/frame (p95 16.68) on
  Intimidator and 12.82 ms/frame (p95 16.36) on Gravitude. Neither run
  halted or missed frame memory. OpenXR costs remain unmeasured.
  Their post-process paths differ, so this is evidence only that the stereo
  producer can run near 60 Hz offline. The OpenXR blit, compositor, headset
  pacing and queue synchronization are absent; the live target remains
  unverified.
  A trial of generating mipmaps for the original one-level textures did not
  materially improve the close-course appearance in 1920-square eye captures.
  In a 10-repeat frozen-frame comparison, ARA1 changed about 280,000 of
  3,686,400 pixels and its render time went from 14.16 to 16.25 ms; DRA4
  changed about 6,500 pixels and went from 14.44 to 13.92 ms. These timings
  are noisy and the visual gain was too small to justify the extra texture
  preparation, so the experiment was removed. The coarse close surfaces need
  separate geometry and source-texture inspection rather than a global mip
  switch.
  Gravitude's overhead chevrons are a separate close-view case: the original
  art is a translucent 64 × 64 texture, so the arrows look stair-stepped even
  when each eye is rendered at 1920 × 1920. A traced 512 × 512 version of
  that same two-chevron tile is now bundled with the VR executable. Offline
  stereo frame-1000 captures show smooth arrow outlines while both eyes keep
  full coverage. The 20-repeat frozen-frame A/B timings varied by several
  milliseconds between adjacent runs, so there is no reliable frame-rate
  claim from this change. Custom texture-pack files take precedence, and
  `SSX3_VR_TEXTURES=0` disables the built-in replacement. Other close objects
  still use their native textures.

Keep headset review behind the remaining G1 coverage and G3 pacing work.
All game captures in this investigation used `SSX3_AUDIO=0`.

## Product contract

- Keep the native Rust SSX 3 game: one simulation, the existing rider, chase
  camera, course, controls, timing, menus, audio and saves.
- The rider remains in view as the chase camera follows it. Head rotation
  changes the view **inside the course** independently of rider steering.
  Left and right eyes see the same world state with their own tracked poses
  and OpenXR projections. There is no full-game-screen panel posing as VR.
- The first target is Windows, SteamVR/OpenXR, gamepad, 60 game ticks/s, at
  least 60 new stereo images/s, with rendering paced to the active headset.
- Existing desktop output and race replay RAM must remain valid.

## What the prototype established

1. The OpenXR/Vulkan/wgpu integration works with the local PICO and SteamVR.
   True stereo and head orientation exist; the earlier gray right eye was a
   draw-list ownership bug, now fixed in the working tree.
2. `ssx3-gfx::Renderer::render_one` runs `Build::run` once and reprojects its
   already-built draw list for two eyes. That list was selected and clipped
   for the original forward camera. Side and rear head turns therefore lack
   geometry; changing eye FOV or post-process UVs cannot restore it. The
   game's Nash classification and both native world draw paths need an
   upstream VR visibility solution.
3. The HUD is still screen-space data baked into each full-eye image. Its
   screen corners end up in the headset periphery, outside a comfortable
   glance. It needs an independently placed layer.
4. The VR loop starts at a 768-square internal eye target, then automatically
   steps to 640 and 512 when image delivery is slow. The local OpenXR view
   recommendation was 1920 square. Enlarging 512 to 1920 is 3.75x in each
   dimension, explaining much of the reported pixelation. In the active
   headset log, the image rate remained poor after the reduction. Resolution
   is therefore not an established primary bottleneck.
5. The game can advance near 60 ticks/s while new OpenXR images arrive far
   less often. The current loop serializes `run_frame`, `gfx.flush()`, eye
   blit, and `end_frame`; the flush was added because the wgpu worker and
   OpenXR touch the same Vulkan queue. Current aggregate timers do not
   isolate CPU build, GPU work, queue waits, swapchain wait, compositor, and
   headset throttling. Do not assign the slowdown to one stage yet.

The previous plan already called for pre-culling eye transforms, separate
HUD, and per-stage timing. The mistake was accepting a post-culling stereo
prototype as if those requirements had been met. This design makes each a
gate before another headset review.

## Target architecture

```text
Native game, fixed 60 Hz
  ├─ gameplay camera / rider state / input / audio / saves
  └─ frame publication: immutable world state + UI commands + resource updates
       ├─ ordered resource/state preparation (textures, VIF/GS carry, animation)
       ├─ VR-visible world set (head-turn envelope before destructive culling)
       └─ UI-only layer
              ↓
OpenXR frame loop, at runtime cadence
  ├─ choose prepared world state, interpolate visual chase/rider transforms
  ├─ sample predicted left/right eye poses as late as practical
  ├─ render complete world per eye with each runtime projection
  ├─ composite separate spatial UI for each eye
  └─ submit both eyes, with explicit Vulkan queue synchronization
```

### 1. Preserve game state; change the rendering boundary

The game remains the sole owner of gameplay and its 60 Hz state changes.
Replace the current `Built -> forward-camera GS batches -> reproject` VR
boundary with a representation that retains world positions and all geometry
needed by the tracked views. The native renderer and asset decoding remain
the source; this is a renderer adaptation, not a new game engine. Classify
published work as world, sky/effects, UI, and ordered resource updates.

The frame stream is stateful. VIF/GS carry, texture uploads, and other
resource changes must be consumed in order even when the headset renders the
latest of two prepared snapshots. A simple `latest frame wins` channel may
lose state and is not acceptable without proving the state transition is
complete. Double or triple buffering applies to immutable **prepared**
scene data, with explicit ownership and GPU completion fences.

### 2. Make visibility valid for head motion

Start with the existing hand-written `Nash_ClassifyPatch` and
`Nash_ClassifyInstance`, and trace subsequent clipping in terrain, world,
objects, rider, effects, and sky paths. Determine the earliest point where a
VR union of eye frusta plus a bounded head-turn envelope can replace the
single forward camera's visual cull **without changing gameplay camera or
simulation state**. A huge generic FOV is not the design: it already failed
to fill the side/back view and could waste draw time. Use coarse course-cell
visibility, stable conservative bounds, and per-eye fine culling. Disable
single-view occlusion decisions only where they remove geometry visible to
an eye, then measure the cost.

If the published native frame has already discarded required objects, move
the VR selection upstream of that discard. Do not fabricate side scenery by
stretching screen triangles. A geometry-capture/cache path is acceptable
only if it reuses native assets, animations, materials, and event order and
passes the same coverage and parity gates. Two visual traversal passes from
one simulation tick are a fallback to evaluate, not an assumption that the
original render traversal has no side effects.

### 3. Give the HUD its own spatial contract

Extract original HUD/menu primitives and textures into an alpha UI target,
updated when the game changes. Present race information on a modest central
quad or a few anchored elements around 1.5–2 m apparent distance; expose
size, height, and follow mode as comfort controls. Use a stable rider/camera
anchor for information that should not swing with every small head motion,
and a separate central menu panel for menus and pause. Keep full-screen
effects and subtitles explicit: decide whether each is a world effect,
headset overlay, or spatial card. Avoid submitting the whole game image as
one panel. Verify the complete original HUD remains readable when looking
forward and can be found naturally during a head turn.

### 4. Separate game cadence from display cadence

Maintain 60 deterministic game ticks/s. Produce an XR stereo view each
display opportunity from the most recent prepared state; interpolate visual
chase/rider motion between the two latest states where safe. Sample head pose
close to the eye draw, so a fresh headset view does not require an extra game
tick. On the local 72 Hz headset, target 72 timely stereo submissions/s;
the project minimum remains 60 new stereo images/s. Track repeated game
snapshots separately from repeated eye images.

Measure the full critical path before changing synchronization: game tick,
frame extraction, state preparation, left/right CPU recording, GPU per eye,
queue wait, swapchain acquire/release, OpenXR wait/end, and compositor frame
rate, with p50/p95/max and dropped/reprojected counts. The current forced
`gfx.flush()` may erase useful overlap, but removing it without a queue lock
or correct fences risks device loss. Resolve ownership and submission order
first. Use Re-RAC's pipelined approach and per-stage timing as a model, not a
drop-in Bevy dependency.

The offline renderer currently prepares and submits the same GS batch list
three times (left eye, right eye, spatial UI), including repeated bind groups
and texture snapshots. The next renderer cut is a prepared draw plan shared by
both eyes and a separate compact UI plan. The XR thread then needs ownership
of immutable prepared scene snapshots so it can draw fresh head poses at the
headset cadence even when the game has no new 60 Hz tick. Replaying `Build::run`
to get another eye image would repeat stateful VIF/GS effects and is not a
safe snapshot path.

### 5. Recover image quality deliberately

Start measurements at the OpenXR recommended 1920-square per eye and profile
there. Keep a clear quality floor and report the **internal** resolution in
logs and UI; never silently collapse to 512. Lower resolution only after GPU
timings prove pixel cost is the limiting stage and the resulting headset
image remains acceptable. Investigate appropriate texture filtering, depth,
transparency, and anti-aliasing in the native renderer after full scene
coverage; original PS2 texture detail will remain an asset constraint.
Render UI separately at a resolution that keeps text sharp.

## Implementation gates

| Gate | Deliverable | Evidence required before the next gate |
| --- | --- | --- |
| G0 — baseline | Reproducible ARA1 run and timing capture at fixed 768 and 1920 eye sizes, with no automatic resize. | Both-eye nonblank checks; per-stage CPU/GPU/XR p50/p95/max; actual submitted, reprojected, and game rates. Headset test audio muted. |
| G1 — world coverage | Visibility and world-geometry path that survives head rotation, including both native draw paths. | Offline left/right captures at 0°, ±90°, 180° on near/mid/far course points; no missing terrain/objects or stretched triangles; rider visible when looking forward and chase anchor stable when looking elsewhere. One short headset check only after offline passes. |
| G2 — spatial UI | Race HUD, menus, prompts and full-screen effects separated and placed appropriately. | All HUD elements legible without zoom/cropping at forward view; menus usable; no UI duplicated inside the world eye texture. |
| G3 — frame pacing | Prepared scene buffering, safe queue ownership, and XR-cadence head tracking. | 60 game ticks/s and >=60 new stereo images/s sustained on target PC; aim for the local 72 Hz period (13.89 ms) without forced half-rate; recorded p95 and misses; no Vulkan device loss. |
| G4 — quality and regressions | Native-resolution tuning and representative course tests. | Image judged acceptable in headset, internal resolution disclosed, both eyes correct, side/rear coverage and HUD retained, desktop/race replay RAM gates pass. |

Do not ask the user to wear the headset for an intermediate build that has
not passed its corresponding offline gate. Tests and game runs stay muted
(`SSX3_AUDIO=0`). Do not add expensive validation runs without a concrete
remaining risk; keep the browser smoke test last under `AGENTS.md`.

## First engineering investigation

Map exactly where the game's visual classification discards course patches,
instances and effects relative to `FrameBuilder` and `Build::run`. On one
frozen race state, compare forward, side and rear candidate sets **before**
and **after** that boundary. This determines whether the smallest sound
change is a VR-aware native visibility traversal or a retained scene cache.
In parallel with that read-only investigation, capture stage timings at a
fixed resolution. Those two measurements decide the renderer implementation
and the performance work; neither requires another headset session.
