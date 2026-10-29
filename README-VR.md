# SSX 3 VR

This directory is an independent copy of `ssx3-decomp` for a native VR
adaptation. The game, its Rust port, assets loaded from the owner's disc,
controller mapping, sound and save system remain the base of the project.

## Initial target

- Third-person play: retain the game's chase camera and visible rider.
- SteamVR through its OpenXR runtime on Windows.
- 60 game updates per second (`SSX3_TICK_HZ=60`) and a measured target of
  60 new stereo images per second. The OpenXR compositor still runs at the
  headset's selected refresh rate; frame pacing must follow its predictions.
- Existing gamepad controls for the first playable version. Head tracking
  adjusts the view around the chase camera without steering the rider.

## Current state

The experimental `ssx3-vr` executable boots the existing game and renders
its 3D triangles twice, from the two OpenXR eye poses. The game's chase
camera still follows the rider. Turning the headset changes the view into
the scene; both eye images differ. The old duplicated flat image is gone.

**This is not yet a finished playable VR release.** The game's normal draw
list covers mainly the forward camera view. Large head turns, especially
around 90 degrees and behind the rider, reveal missing or stretched geometry.
The race HUD is still composited into each eye's image. A warmed idle-headset
run reached about 50 game updates/s on the local RTX 3090, short of the
60/s target. In-headset image quality and active-headset frame delivery have
not yet been verified. See [the VR implementation plan](docs/plans/vr.md).

## Local setup

The original disc image is not copied here. Point `SSX3_ISO` to your own
SSX 3 (USA) image. `orig/`, `generated/` and `saves/` were copied locally so
this checkout retains the original project's generated code and test data.
They remain ignored by Git and must not be published.

The desktop baseline can be run from this directory with:

```powershell
$env:SSX3_ISO = 'X:\dev\ssx3\SSX 3 (USA).iso'
$env:SSX3_TICK_HZ = '60'
cargo run --profile game -p ssx3 --bin ssx3
```

For the experimental VR front end, with SteamVR selected as the active
OpenXR runtime and a gamepad connected:

```powershell
$env:SSX3_ISO = 'X:\dev\ssx3\SSX 3 (USA).iso'
$env:SSX3_EVENT = 'ARA1'
cargo run --profile game -p ssx3 --bin ssx3-vr
```

Set `SSX3_AUDIO=0` for silent tests. `SSX3_EXIT_AFTER_FRAMES=700` exits a
short course run. While a headset is idle, SteamVR may request almost no
visible images; those runs validate game simulation but not headset visuals
or VR frame rate. `SSX3_VR_NO_SUBMIT=1` skips OpenXR images and
`SSX3_VR_CLEAR_ONLY=1` submits colored eyes to isolate GPU interop issues.
`SSX3_VR_UNITS_PER_M` adjusts the camera world scale (default 100 game units
per metre).

For a headset-free renderer check, capture a course frame with
`SSX3_FRAME_DUMP=2`, then run `gfx_replay` with
`SSX3_VR_REPLAY_EYE=stereo`. It writes separate `-left.png` and `-right.png`
images and checks that the two rendered eye images differ.

To inspect the active OpenXR runtime and headset before the VR renderer is
ready, run `cargo run -p ssx3-vr --bin ssx3-vr-probe`. This diagnostic does
not start the game or create a VR graphics session. If the OpenXR loader DLL
is installed outside the default Steam directory, set `SSX3_OPENXR_LOADER`
to its full path.

`cargo run -p ssx3-vr --bin ssx3-vr-probe -- --session` also creates an
OpenXR Vulkan graphics session and submits a short sequence of red and blue
eye images. It reports the headset pose and predicted display period, then
closes the session. `--wgpu-session` exercises the renderer's shared wgpu
device and OpenXR swapchain directly.
