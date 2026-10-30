# SSX 3 VR: renderer redesign after the first headset test

Status: implementation in progress, 2026-10-30. This supersedes the implementation order in
[`vr.md`](vr.md); its scope and original acceptance goals still apply.

## Measured implementation status (2026-10-30)

- Menu frames now publish that they have no world depth. The game's
  frame-indexed depth query previously waited 100 ms for every VR menu
  frame, even though its UI render took under 1 ms. A muted 1920-square
  offline menu run now reaches 60.0 game ticks/s through frame 500, versus
  12.3/s before the fix. Its centered menu card is nonblank in both eyes;
  no perspective world image is fabricated behind it. In a muted Gravitude
  run at the same eye size, frames 420–501 measured 60.3 game ticks/s,
  71.4 stereo submissions/s and 59.5 fresh stereo images/s. A simulated
  90° turn kept the rendered eye pose aligned with the requested pose and
  the final eyes distinct. These are offline results; SteamVR headset
  pacing and comfort still need validation.
- The VR executable now uses OpenXR's recommended eye extent by default
  (1920 × 1920 on the local PICO), with no automatic descent to 512. It logs
  per-stage p50/p95/max timing windows. These new counters still need a live
  headset capture; CPU queue-wait time is not a per-eye GPU timestamp.
- The game now runs on its own 60 Hz thread while the OpenXR thread presents
  the latest complete stereo image. A queue lock protects shared Vulkan
  submissions and OpenXR acquire/release/end; the renderer can prepare a new
  game frame while OpenXR presents the previous one. A generation counter
  distinguishes newly rendered eyes from repeated submissions. The view-only
  path redraws the retained scene when the tracked eyes move and no newer
  game image is ready. `ssx3-vr --offline-pipeline 500` exercises this
  scheduling without starting SteamVR and always mutes game audio.
  The offline presenter now runs the same two-eye world/HUD composition shader
  into a synthetic 1920 × 1920 × 2 target and waits for its GPU submission.
  With a simulated 90° head turn, Gravitude frames 421–501 measured 59.4
  game ticks/s, 71.3 stereo presentations/s and 59.4 fresh stereo images/s;
  Snow Jam frames 420–502 measured 59.3, 70.2 and 58.6/s respectively.
  After the game stopped, another 5° turn redrew the retained scene. The
  composited Gravitude eyes had 3,685,629 and 3,685,620 nonblack pixels out
  of 3,686,400, with 3,168,165 differing pixels. Snow Jam also retained
  two filled, distinct eyes. A longer Gravitude run through frame 1001
  measured 60.0 ticks/s, 71.9 presentations/s and 59.9 fresh stereo
  images/s over race frames 420–1001; after a 5° retained-scene refresh,
  both final compositor layers still had over 3.68 million nonblack pixels.
  OpenXR swapchain waits, frame submission and
  the headset compositor are absent, so headset pacing, comfort and the 60
  fresh images/s target remain unverified.
- Each published stereo image now retains the eye transforms actually used
  to draw it. The final HUD composition uses those transforms, and the
  OpenXR projection layer submits their matching `xrLocateViews` pose/FOV
  sample. Previously the layer could describe a newer headset pose than the
  image, giving the runtime inconsistent inputs for reprojection while
  turning. This follows
  the [OpenXR projection-view contract](https://registry.khronos.org/OpenXR/specs/1.1/man/html/XrCompositionLayerProjectionView.html).
  A synthetic 180° sweep at 30°/s on Gravitude frames 420–1001 kept 59.9
  fresh stereo images/s; the gap between requested and drawn head rotation
  was 0.50° at p95 and 0.69° maximum. A faster 90° turn at 180°/s reached
  3.82° maximum. SteamVR's actual reprojection remains unmeasured.
  A large-pose-jump path now waits for the current scene render only when
  the requested head direction differs by more than 15° from the image.
  In the 1920-square Gravitude offline pipeline, an instantaneous 90° turn
  previously presented one image with a 90° pose gap; the new path refreshed
  that scene once and measured a 0° maximum gap. That exceptional refresh
  took 34.3 ms, while race frames 420–651 measured 59.8 game ticks/s and
  59.3 fresh stereo images/s. During a 180°/s continuous sweep, no blocking
  refresh was needed; race frames 420–650 measured 59.7 ticks/s and 60.0
  fresh stereo images/s, with 2.91° p95 and 8.47° maximum pose gap. This
  improves sudden turns but does not establish headset comfort or pacing.
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
  validates. The offline pipeline can now save the final compositor's two
  1920-square eye images. A forward Gravitude capture at frame 1001 shows
  placement, time, course progress, speed and the trick meter within the
  central view, with the rider still visible. A 95° side capture shows the
  course without that camera-anchored HUD, as expected when looking away.
  Both eyes retain the world and HUD. Headset comfort and menu interaction
  remain unverified.
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
- A local VR-only Gravitude sky pack now contains nine 1024×512 restorations
  of the disc's 256×128 panorama textures. The repository's Real-ESRGAN
  tool gained `--edge clamp --alpha linear`: it preserves the original
  silhouette and the top-row alpha (72/255 in four panels). Its default
  wrapping/model-alpha path made a dark curved boundary in a frozen frame;
  the clamped path removes that boundary. Frozen frame-1000 1920-square
  stereo captures were inspected at 0°, -90°, +90° and 180°; both eyes
  remained filled and distinct. With the pack, a muted live-game offline
  run through frame 1002 measured 60.0 game ticks/s, 71.8 stereo
  submissions/s and 59.6 fresh images/s over frames 420–1002. The nine
  derived PNGs live in git-ignored `textures/vr-replace`, loaded only by
  the VR executable when present. The desktop textures remain untouched.
  The distant mountains are sharper; close snow geometry and its 128×128
  textures remain visibly coarse. The headset image remains unverified.
  To regenerate the pack from an `SSX3_TEX_DUMP=textures/dump` Gravitude
  run, use the portable Real-ESRGAN executable documented in
  [`tools/upscale-textures.py`](../../tools/upscale-textures.py):

  ```powershell
  python tools/upscale-textures.py --dump textures/dump --out textures/vr-replace --esrgan textures/.tools/realesrgan/realesrgan-ncnn-vulkan.exe --model realesrgan-x4plus --edge clamp --alpha linear --jobs 1 2e1883ea3e3234e9 3a6f7b87a7fff1fa 495b17680be14ec7 961cbc35f32993aa a38a73d21917a5f5 b4befe89e8aaa8c5 bbbbb9e865d7dc34 bbf9cab272b33e27 bc3014b1fd28cc91
  ```
- A separate frozen Gravitude frame-1000 experiment replaced 17 opaque
  128×128 snow/rock tiles with 512×512 restorations. The side-view eyes
  remained fully filled and distinct, but the change averaged only 0.15
  RGB levels per channel across the image and did not improve the large
  faceted wall. This tile set is **not** in the local VR pack. VIF call
  counts on that frame show 261 Bezier1 patches using the 4×4 grid,
  75 using 6×6 and only three using 8×8. Blanket texture upscaling did not
  solve the close-wall problem; its geometry and material need separate
  treatment.
  The game-side LOD table returns 4, 6 or 8 points per patch edge. An
  experimental VR minimum of 6 changed only 50,396 of 3,686,400 pixels
  in the frame-1000 side view, leaving the wall's silhouette unchanged.
  Forcing 8 opened obvious gaps and dropped the renderer's average draw
  count from 872 to 353; its faster frame time was caused by missing
  terrain. Both LOD overrides were removed. If VR terrain is tessellated
  further, it must use source patch data on the renderer side and match
  shared edge samples, while leaving the game's LOD output intact.
- A pixel probe on Gravitude frame 1000's +90° view (512-square pixel
  400,240) identified the close wall as Bezier1 draw 237, not a sky panel.
  Its affecting GPU calls use a 6×6 grid (60 strip vertices): the first
  terrain pass samples texture `3b48244bfb79ebcb` (128×128 snow) and the
  next subtractive material pass samples `c4594029cf69c017` (128×128
  baked colour). The earlier 17-tile trial did not include either source.
  A 4× restoration of the snow tile made the repeated striations harsher;
  restoring the baked tile produced large diagonal colour blocks. Blurring
  that baked layer also exposed broad patch boundaries. None of these
  variants entered the local pack. `SSX3_GFX_PROBE` now reports the source
  draw and GPU call; `SSX3_VR_INSPECT_DRAW=<index>` reports its material,
  texture IDs and VU calls. A `SSX3_GFX_SKIP` search pointed at draw 42,
  but that draw is a five-qword DIRECT state packet, so skipping it is not
  valid visual attribution.
- A renderer-only type-1 grid refinement trial reconstructed the VU call's
  4/6/8-point rows, sampled a cubic surface at twice the grid intervals,
  and carried both texture passes onto the denser strips. Original grid
  sites and second-pass UVs were preserved in a unit test. On Gravitude
  frame 1000, refining draw 237 made the close wall only slightly less
  faceted; its stretched 128×128 texture remained prominent. Refining all
  type-1 terrain in a frozen 1920-square stereo side replay raised steady
  render time from 19.13 to 27.74 ms per frame (16 repeats); refining only
  draw 237 took 20.81 ms. Both eyes stayed filled, but the broad material
  seams remained. The trial was removed rather than made a VR default.
  The next terrain path should use the original power-basis coefficients
  available in the Nash patch data, then choose density from screen error
  and preserve matching samples on neighbouring edges. It must fit the
  stereo frame budget and keep the game's 4/6/8 guest grid unchanged.
- The packetless terrain emitter now tags each VU draw with its source Nash
  patch address, including the deferred type-1 groups. The frame builder
  copies the 16 original power-basis coefficient vectors once per distinct
  drawn patch when `SSX3_VR_TERRAIN_SOURCE=1` (the VR default). This host
  metadata does not enter VU memory or change guest state. A new muted
  Gravitude frame-1000 dump contained 392 distinct terrain sources;
  `gfx_replay` found all 392 terrain draw calls tagged and an **exact**
  match between the first VU grid position and coefficient C15 on every
  one, with none missing. Frozen
  512-square stereo images with source copying off/on had identical SHA-256
  hashes in both eyes. In adjacent 500-frame headless runs the typed-frame
  mean was 1.88/1.93 ms with copying off and 2.06 ms with copying on;
  this is an approximate 0.13–0.18 ms cost, mixed with run variance.
  A muted 1920-square Gravitude offline pipeline through frame 650, with
  a 90° head turn at frame 500, measured 60.0 game ticks/s, 71.8 stereo
  submissions/s and 59.5 fresh stereo images/s over frames 420–650 with
  capture on; the adjacent capture-off run measured 60.0, 72.0 and 59.7.
  Both retained stereo eyes were filled and distinct. These are offline
  results and remain just under the 60 fresh-images/s headset gate.
  The standard 6000-frame race replay RAM gate matches its baseline with
  terrain-source copying both off and on.
  The default renderer does not yet use the captured coefficients to change
  the visible mesh.
- An **opt-in** renderer mesh trial now uses those coefficients and the
  native strip vertices' patch coordinates. It measures each original
  triangle against both VR eyes, subdivides visible triangles above a
  projected error threshold, and shares the refined source vertices between
  the terrain's texture passes. Midpoints bordering coarse triangles or
  neighbouring patches stay on the original straight edge, so a change in
  refinement level does not open a geometric gap. The guest's 4/6/8 grid is
  unchanged. On the Gravitude frame-1000 side-view replay, a 256-triangle
  budget kept both 1920-square eyes filled and distinct, but raised the
  repeated frozen render from 24.27 to 25.69 ms (frame build 3.18 vs
  3.07 ms). A 1200-triangle trial took 28.06 ms. The visual improvement on
  that frame is modest. A muted live Gravitude run through frame 651 with
  the 256-triangle option measured only 54.2 game ticks/s and 54.0 fresh
  stereo images/s over race frames 420–651, versus 59.9 ticks/s and 59.9
  fresh images/s in the adjacent default-path run. With refinement disabled,
  the new per-vertex patch metadata produced byte-identical 512-square left
  and right images against the source-capture-off control. This CPU path
  therefore remains disabled by default. The compact GPU assembly trial below
  targets the highest-error close surfaces before repeating the headset gate.
- A second opt-in mesh path (`SSX3_VR_TERRAIN_MESH=gpu`) now retains the
  native coarse draws and sends only selected cubic midpoint positions to
  the shader, which assembles the refined geometry, texture coordinates and
  normals for each eye and texture pass. At a 0.7-pixel threshold, a frozen
  512-square Gravitude side view matched the CPU mesh pixel-for-pixel in the
  left eye; both eyes were filled and distinct. Measuring the error distribution
  then motivated a 10-pixel default threshold for this experimental mode:
  roughly 115 side-view or 129 forward-view triangles meet it at frame 1000.
  A muted 1920-square live race through frame 651 still measured only 56.3
  game ticks/s and 56.0 fresh stereo images/s over frames 420–651. The
  renderer recorded 7.7 colour snapshots and copies covering 770% of an eye
  frame per draw. An adjacent run with the mesh off measured 59.9 game ticks/s
  and 59.9 fresh stereo images/s, with 7.2 colour snapshots and 715% copied.
  This path stays off by default. These measurements are offline, with no
  new headset validation.
- The colour-copy trace identified repeated destination-alpha (DATE) tests
  on small world draws as the main source of full-eye snapshots. The VR
  renderer now projects CPU triangle vertices and conservative type-3 strip
  boxes into each eye, then copies only their affected region. It retains
  the original DATE tests. Disabling DATE changed 14–17% of pixels on two
  forward course captures, so dropping those tests is not a valid shortcut.
  With eye-space bounds enabled, 12 stereo course images across ARA1, DRA4
  and ERA5 at 0° and +90° matched the unbounded control pixel-for-pixel.
  In adjacent muted Gravitude runs at 1920-square eyes, copied area fell from
  715% to 279% of a rendered frame per draw and the average colour
  snapshots from 7.2 to 5.0. Fresh stereo rates were 59.7 and 60.0/s over
  frames 420–650; this small timing difference is within run variation.
  A later frame-650 Gravitude diagnosis found that merging adjacent type-3
  strip calls kept only the first call's 3D bound. In the forward 1920-square
  stereo replay, this skipped visible pixels: the bounded result differed
  from full-frame snapshots at 2,951 left-eye and 2,955 right-eye pixels.
  The merge now unions all six bound components across the calls. CPU
  triangles crossing the tracked eye's clip planes are bounded after
  homogeneous clipping, rather than forcing a full-frame copy. With both
  fixes, frame-650 forward and +90° stereo views and -90°/180° mono views
  match full-frame snapshots pixel-for-pixel. +90° stereo views of Snow Jam
  frame 1500 and Intimidator frame 1000 also match in both eyes. On the
  repeated +90° Gravitude stereo
  frame, copied colour area fell from 720% of an eye under the old bounded
  renderer to 453%; measured render time varied and does not establish an
  FPS gain. A muted 650-tick offline race with a 90° turn measured 59.8 game
  ticks/s, 69.7 stereo submissions/s and 57.8 fresh stereo images/s over
  frames 420–650. Both final eyes were filled and distinct. G3 remains open.
- The mesh selector now composes the eye projection once per patch. Frozen
  forward and side stereo captures with GPU refinement remained pixel-identical.
  On one repeated Gravitude side frame, selecting about 114 of 2510 candidate
  triangles still took roughly 0.8 ms; midpoint generation took 0.02 ms and
  call splitting 0.11 ms. A live refinement run with snapshot bounds reached
  58.9 fresh stereo images/s over frames 420–651. A later adjacent run on
  a busier machine fell to 51.7/s while the default path also fell to
  58.2/s. Refinement remains opt-in. Neither 60 fresh images/s with terrain
  refinement nor the headset pacing gate is established.
- The experimental GPU mesh now caches each patch's decoded coefficients,
  source positions, patch coordinates, validity and cubic edge midpoints.
  It reuses an entry only when those source values match exactly; the two
  tracked-eye projections are still recomputed each frame. On the repeated
  Gravitude side frame, 78 patch entries and 7530 edge samples were reused
  after the first frame. Selection fell from roughly 0.9 to 0.7 ms with
  cache disabled/enabled, and the forward/side stereo images were identical
  pixel-for-pixel in both eyes. Near the end of a muted live Gravitude run,
  about 190 patches per frame were reused with 0–7 misses. Adjacent live
  1920-square runs measured 3.37 versus 3.22 ms build time per rendered
  frame, while fresh stereo rates were 59.3 versus 59.4/s over race frames
  420–650; the rate difference is within measurement variation. The mesh
  stays opt-in and the headset gate remains open.
- The opt-in GPU terrain selector now reuses each edge's two-eye projected
  error within a patch selection. The cached cubic geometry still remains
  independent of the current head pose. On a 20-repeat 1920-square Gravitude
  frame-1000 side replay, it selected the same 114 of 2510 candidate
  triangles. The stereo PNG matched the previous selector byte-for-byte
  (SHA-256 `f5b1542c872d7d96e31aaae8761304ffcbc6014a286d97eb635791d00dfa417b`).
  The last three selection measurements were 0.649–0.690 ms before and
  0.568–0.651 ms after; whole-frame render time varied in the other direction,
  so this does not establish a frame-rate gain. The mesh remains opt-in.
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
  the live game, compositor and headset costs. The earlier serialized loop
  has since been replaced by the independent game/OpenXR pipeline described
  above. The 60 new stereo images/s target has not been measured on the
  headset with this build.
  The first shared-scene input pass now uploads GS vertices, VU call data,
  mesh indices, shadow uniforms, and static caster data once per game frame
  rather than three times for left eye, right eye, and UI. On the same frozen
  1920-square stereo replay with a GPU wait after every frame, adjacent runs
  measured 16.36 ms before and 14.24 ms after (frame extraction was 3.01 ms
  and 2.78 ms). The left, right, and UI PNG files have identical SHA-256
  hashes before and after. This is a local renderer improvement, not a live
  SteamVR frame-rate result; duplicated draw planning remains. The serialized
  OpenXR loop has since been replaced by the pipeline above.
  The renderer also retains the latest published frame's texture pages and
  ordered GS batches. When OpenXR asks for an image between game ticks, it
  can draw two new tracked eyes from that scene without rerunning the
  simulation or stateful VIF/GS build; the HUD texture and game depth queries
  remain on the original tick. Offline direct and retained-scene renders at
  +90° have byte-identical left and right PNGs at both 1024² and 1920²;
  the eyes differ from one another. One isolated 1920² view-only refresh
  with a GPU wait measured 5.92 ms. The live loop now reports submitted
  OpenXR frames separately from fresh stereo images and times the refresh.
  Game ticks and XR submission now run on separate threads, but 60 fresh
  stereo images/s on hardware remains unproven.
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
  The race HUD pass now receives only its main-target HUD batches when shadows
  are off, and avoids preparing the world VU inputs if none of those HUD
  batches uses them. Frozen 1920-square ARA1 frame-1500 and DRA4 frame-1000
  captures had byte-identical left, right and HUD PNGs before and after both
  changes. The shadow path retains the full batch list because it also derives
  a silhouette-target set from world draws. Adjacent repeated-render timings
  were noisy, so this has no claimed FPS gain or live OpenXR validation.
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

### Gravitude side-view diagnosis (2026-10-31)

A muted live-game capture at frame 650 with a 90-degree head turn shows a
flat-edged dark cliff, a nearly uniform purple sky, and a dark blue foreground.
Pixel probes of its stereo frame dump put the foreground in the Bezier terrain
path: draw 175 first writes a light base texture, then the game's subtractive
baked-colour pass darkens it. A pixel on the cliff/sky region is covered by
Mat3 mesh draws 198/200. Dropping draw 198 in a replay reveals the underlying
sky gradient, but leaves the cliff and foreground visibly coarse; suppressing
that draw is therefore no solution. The defect spans the native world meshes,
terrain colour pass, and the source assets rather than the snow tile alone.

The traced Mat3 rock texture is an opaque 128 × 128 tile, named
`b971a7abaf011a26.png` in the texture dump. The offending General draw uses
multiple native mesh calls with roughly 20–67 vertices each. An all-direction
Nash visibility run at the same game frame produced a byte-identical side
image, so missing gaze-cull nodes are not causing this particular edge. A
1254-square rock texture trial adds visible surface detail but cannot change
the straight source silhouette. It was not installed: adjacent 1920-square
offline-pipeline runs with a 90-degree turn at frame 500 measured 60.2 fresh
stereo images/s with the original and 57.7/s with the trial. These short runs
vary, but do not establish a free quality gain.

Three texture/refinement trials were rejected. Increasing the optional Bezier
terrain refinement from threshold 10 to 1 barely changed the side image and
exhausted its triangle budget. A generated opaque snow replacement lost the
original tile's variable alpha and made the blend harsher. A procedural
detail version that preserved alpha had little visible benefit and dropped
the live 1920-square offline fresh-stereo rate from about 59.8 to 57.4-59.0/s
across adjacent runs. No trial was installed. The next G1 work must address
the source mesh silhouette and its material together; texture upscaling alone
leaves the main defect visible. G3 still requires a live OpenXR pacing
measurement.

Keep headset review behind the remaining G1 coverage and G3 pacing work.
All game captures in this investigation used `SSX3_AUDIO=0`.

### VR texture preparation (2026-10-31)

The VR renderer now decodes local replacement PNGs and creates their mip chains
before the game/render workers start, with a 64 MiB decoded-data limit. The
local Gravitude pack preloads 9 images (24.0 MiB); first-use texture upload to
the GPU still occurs on demand. This removes PNG decoding from the frame path
for those images, without claiming an image-quality change. A muted 650-tick
ERA5 offline A/B at 1920 square per eye with a 90-degree turn measured 59.9
fresh stereo images/s with preloading disabled and 59.5 with it enabled over
frames 420–651. That difference is within run variation; sustained cadence is
unchanged by this preparation. `SSX3_VR_PRELOAD_TEXTURES=0` is the comparison
switch.

A separate source-mesh experiment subdivided the cliff while preserving its
pre-divided STQ and fog varyings. Zero-displacement refinement nearly preserved
the original image, but local-axis displacement made spikes and the silhouette
remained poor. Removing the cliff revealed a snowy background but erased real
course geometry; front-face culling likewise left holes. No mesh experiment
was installed. A source-space silhouette and material solution remains G1 work.

The offending General draw has six unlit source meshes of 40–48 vertices.
Their UVs cover most of a single 128×128 rock tile across a large wall. A
1254-square grayscale rock albedo generated from that original tile is now
bundled in the VR executable. It preserves the game's blue vertex tint and
adds visible close-view detail in frame-650 side and frame-1000 forward stereo
captures. The side silhouette is unchanged and still coarse. In adjacent
muted 1920-square 650-tick offline runs with a 90-degree turn, race frames
420–651 measured 59.5 fresh stereo images/s with the original rock and 60.0/s
with the detailed albedo, a difference within run variation. The bundled image
is predecoded with the local VR pack; that path reproduced both eyes of the
explicit trial pack byte-for-byte. An explicit file with the same texture hash
still takes precedence. The rebuilt VR executable's default-pack offline run
measured 59.6 fresh stereo images/s over frames 420–651, with both eyes filled
and distinct. This improves one material but does not close G1 or prove the
SteamVR frame-rate gate.

The native frame-650 mesh topology explains the hard edge more precisely.
Draw 198 repeats six small closed rock meshes in two screen-matrix groups;
the visible final cliff is draw 200, whose 24 source calls form mostly open
patches. Shared positions must be welded across calls before changing them.
An opt-in VR renderer trial (`SSX3_VR_ROCK_SMOOTH=0.7`) now applies one blended
Loop subdivision step to the multi-call groups, retaining open boundary
vertices and reusing refined source meshes when the camera matrix changes.
On a frozen 1920-square side stereo replay, it changed 526,757 left-eye and
523,179 right-eye pixels, rounded the cliff outline, and kept both eyes
filled. Twenty repeats measured 18.23 ms per rendered stereo frame without
the trial and 18.31 ms with it; this small difference is not a reliable
performance claim. Single-call refinement affected only about 1,100 pixels
per eye in the same view and was omitted. A muted live 650-tick run reused
28 source calls by frame 600 without preparing new groups; race frames
420–650 measured 55.9 fresh stereo images/s, but an adjacent control run
hit a long refresh and measured 44.8/s, so these runs do not establish the
trial's live cost. The large rock UVs remain visibly stretched, and source
geometry is still coarse. This trial was initially kept off; G1, G3 and
headset review remained open.

The Gravitude rock path now enables the 0.7 source-space smoothing by default
in VR and maps its detailed albedo onto each source triangle's dominant local
plane, four texture repeats per local unit. Projection seams duplicate vertex
records but keep the resulting position, colour, draw order and stereo geometry;
the tiling is available on the first frame while the multi-call smooth mesh
cache warms. The frame-650 +90° cliff has less stretched surface texture and
a softer outline, although its blue material and some large-scale streaks
remain. Frozen 1920-square stereo forward frame 1000 and +90° frame 650 have
filled, distinct eyes; the UI is unchanged. With both VR settings set to zero,
the side eyes and UI match the prior renderer byte-for-byte, and the desktop
frame is byte-identical with the new defaults. Frozen ARA1, BRA2, CRA3 and
DRA4 views did not use this Gravitude rock material. `SSX3_VR_ROCK_SMOOTH=0`
and `SSX3_VR_ROCK_TILE=0` restore the original VR rock surface.

Adjacent muted 1920-square Gravitude offline runs, both with smoothing 0.7,
measured 59.1 fresh stereo images/s without tiling and 59.2 with four repeats
over race frames 420–650/651. The difference is within run variation. Both
completed a retained +90° refresh with distinct eyes. This one-course material
improvement does not close G1's overall side/rear coverage or G3's sustained
SteamVR pacing gate. Two subsequent default-setting runs measured 53.8 and
60.0 fresh stereo images/s on the same race interval, reinforcing that a
short offline run is not a sustained frame-rate result.

### Gravitude upper-sky joins (2026-10-31)

The upper panorama consists of four 45-degree strips, three contiguous over
angles -90 to 45 degrees and one isolated at 180 to 225 degrees. The prior
edge alpha faded both sides of each strip into the plain dome, creating a dark
vertical notch at the two shared borders (-45 and 0 degrees). Gravitude now
keeps alpha at those joins while retaining the fade at unpaired outer edges.
Frozen frame-650 stereo replays at 1920 square per eye show the notch gone in
the forward view. The 90-degree side images are byte-identical to the old
fade in both eyes, and the missing outer-sector wedge stays hidden. Both
forward eyes are fully covered and distinct. A muted 650-tick offline run at
the same size measured 59.9 game ticks/s, 71.5 stereo submissions/s and 59.6
fresh stereo images/s over frames 420-651, comparable to the preceding run.
The lower/upper artwork still has a horizontal transition and the side course
wall remains coarse; G1 is open. `SSX3_VR_SKY_GRAVITUDE_SMOOTH=0` restores
the former fade.

The remaining horizontal transition was probed on frozen frame 650: upper
Mat3 sky draw 7 supplies a pixel just above it, while lower Mat3 sky draw 4
supplies the pixel below. Their sampled colours differ before post effects.
An opt-in trial extended the upper strip below its native boundary and faded
its alpha while clamping its last texture row. It removed the hard edge but
stretched that row into conspicuous vertical streaks across the mountains.
That trial was reverted. The installed path instead extends the lower strip
two mesh rows upward, maps its whole source texture across the new height, and
fades the upper strip through those same rows. The two original images blend
without clamping an edge texel. Frozen Gravitude stereo captures at frames
650, 1000 and 1500 show the hard horizontal band gone in the forward view;
both 1920-square eyes at frames 650 and 1000 remain filled and distinct. The
frame-650 side images are byte-identical to the prior path in both eyes. In
adjacent muted 650-tick offline runs at 1920 square per eye, race frames
420-651 measured 59.7 fresh stereo images/s with the blend and 60.0/s with
it disabled. The small difference is within run variation; headset pacing
remains unverified. `SSX3_VR_SKY_GRAVITUDE_HORIZON=0` restores the previous
hard border. Close course walls still need a separate geometry/material fix.

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

The first ownership cut is installed: `Gs::take_geometry` detaches the
frame's CPU vertices, GPU source calls and ordered batches after the native
VIF/GS build. Both eye draws read that geometry, and the published VR scene
retains it with its frame-memory snapshot for a later head-pose refresh.
The GS register/texture carry remains available for the next game tick, and
the detached vector storage is recycled to avoid per-tick allocations. On
Gravitude frame 650, frozen 1920-square forward and +90° stereo eyes match
the preceding renderer pixel-for-pixel; a 1024×768 desktop image also
matches. A muted 650-tick offline run with a 90° turn completed a retained
scene refresh, kept both eyes filled and distinct, and measured 60.6 game
ticks/s and 59.9 fresh stereo images/s over frames 420–650. This is a
single offline run, not a sustained headset-rate result. The next cut
captures the GS camera, frame size, target map, context registers, light
values and fog palettes with the detached geometry. `draw_built` now has no
direct reads from the mutable `Gs` carry. Frozen Gravitude forward and +90°
stereo eyes, and the desktop image, still match the preceding renderer pixel
for pixel; all 31 graphics library tests pass. A first muted offline run of
this cut completed 650 ticks with both eyes filled and distinct. Two muted
offline runs measured 51.0 and 59.1 fresh stereo images/s over frames
420–650, showing run-to-run variation. At that point the renderer's
per-frame inputs still came from mutable `Inner` state. The 60 fresh
stereo/s target remained open.

The frame's renderer-owned inputs are now retained beside that GS snapshot:
phase boundary, display target, depth scale, GraphicsMan address, world shadow
casters, painter sun and the lens-flare light query. A head-pose refresh reads
these from the published frame rather than from the next mutable renderer
state. Frozen Gravitude frame-650 forward and +90° stereo eyes, UI and desktop
output match the previous commit byte-for-byte; all 31 graphics tests pass.
A muted 1920-square 650-tick offline run with a 90° turn completed a retained
refresh with distinct, filled eyes and measured 59.8 game ticks/s and 59.8
fresh stereo images/s over frames 420–651. Temporal light smoothing, texture
and target resources, and queue submission are still owned by `Inner`; this
cut alone does not establish XR-cadence parallel rendering or sustained 60.

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

For close Bézier terrain, keep the game's 4/6/8 edge decisions and guest
buffers unchanged. Capture each patch's original control points or
power-basis coefficients before its fixed grid is emitted, then evaluate
an immutable VR mesh at a screen-error-driven density. Neighbouring patches
must use identical samples along shared edges; the terrain's two texture
passes, normals, shadows and draw order must still refer to the same mesh.
Use the native grid when source data is unavailable. Compare frozen
forward/side stereo images for holes and silhouette changes before timing
the live pipeline. The direct game-side LOD override failed this gate.

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
