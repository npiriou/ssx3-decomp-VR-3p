# Third-person SteamVR adaptation

## Scope

Adapt the existing native Rust game. Keep the SSX 3 simulation, gameplay
camera, animation, collision, audio, saves and controller input. Introduce a
VR front end and stereo rendering without running the game simulation twice.
The first playable target is Windows, SteamVR as the active OpenXR runtime,
gamepad control and 60 game ticks per second.

## Existing integration points

- `crates/ssx3/src/main.rs` drives the game with `tick::Pacer`; `game.rs`
  boots the existing machine and installs the native renderer.
- `crates/ssx3-ps2/src/frame_sink.rs` already adjusts the game's projection
  aspect and field of view, including its culling values.
- `crates/ssx3-render/src/frame.rs` records view and matrix information in
  `Frame`, `View` and `MatrixRecord`. A published frame represents one game
  update; it is the input to `ssx3-gfx`.
- `crates/ssx3-gfx/src/lib.rs` builds and renders the frame. Some geometry is
  already in screen coordinates while GPU VU draws still have world/clip
  information. Stereo must cover both paths, terrain, riders, effects and
  culling. The current `present_view()` is a single flat image.

## Implementation order

1. **OpenXR front end.** Add a Windows VR binary beside `ssx3`, discover the
   SteamVR OpenXR runtime, create the session and two eye swapchains with
   the graphics API/device selected for the native renderer. Follow the
   OpenXR event and wait/begin/end frame sequence; report headset resolution,
   display period and errors. Keep the desktop binary working.
2. **Stereo camera.** Use the game's chase camera as the third-person anchor.
   Compose its pose with the HMD orientation and the OpenXR poses and
   projections for each eye. Set a comfortable initial camera distance and
   world scale. Apply the transform before culling and to every 3D render
   path. Draw the world twice from one immutable game frame. Verify real
   parallax, visible rider and terrain at near, middle and far distance.
3. **HUD and presentation.** Render menus, race HUD and prompts to a stable
   head-locked or world-locked layer rather than making the whole game image
   follow the head. Submit both eye textures to OpenXR; keep an optional
   desktop mirror for debugging.
4. **Input and comfort.** Keep gamepad steering and tricks. Add recenter,
   optional head-look limits and comfort settings for camera roll, shake and
   FOV changes. Never use HMD motion as a rider movement input by default.
5. **Performance.** Keep `SSX3_TICK_HZ=60` for the first target. Instrument
   game, build, each eye, GPU and OpenXR frame times. Reuse textures and
   geometry across eyes where possible. Profile an actual race and check
   frame delivery against the runtime's predicted display period. A 60 fps
   target does not imply that every headset has a 60 Hz mode; compositor
   reprojection and headset refresh must be measured on real hardware.

## Acceptance checks

- With SteamVR active and a headset connected, a race boots in the headset
  with a visible rider, independent left/right eye views and head tracking.
- The same game update feeds both eyes; no double simulation, audio tick or
  controller poll.
- HUD text is legible and stays at a comfortable apparent distance.
- Game updates average 60/s, and new stereo images average at least 60/s
  in a representative course on the target PC. Log missed frames and the
  headset's actual display period.
- Desktop rendering, saves and the race replay RAM gate still work.

## First technical decision

Use OpenXR, with SteamVR selected as the OpenXR runtime. The native renderer
already uses `wgpu`; the graphics binding and swapchain texture handoff are
the first engineering spike. Do not introduce a second game engine or rewrite
the SSX 3 game loop.

The initial `ssx3-vr-probe` checks loader, runtime, graphics extension and
headset view sizes. Its `--session` mode creates a Vulkan graphics session,
locates both HMD eye poses at the predicted display time, follows session
events and submits colored images to a two-layer swapchain. On the local
machine on 2026-10-29 it
reported SteamVR/OpenXR 2.18.4, a PICO headset, two recommended 1920 × 1920
views, Vulkan binding support and a 13.889 ms display period (72 Hz). The
session entered READY, VISIBLE and FOCUSED, then exited through STOPPING.
The swapchain uses three 1920 × 1920 × 2 RGBA8 sRGB images. A later run while
the HMD was idle reported a throttled 69.445 ms period (14.4 Hz); that run
proves API submission but not visible in-headset presentation. Stereo game
rendering and frame delivery remain to be implemented and measured.

The `--wgpu` mode now asks OpenXR to create the Vulkan instance and device,
then adopts both into wgpu 30. The local SteamVR runtime selected the RTX
3090. The `--wgpu-session` mode also wraps the OpenXR swapchain images as
wgpu render targets and clears each eye through wgpu. It submitted 1 of 144
frames while the HMD was idle, without Vulkan or wgpu errors. This proves
that the native renderer can share the exact GPU device used by SteamVR;
the game image, stereo camera, HUD and controls are not connected yet.

The experimental `ssx3-vr` binary now boots the existing game, installs its
native renderer on the OpenXR-created wgpu device, polls the existing gamepad
mapping and submits the renderer's image to the XR swapchain. The image is
still duplicated in both eyes. A short ARA1 run reached frame 700 with the
renderer producing 512×448 images. The PICO was idle and OpenXR requested
only one renderable image, before the race image was ready, so actual in-headset
game presentation is unverified. The same race without XR benchmarked 92 fps
over frames 500–700. In the idle XR loop, game update rate fluctuated around
50–60 fps after synchronization; this cannot establish active-headset
performance. Initial GPU loss was consistent with unsynchronized host access
to the Vulkan queue between the renderer worker and OpenXR. Flushing the
worker before the OpenXR acquire/release/end calls let the 700-frame run finish.
This flush may cost overlap; a shared queue lock is the longer-term fix.

At this headset setting the OpenXR loop runs at 72 Hz while the initial game
simulation target is 60 Hz. A fixed 60/72 presentation pattern would judder;
the renderer must keep head pose updates at the runtime cadence and either
interpolate the chase camera or use a headset mode divisible by 60. The
performance target is at least 60 distinct stereo images per second, with
smooth head tracking at the selected headset refresh rate.

References: [Khronos OpenXR frame submission](https://github.com/KhronosGroup/OpenXR-Guide/blob/main/chapters/frame_submission.md),
[OpenXR Rust bindings](https://github.com/Ralith/openxrs), and
[Steamworks VR application settings](https://partner.steamgames.com/doc/features/steamvr/settings).
