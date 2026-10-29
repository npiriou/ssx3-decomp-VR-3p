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

The repository is copied and can still run as the native desktop game. **It
does not yet render to a headset.** See [the VR implementation plan](docs/plans/vr.md)
for the integration points and acceptance tests. Do not treat a flat image
duplicated to both eyes as the stereo milestone.

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

The VR executable will be a separate front end. Until it exists, this
command is a desktop baseline only.
