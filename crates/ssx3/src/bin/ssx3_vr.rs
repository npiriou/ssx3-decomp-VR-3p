//! Experimental SteamVR front end for the existing native SSX 3 game.
//! The game's chase camera is reprojected into two tracked scene views.

#[cfg(all(feature = "ee-threads", not(feature = "ee-unwind")))]
extern crate ssx3_ee_threads as ssx3_ee;
#[cfg(feature = "ee-unwind")]
extern crate ssx3_ee_unwind as ssx3_ee;

use std::error::Error;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use gilrs::{Axis, Button, Gilrs};
use ssx3_vr::openxr as xr;
use ssx3_vr::xr_wgpu;
use ssx3_vr::xr_wgpu_session::{self, StereoTarget, VIEW_TYPE};

const STAGE_NAMES: [&str; 12] = [
    "xr_wait",
    "xr_begin",
    "gfx_lock",
    "present_age",
    "xr_acquire",
    "blit_encode",
    "queue_wait",
    "xr_release",
    "xr_end",
    "view_refresh",
    "eye_pose_lag_deg",
    "gaze_visibility_wait",
];

/// Short-window CPU timings. Queue wait includes GPU work submitted before
/// the eye blit, so it is not a GPU timestamp for an individual eye.
struct StageWindow {
    samples: [Vec<f64>; STAGE_NAMES.len()],
}

impl StageWindow {
    fn new() -> Self {
        Self {
            samples: std::array::from_fn(|_| Vec::new()),
        }
    }

    fn push(&mut self, stage: usize, ms: f64) {
        self.samples[stage].push(ms);
    }

    fn elapsed(&mut self, stage: usize, start: Instant) {
        self.push(stage, start.elapsed().as_secs_f64() * 1000.0);
    }

    fn report_and_reset(&mut self) -> String {
        let mut out = String::new();
        for (name, values) in STAGE_NAMES.iter().zip(&mut self.samples) {
            if values.is_empty() {
                continue;
            }
            values.sort_by(f64::total_cmp);
            let n = values.len();
            let p95 = ((n * 95).div_ceil(100) - 1).min(n - 1);
            let unit = if *name == "eye_pose_lag_deg" {
                "deg"
            } else {
                "ms"
            };
            out.push_str(&format!(
                " {name}[n={n} p50={:.2} p95={:.2} max={:.2}{unit}]",
                values[n / 2],
                values[p95],
                values[n - 1]
            ));
            values.clear();
        }
        out
    }
}

#[derive(Default)]
struct GameProgress {
    running: AtomicBool,
    stop: AtomicBool,
    frames: AtomicU64,
    tick_us: AtomicU64,
    status: Mutex<String>,
    error: Mutex<Option<String>>,
}

/// Wait for the renderer to publish the specific Nash visibility generation.
/// Waiting for its queue to become empty can starve while the 60 Hz game
/// keeps publishing frames.
fn wait_for_gaze_frame(gfx: &ssx3_gfx::Renderer, generation: u64, timeout: Duration) -> bool {
    let started = Instant::now();
    while gfx.present_vr_visibility_epoch() < generation {
        if started.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_micros(500));
    }
    true
}

/// The EE machine is created and stays on this thread. It publishes one
/// native game state per 60 Hz tick while OpenXR presents completed images
/// independently on the calling thread.
struct GameWorker {
    progress: Arc<GameProgress>,
    start: Option<mpsc::SyncSender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl GameWorker {
    fn boot() -> Result<(Self, ssx3_gfx::Renderer), Box<dyn Error>> {
        let progress = Arc::new(GameProgress::default());
        let state = progress.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel(0);
        let (start_tx, start_rx) = mpsc::sync_channel(0);
        let thread = std::thread::Builder::new()
            .name("ssx3-game".into())
            .spawn(move || {
                let mut cpu = game::boot(true);
                let gfx = game::install_renderer(&mut cpu);
                let playable = gfx.is_some();
                if ready_tx.send(gfx).is_err() || !playable || start_rx.recv().is_err() {
                    return;
                }
                let mut gilrs = Gilrs::new().map(Some).unwrap_or_else(|err| {
                    eprintln!("[vr pad] gamepad API unavailable: {err}");
                    None
                });
                let game_hz = ssx3_ps2::machine::vsync_hz() as f64 / 1.001;
                let mut pacer = ssx3_game::tick::Pacer::default();
                let mut clock = Instant::now();
                let mut was_running = false;
                while !state.stop.load(Ordering::Acquire) {
                    if !state.running.load(Ordering::Acquire) {
                        was_running = false;
                        std::thread::sleep(Duration::from_millis(2));
                        continue;
                    }
                    if !was_running {
                        clock = Instant::now();
                        pacer = ssx3_game::tick::Pacer::default();
                        was_running = true;
                    }
                    let plan = pacer.plan(clock.elapsed().as_secs_f64(), 1.0 / game_hz, 8, true);
                    ssx3_ps2::machine::set_audio_lead_frames((pacer.audio_lead() * 48000.0) as u32);
                    ssx3_game::tick::set_updates_per_vsync(plan.updates_per_vsync.max(1));
                    for _ in 0..plan.vsyncs {
                        if state.stop.load(Ordering::Acquire)
                            || !state.running.load(Ordering::Acquire)
                        {
                            break;
                        }
                        let start = Instant::now();
                        update_pad(&mut gilrs, &mut cpu);
                        if !ssx3_ps2::machine::run_frame(&mut cpu) {
                            *state.error.lock().unwrap() = Some(format!(
                                "Game halted: {:?}",
                                ssx3_ps2::m!(cpu).kernel.halted
                            ));
                            return;
                        }
                        state
                            .tick_us
                            .fetch_add(start.elapsed().as_micros() as u64, Ordering::Relaxed);
                        let frames = state.frames.fetch_add(1, Ordering::Release) + 1;
                        if frames % 60 == 0 {
                            *state.status.lock().unwrap() = game::status(&cpu);
                        }
                    }
                    if plan.vsyncs == 0 {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
            })?;
        let worker = Self {
            progress,
            start: Some(start_tx),
            thread: Some(thread),
        };
        let gfx = ready_rx
            .recv()
            .map_err(|_| "VR game worker stopped during boot")?
            .ok_or("Native game renderer did not start on the OpenXR GPU")?;
        // Keep the worker at the boot barrier until the main thread has set
        // the headset's eye target size.
        worker.progress.running.store(false, Ordering::Release);
        Ok((worker, gfx))
    }

    fn start(&mut self) -> Result<(), Box<dyn Error>> {
        self.start
            .take()
            .ok_or("VR game worker already started")?
            .send(())?;
        Ok(())
    }
}

impl Drop for GameWorker {
    fn drop(&mut self) {
        self.progress.stop.store(true, Ordering::Release);
        self.progress.running.store(false, Ordering::Release);
        self.start.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[path = "../vr_camera.rs"]
mod vr_camera;

#[allow(dead_code)]
#[path = "../game.rs"]
mod game;
#[path = "../png.rs"]
mod png;
#[allow(dead_code)]
#[path = "../settings.rs"]
mod settings;

fn pad_bit(button: Button) -> Option<u16> {
    use Button::*;
    Some(match button {
        Select => 1 << 0,
        LeftThumb => 1 << 1,
        RightThumb => 1 << 2,
        Start => 1 << 3,
        DPadUp => 1 << 4,
        DPadRight => 1 << 5,
        DPadDown => 1 << 6,
        DPadLeft => 1 << 7,
        LeftTrigger2 => 1 << 8,
        RightTrigger2 => 1 << 9,
        LeftTrigger => 1 << 10,
        RightTrigger => 1 << 11,
        North => 1 << 12,
        East => 1 << 13,
        South => 1 << 14,
        West => 1 << 15,
        _ => return None,
    })
}

fn update_pad(gilrs: &mut Option<Gilrs>, cpu: &mut ssx3_ps2::Cpu) {
    let mut buttons = ssx3_game::event_boot::pad_buttons();
    let mut sticks = [0x80u8; 4];
    if let Some(g) = gilrs {
        while let Some(event) = g.next_event() {
            match event.event {
                gilrs::EventType::Connected => {
                    eprintln!("[vr pad] connected: {}", g.gamepad(event.id).name())
                }
                gilrs::EventType::Disconnected => {
                    eprintln!("[vr pad] disconnected: {}", g.gamepad(event.id).name())
                }
                _ => {}
            }
        }
        if let Some((_, pad)) = g.gamepads().next() {
            for button in [
                Button::Select,
                Button::LeftThumb,
                Button::RightThumb,
                Button::Start,
                Button::DPadUp,
                Button::DPadRight,
                Button::DPadDown,
                Button::DPadLeft,
                Button::LeftTrigger2,
                Button::RightTrigger2,
                Button::LeftTrigger,
                Button::RightTrigger,
                Button::North,
                Button::East,
                Button::South,
                Button::West,
            ] {
                if pad.is_pressed(button) {
                    buttons |= pad_bit(button).unwrap();
                }
            }
            let axis = |which: Axis, invert: bool| {
                let v = pad.value(which).clamp(-1.0, 1.0);
                let v = if invert { -v } else { v };
                (v * 127.5 + 127.5).round() as u8
            };
            sticks = [
                axis(Axis::RightStickX, false),
                axis(Axis::RightStickY, true),
                axis(Axis::LeftStickX, false),
                axis(Axis::LeftStickY, true),
            ];
        }
    }
    let sif = &mut ssx3_ps2::m!(cpu).sif;
    sif.pad_buttons[0] = buttons;
    sif.pad_sticks[0] = sticks;
}

fn configure_vr_game() {
    // Ask Nash to retain nearby world nodes in every head direction before
    // the typed frame is published. Desktop/headless runs keep their cull.
    std::env::set_var("SSX3_VR_VISIBILITY", "1");
    // Retain the source coefficients of terrain actually drawn this frame
    // for a future per-eye, screen-error-based mesh path.
    if std::env::var_os("SSX3_VR_TERRAIN_SOURCE").is_none() {
        std::env::set_var("SSX3_VR_TERRAIN_SOURCE", "1");
    }
    // Ship the few hand-reconstructed close-view textures with the VR binary.
    // An explicit texture pack still overrides individual built-in images.
    if std::env::var_os("SSX3_VR_TEXTURES").is_none() {
        std::env::set_var("SSX3_VR_TEXTURES", "1");
    }
    if std::env::var_os("SSX3_TEX_REPLACE").is_none() {
        // A VR-only local pack is generated from the owner's disc. Keep the
        // desktop texture directory and explicit pack overrides untouched.
        let local = std::path::PathBuf::from("textures/vr-replace");
        let beside_exe = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join("textures/vr-replace")));
        let source_tree = game::repo_root().join("textures/vr-replace");
        if let Some(dir) = [Some(local), beside_exe, Some(source_tree)]
            .into_iter()
            .flatten()
            .find(|dir| dir.is_dir())
        {
            std::env::set_var("SSX3_TEX_REPLACE", dir);
        }
    }
    game::init_standalone();
    settings::load();
    // VR's first target is fixed at 60 game updates per second. Display
    // refresh follows OpenXR and may be higher or lower than 60 Hz.
    std::env::set_var("SSX3_TICK_HZ", "60");
    // The PS2 bloom replays several full-screen copies for every eye.
    // Keep it available as an explicit option, but start VR without it:
    // a 1920-square stereo replay measured 20.04 ms with the original
    // bloom and 13.89 ms without it on the target PC.
    if std::env::var_os("SSX3_BLOOM").is_none() {
        std::env::set_var("SSX3_BLOOM", "off");
    }
    env_logger::init();
}

fn vr_units_per_metre() -> f32 {
    std::env::var("SSX3_VR_UNITS_PER_M")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(100.0)
}

fn vr_panel_config(units_per_metre: f32) -> xr_wgpu_session::PanelConfig {
    let ui_setting = |name: &str, default: f32, min: f32, max: f32| {
        std::env::var(name)
            .ok()
            .and_then(|s| s.parse::<f32>().ok())
            .filter(|v| v.is_finite() && (min..=max).contains(v))
            .unwrap_or(default)
    };
    xr_wgpu_session::PanelConfig {
        width: ui_setting("SSX3_VR_UI_WIDTH_M", 1.3, 0.5, 3.0) * units_per_metre,
        height: ui_setting("SSX3_VR_UI_HEIGHT_M", 1.3, 0.5, 3.0) * units_per_metre,
        distance: ui_setting("SSX3_VR_UI_DISTANCE_M", 2.0, 1.0, 5.0) * units_per_metre,
        y: ui_setting("SSX3_VR_UI_HEIGHT_OFFSET_M", 0.0, -1.0, 1.0) * units_per_metre,
    }
}

fn panel_eyes(eyes: [ssx3_gfx::vr::Eye; 2]) -> [xr_wgpu_session::PanelEye; 2] {
    eyes.map(|eye| xr_wgpu_session::PanelEye {
        game_from_eye: ssx3_gfx::vr::inverse(eye.view_from_game).unwrap_or(ssx3_gfx::vr::IDENTITY),
        clip: [
            eye.projection[0][0],
            eye.projection[1][1],
            eye.projection[2][0],
            eye.projection[2][1],
        ],
    })
}

fn run() -> Result<(), Box<dyn Error>> {
    configure_vr_game();
    let entry = ssx3_vr::load_entry()?;
    let extensions = entry.enumerate_extensions()?;
    if !extensions.khr_vulkan_enable2 {
        return Err("SteamVR OpenXR runtime lacks XR_KHR_vulkan_enable2".into());
    }
    let mut enabled = xr::ExtensionSet::default();
    enabled.khr_vulkan_enable2 = true;
    let instance = entry.create_instance(
        &xr::ApplicationInfo {
            application_name: "SSX 3 VR",
            application_version: 1,
            engine_name: "ssx3-decomp",
            engine_version: 1,
            ..Default::default()
        },
        &enabled,
        &[],
        &(),
    )?;
    println!("OpenXR runtime: {}", instance.properties()?.runtime_name);
    let system = instance.system(xr::FormFactor::HEAD_MOUNTED_DISPLAY)?;
    let graphics = xr_wgpu::create(&instance, system)?;
    println!("OpenXR GPU: {}", graphics.adapter.get_info().name);
    ssx3_ps2::gpu::install_context(ssx3_ps2::gpu::GpuContext {
        instance: graphics.instance.clone(),
        adapter: graphics.adapter.clone(),
        device: graphics.device.clone(),
        queue: graphics.queue.clone(),
    })
    .map_err(|_| "GPU context was initialized before OpenXR")?;
    let (session, mut waiter, mut stream) =
        xr_wgpu_session::create_session(&instance, system, &graphics)?;
    let local =
        session.create_reference_space(xr::ReferenceSpaceType::LOCAL, xr::Posef::IDENTITY)?;
    let blend = instance.enumerate_environment_blend_modes(system, VIEW_TYPE)?[0];
    let target = StereoTarget::new(&instance, system, &session, &graphics.device)?;
    let (mut game_worker, gfx) = GameWorker::boot()?;
    // Keep the internal world image at the OpenXR recommended extent by
    // default. The previous automatic 768 -> 640 -> 512 reduction produced a
    // visibly pixelated image without fixing the headset cadence. An explicit
    // setting remains useful for controlled profiling.
    let fixed_eye_size = std::env::var("SSX3_VR_EYE_SIZE")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|&v| (512..=3840).contains(&v));
    let eye_size = fixed_eye_size.map_or_else(|| target.eye_size(), |size| (size, size));
    gfx.set_output_size(Some(eye_size));
    eprintln!(
        "[vr] internal eye target: {}x{} (OpenXR recommended {}x{})",
        eye_size.0,
        eye_size.1,
        target.eye_size().0,
        target.eye_size().1,
    );
    ssx3_ps2::machine::set_realtime(true);
    game_worker.start()?;
    let progress = game_worker.progress.clone();
    let exit_after = std::env::var("SSX3_EXIT_AFTER_FRAMES")
        .ok()
        .and_then(|v| v.parse::<u64>().ok());
    let no_submit = std::env::var_os("SSX3_VR_NO_SUBMIT").is_some();
    let clear_only = std::env::var_os("SSX3_VR_CLEAR_ONLY").is_some();
    let start = Instant::now();
    let mut submitted = 0u64;
    let mut fresh_stereo = 0u64;
    let mut last_fresh_stereo = 0u64;
    let mut last_present_serial = 0u64;
    let mut game_rate_mark = Instant::now();
    let mut last_game_frames = 0u64;
    let mut last_publish_wait_us = 0u64;
    let mut last_submitted = 0u64;
    let mut blit_ms = 0.0f64;
    let mut stages = StageWindow::new();
    let mut events = xr::EventDataBuffer::new();
    let mut running = false;
    let mut requested_exit = false;
    let mut anchor: Option<vr_camera::Anchor> = None;
    let mut current_eyes: Option<[ssx3_gfx::vr::Eye; 2]> = None;
    let mut gaze_generation = 0u64;
    let mut last_gaze_waited = 0u64;
    let mut pose_history = vr_camera::PoseHistory::default();
    let mut pose_misses = 0u64;
    let units_per_metre = vr_units_per_metre();
    let panel = vr_panel_config(units_per_metre);
    eprintln!(
        "[vr] UI panel: {:.2}x{:.2}m at {:.2}m, height offset {:.2}m",
        panel.width / units_per_metre,
        panel.height / units_per_metre,
        panel.distance / units_per_metre,
        panel.y / units_per_metre
    );
    loop {
        while let Some(event) = instance.poll_event(&mut events)? {
            match event {
                xr::Event::SessionStateChanged(change) => {
                    eprintln!("[vr] session: {:?}", change.state());
                    match change.state() {
                        xr::SessionState::READY => {
                            gfx.with_queue_exclusive(|| session.begin(VIEW_TYPE))?;
                            running = true;
                            progress.running.store(true, Ordering::Release);
                            game_rate_mark = Instant::now();
                            last_game_frames = progress.frames.load(Ordering::Acquire);
                            last_publish_wait_us = gfx.publish_wait_us();
                        }
                        xr::SessionState::FOCUSED => {
                            anchor = None;
                            pose_history.clear();
                        }
                        xr::SessionState::STOPPING => {
                            progress.running.store(false, Ordering::Release);
                            gfx.with_queue_exclusive(|| session.end())?;
                            running = false;
                            pose_history.clear();
                            if requested_exit {
                                return Ok(());
                            }
                        }
                        xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => return Ok(()),
                        _ => {}
                    }
                }
                xr::Event::InstanceLossPending(_) => return Ok(()),
                _ => {}
            }
        }
        if !running {
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        let xr_wait_start = Instant::now();
        let frame = waiter.wait()?;
        stages.elapsed(0, xr_wait_start);
        let xr_begin_start = Instant::now();
        gfx.with_queue_exclusive(|| stream.begin())?;
        stages.elapsed(1, xr_begin_start);
        let (flags, views) =
            session.locate_views(VIEW_TYPE, frame.predicted_display_time, &local)?;
        if flags
            .contains(xr::ViewStateFlags::POSITION_VALID | xr::ViewStateFlags::ORIENTATION_VALID)
        {
            if anchor.is_none() {
                anchor = vr_camera::Anchor::new(&views);
            }
            let eyes = anchor.and_then(|a| a.eyes(&views, units_per_metre));
            if let Some(eyes) = eyes {
                let view = &eyes[0].view_from_game;
                gaze_generation = ssx3_game::game::main::ssxapp::set_vr_gaze([
                    view[0][2], view[1][2], view[2][2],
                ]);
                pose_history.record(eyes, &views);
            }
            current_eyes = eyes;
            gfx.set_vr_eyes(eyes);
        }
        let display_hz = 1_000_000_000.0 / frame.predicted_display_period.as_nanos() as f64;
        if let Some(error) = progress.error.lock().unwrap().clone() {
            return Err(error.into());
        }
        if game_worker.thread.as_ref().is_some_and(|t| t.is_finished()) {
            return Err("VR game worker stopped unexpectedly".into());
        }
        // A large turn can reveal course patches Nash culled for the old
        // gaze. Wait for its rendered frame before redrawing the retained
        // scene. Ordinary motion stays nonblocking.
        let visibility_jump = current_eyes
            .zip(gfx.present_vr_eyes())
            .is_some_and(|(now, drawn)| vr_camera::large_pose_jump(now, drawn));
        if visibility_jump
            && gaze_generation > last_gaze_waited
            && gfx.present_vr_visibility_epoch() < gaze_generation
        {
            last_gaze_waited = gaze_generation;
            let wait_start = Instant::now();
            if !wait_for_gaze_frame(&gfx, gaze_generation, Duration::from_millis(45)) {
                eprintln!("[vr] new gaze visibility not ready within 45 ms");
            }
            stages.elapsed(11, wait_start);
        }
        let visibility_ready = gfx.present_vr_visibility_epoch() >= gaze_generation;
        let image_serial = gfx.present_serial();
        let image_eyes = gfx.present_vr_eyes();
        let large_jump = current_eyes
            .zip(image_eyes)
            .is_some_and(|(now, drawn)| vr_camera::large_pose_jump(now, drawn));
        if frame.should_render
            && !no_submit
            && current_eyes.is_some_and(|eyes| {
                image_eyes.is_none_or(|drawn| vr_camera::eyes_moved(eyes, drawn, units_per_metre))
                    && (image_serial == last_present_serial || large_jump)
                    && (!large_jump || visibility_ready)
            })
        {
            let refresh_start = Instant::now();
            let _drawn = current_eyes.is_some_and(|eyes| {
                if large_jump {
                    gfx.refresh_vr_eyes_after_jump(eyes)
                } else {
                    gfx.refresh_vr_eyes_if_stale(eyes, image_serial)
                }
            });
            stages.elapsed(9, refresh_start);
        }
        let lock_start = Instant::now();
        gfx.with_queue_exclusive(|| -> Result<(), Box<dyn Error>> {
            stages.elapsed(2, lock_start);
            if let Some(age) = gfx.present_age_ms() {
                stages.push(3, age);
            }
            let serial = gfx.present_serial();
            let drawn_eyes = gfx.present_vr_eyes();
            if let (Some(now), Some(drawn)) = (current_eyes, drawn_eyes) {
                stages.push(10, vr_camera::rotation_delta_deg(now[0], drawn[0]) as f64);
            }
            if frame.should_render && !no_submit {
                let blit_start = Instant::now();
                let mut submitted_fresh_stereo = false;
                let mut presented_world = false;
                if clear_only {
                    target.clear_eyes(&graphics)?;
                } else if let (Some(hud), Some(eyes)) =
                    (gfx.present_vr_ui_view(), drawn_eyes.or(current_eyes))
                {
                    let stereo = gfx.present_stereo_views();
                    let sources = stereo.as_ref().map(|s| [&s[0].0, &s[1].0]);
                    presented_world = stereo.is_some();
                    let timing =
                        target.blit_stereo(&graphics, sources, &hud, panel_eyes(eyes), panel)?;
                    submitted_fresh_stereo = stereo.is_some() && serial != last_present_serial;
                    stages.push(4, timing.acquire_wait_ms);
                    stages.push(5, timing.encode_submit_ms);
                    stages.push(6, timing.gpu_wait_ms);
                    stages.push(7, timing.release_ms);
                } else {
                    target.clear_eyes(&graphics)?;
                }
                let xr_end_start = Instant::now();
                let submit_views: &[xr::View] = if presented_world {
                    drawn_eyes
                        .and_then(|drawn| pose_history.views_for(drawn))
                        .map(|pair| pair.as_slice())
                        .unwrap_or_else(|| {
                            pose_misses += 1;
                            &views
                        })
                } else {
                    &views
                };
                target.submit_frame(
                    &mut stream,
                    frame.predicted_display_time,
                    blend,
                    &local,
                    submit_views,
                )?;
                stages.elapsed(8, xr_end_start);
                submitted += 1;
                fresh_stereo += u64::from(submitted_fresh_stereo);
                blit_ms += blit_start.elapsed().as_secs_f64() * 1000.0;
                last_present_serial = serial;
            } else {
                stream.end(frame.predicted_display_time, blend, &[])?;
            }
            Ok(())
        })?;
        let frames = progress.frames.load(Ordering::Acquire);
        if game_rate_mark.elapsed() >= Duration::from_secs(5) {
            let elapsed = game_rate_mark.elapsed().as_secs_f64().max(1e-6);
            game_rate_mark = Instant::now();
            let ticks = frames - last_game_frames;
            let images = submitted - last_submitted;
            let rate = ticks as f64 / elapsed;
            let image_rate = images as f64 / elapsed;
            let fresh_rate = (fresh_stereo - last_fresh_stereo) as f64 / elapsed;
            let tick_ms =
                progress.tick_us.swap(0, Ordering::Relaxed) as f64 / ticks.max(1) as f64 / 1000.0;
            let publish_wait_us = gfx.publish_wait_us();
            let publish_wait_ms =
                (publish_wait_us - last_publish_wait_us) as f64 / ticks.max(1) as f64 / 1000.0;
            last_publish_wait_us = publish_wait_us;
            last_game_frames = frames;
            last_submitted = submitted;
            last_fresh_stereo = fresh_stereo;
            let status = progress.status.lock().unwrap().clone();
            let present = gfx
                .present_stereo_views()
                .map(|p| format!("{}x{} stereo", p[0].1, p[0].2))
                .unwrap_or_else(|| "none".into());
            eprintln!(
                "[vr {:.1}s] {} | game {:.1} Hz, OpenXR {:.1} Hz, submissions {:.1}/s, fresh stereo {:.1}/s, present {present}; tick {:.1} ms, publish wait {:.1} ms/tick, blit {:.1} ms",
                start.elapsed().as_secs_f64(),
                status,
                rate,
                display_hz,
                image_rate,
                fresh_rate,
                tick_ms,
                publish_wait_ms,
                blit_ms / images.max(1) as f64,
            );
            eprintln!("[vr stages]{}", stages.report_and_reset());
            if pose_misses != 0 {
                eprintln!("[vr] missing rendered OpenXR pose mappings: {pose_misses}");
                pose_misses = 0;
            }
            if let Ok(g) = gfx.0.try_lock() {
                eprintln!("[vr gfx] {}", g.timing_report());
            }
            blit_ms = 0.0;
        }
        if !requested_exit && exit_after.is_some_and(|n| frames >= n) {
            session.request_exit()?;
            requested_exit = true;
        }
    }
}

/// Exercise the same independent game/render workers and stereo refresh
/// without opening OpenXR. The caller supplies SSX3_ISO and SSX3_EVENT for
/// a race; this mode always mutes audio during the offline measurement.
fn run_offline_pipeline(frames_to_run: u64) -> Result<(), Box<dyn Error>> {
    std::env::set_var("SSX3_AUDIO", "0");
    configure_vr_game();
    let gpu = ssx3_ps2::gpu::context().ok_or("No GPU for offline VR pipeline")?;
    let (mut worker, gfx) = GameWorker::boot()?;
    let eye_size = std::env::var("SSX3_VR_EYE_SIZE")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|&v| (512..=3840).contains(&v))
        .unwrap_or(1024);
    let compositor = xr_wgpu_session::OfflineStereoTarget::new(&gpu.device, eye_size, eye_size);
    let units_per_metre = vr_units_per_metre();
    let panel = vr_panel_config(units_per_metre);
    gfx.set_output_size(Some((eye_size, eye_size)));
    let projection = ssx3_gfx::vr::projection(-1.0, 1.0, -1.0, 1.0, 1.0, 30_000.0);
    let mut eyes = std::array::from_fn(|i| {
        let mut view = ssx3_gfx::vr::IDENTITY;
        view[3][0] = (if i == 0 { 0.032 } else { -0.032 }) * units_per_metre;
        ssx3_gfx::vr::Eye {
            view_from_game: view,
            projection,
        }
    });
    gfx.set_vr_eyes(Some(eyes));
    let mut gaze_generation = ssx3_game::game::main::ssxapp::set_vr_gaze([0.0, 0.0, 1.0]);
    ssx3_ps2::machine::set_realtime(true);
    worker.start()?;
    worker.progress.running.store(true, Ordering::Release);
    let started = Instant::now();
    let period = Duration::from_nanos(13_888_889); // local PICO: 72 Hz
    let timeout = Duration::from_secs(frames_to_run / 20 + 30);
    let mut next = Instant::now();
    let mut last_serial = 0;
    let mut submitted = 0u64;
    let mut fresh = 0u64;
    let mut refreshes = 0u64;
    let mut refresh_ms = 0.0;
    let mut gaze_waits = 0u64;
    let mut gaze_wait_ms = 0.0;
    let mut gaze_wait_misses = 0u64;
    let mut last_gaze_waited = 0u64;
    let mut lock_ms = 0.0;
    let mut gpu_wait_ms = 0.0;
    let mut pending_max = 0usize;
    let mut race_mark = None;
    let mut pose_errors = Vec::new();
    let mut fresh_intervals_ms = Vec::new();
    let mut present_ages_ms = Vec::new();
    let mut last_fresh_at: Option<Instant> = None;
    let turn_at = std::env::var("SSX3_VR_OFFLINE_TURN_AT")
        .ok()
        .and_then(|v| v.parse::<u64>().ok());
    let target_yaw = std::env::var("SSX3_VR_OFFLINE_TURN_DEG")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|v| v.is_finite())
        .unwrap_or(90.0);
    let sweep_dps = std::env::var("SSX3_VR_OFFLINE_SWEEP_DPS")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|v| v.is_finite() && *v > 0.0);
    let mut turn_started = None;
    let mut turned = false;
    while worker.progress.frames.load(Ordering::Acquire) < frames_to_run {
        if started.elapsed() > timeout {
            return Err("Offline VR pipeline timed out".into());
        }
        if let Some(error) = worker.progress.error.lock().unwrap().clone() {
            return Err(error.into());
        }
        if worker.thread.as_ref().is_some_and(|t| t.is_finished()) {
            return Err("Offline VR game worker stopped unexpectedly".into());
        }
        let game_frame = worker.progress.frames.load(Ordering::Acquire);
        if turn_at.is_some_and(|at| game_frame >= at) && (!turned || sweep_dps.is_some()) {
            let began = *turn_started.get_or_insert_with(Instant::now);
            let yaw_deg = sweep_dps.map_or(target_yaw, |speed| {
                (speed * began.elapsed().as_secs_f32()).min(target_yaw.abs()) * target_yaw.signum()
            });
            let yaw = yaw_deg.to_radians();
            let (s, c) = yaw.sin_cos();
            for eye in &mut eyes {
                eye.view_from_game[0] = [c, 0.0, s, 0.0];
                eye.view_from_game[2] = [-s, 0.0, c, 0.0];
            }
            gfx.set_vr_eyes(Some(eyes));
            gaze_generation = ssx3_game::game::main::ssxapp::set_vr_gaze([s, 0.0, c]);
            turned = true;
        }
        let now = Instant::now();
        if next < now {
            next = now;
        }
        std::thread::sleep(next.saturating_duration_since(now));
        next += period;
        pending_max = pending_max.max(gfx.pending_frames());
        let visibility_jump = gfx
            .present_vr_eyes()
            .is_some_and(|drawn| vr_camera::large_pose_jump(eyes, drawn));
        if visibility_jump
            && gaze_generation > last_gaze_waited
            && gfx.present_vr_visibility_epoch() < gaze_generation
        {
            last_gaze_waited = gaze_generation;
            let wait_start = Instant::now();
            gaze_waits += 1;
            gaze_wait_misses += u64::from(!wait_for_gaze_frame(
                &gfx,
                gaze_generation,
                Duration::from_millis(45),
            ));
            gaze_wait_ms += wait_start.elapsed().as_secs_f64() * 1000.0;
        }
        let visibility_ready = gfx.present_vr_visibility_epoch() >= gaze_generation;
        let image_serial = gfx.present_serial();
        let image_eyes = gfx.present_vr_eyes();
        let large_jump = image_eyes.is_some_and(|drawn| vr_camera::large_pose_jump(eyes, drawn));
        if image_eyes.is_none_or(|drawn| vr_camera::eyes_moved(eyes, drawn, units_per_metre))
            && (image_serial == last_serial || large_jump)
            && (!large_jump || visibility_ready)
        {
            let t = Instant::now();
            refreshes += u64::from(if large_jump {
                gfx.refresh_vr_eyes_after_jump(eyes)
            } else {
                gfx.refresh_vr_eyes_if_stale(eyes, image_serial)
            });
            refresh_ms += t.elapsed().as_secs_f64() * 1000.0;
        }
        let lock_start = Instant::now();
        gfx.with_queue_exclusive(|| -> Result<(), Box<dyn Error>> {
            lock_ms += lock_start.elapsed().as_secs_f64() * 1000.0;
            let serial = gfx.present_serial();
            let stereo = gfx.present_stereo_views();
            let hud = gfx.present_vr_ui_view();
            if let Some(hud) = hud.as_ref() {
                let rendered_eyes = gfx.present_vr_eyes();
                if game_frame >= 420 && stereo.is_some() {
                    if let Some(drawn) = rendered_eyes {
                        pose_errors.push(vr_camera::rotation_delta_deg(eyes[0], drawn[0]));
                    }
                    if let Some(age) = gfx.present_age_ms() {
                        present_ages_ms.push(age);
                    }
                }
                gpu_wait_ms += compositor.blit_stereo(
                    &gpu.device,
                    &gpu.queue,
                    stereo.as_ref().map(|s| [&s[0].0, &s[1].0]),
                    hud,
                    panel_eyes(rendered_eyes.unwrap_or(eyes)),
                    panel,
                )?;
                submitted += 1;
                let is_fresh = stereo.is_some() && serial != last_serial;
                fresh += u64::from(is_fresh);
                if game_frame >= 420 && is_fresh {
                    let now = Instant::now();
                    if let Some(previous) = last_fresh_at.replace(now) {
                        fresh_intervals_ms
                            .push(now.duration_since(previous).as_secs_f64() * 1000.0);
                    }
                }
                last_serial = serial;
            } else {
                let t = Instant::now();
                let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
                gpu_wait_ms += t.elapsed().as_secs_f64() * 1000.0;
            }
            Ok(())
        })?;
        let frame = worker.progress.frames.load(Ordering::Acquire);
        if frame >= 420 && race_mark.is_none() {
            race_mark = Some((
                Instant::now(),
                frame,
                submitted,
                fresh,
                refreshes,
                lock_ms,
                gpu_wait_ms,
                gfx.publish_wait_us(),
            ));
        }
    }
    worker.progress.running.store(false, Ordering::Release);
    worker.progress.stop.store(true, Ordering::Release);
    if let Some(thread) = worker.thread.take() {
        thread
            .join()
            .map_err(|_| "Offline VR game worker panicked")?;
    }
    gfx.flush();
    if std::env::var_os("SSX3_FRAME_DUMP").is_some() {
        if let Some(prefix) = std::env::var_os("SSX3_VR_OFFLINE_SAVE") {
            let dir = format!("{}-dump", prefix.to_string_lossy());
            std::fs::create_dir_all(&dir)?;
            ssx3_ps2::frame_sink::write_dumps(&dir)?;
        }
    }
    let frames = worker.progress.frames.load(Ordering::Acquire);
    let elapsed = started.elapsed().as_secs_f64();
    let images = gfx.stats().frames;
    let game_ms = worker.progress.tick_us.load(Ordering::Relaxed) as f64 / 1000.0;
    let status = worker.progress.status.lock().unwrap().clone();
    println!(
        "offline VR pipeline: game {frames} ticks ({:.1}/s, {:.1}ms CPU/tick), renderer {images} frames, presented {submitted} stereo ({:.1}/s), fresh {fresh} ({:.1}/s), {}x{} per eye, {:.2}s; refresh {refreshes} ({:.1}ms total), gaze waits {gaze_waits} ({gaze_wait_ms:.1}ms, {gaze_wait_misses} misses), publish wait {:.1}ms, lock {:.1}ms, GPU wait {:.1}ms, pending max {pending_max}, status {status:?}",
        frames as f64 / elapsed,
        game_ms / frames.max(1) as f64,
        submitted as f64 / elapsed,
        fresh as f64 / elapsed,
        eye_size,
        eye_size,
        elapsed,
        refresh_ms,
        gfx.publish_wait_us() as f64 / 1000.0,
        lock_ms,
        gpu_wait_ms,
    );
    if let Some((at, first, shown, new, views, locked, waited, publish_waited)) = race_mark {
        let seconds = at.elapsed().as_secs_f64().max(1e-6);
        println!(
            "offline frames {first}..{frames}: game {:.1}/s, stereo {:.1}/s, fresh {:.1}/s, refresh {}, publish wait {:.1}ms, lock {:.1}ms, GPU wait {:.1}ms",
            (frames - first) as f64 / seconds,
            (submitted - shown) as f64 / seconds,
            (fresh - new) as f64 / seconds,
            refreshes - views,
            (gfx.publish_wait_us() - publish_waited) as f64 / 1000.0,
            lock_ms - locked,
            gpu_wait_ms - waited,
        );
    }
    println!("offline renderer: {}", gfx.timing_report());
    let quantiles = |values: &mut Vec<f64>| {
        if values.is_empty() {
            return None;
        }
        values.sort_by(f64::total_cmp);
        let p95 = ((values.len() * 95).div_ceil(100) - 1).min(values.len() - 1);
        Some((
            values[values.len() / 2],
            values[p95],
            values[values.len() - 1],
        ))
    };
    if let (Some((interval_p50, interval_p95, interval_max)), Some((age_p50, age_p95, age_max))) = (
        quantiles(&mut fresh_intervals_ms),
        quantiles(&mut present_ages_ms),
    ) {
        println!(
            "offline race image timing: fresh interval p50 {interval_p50:.2}ms p95 {interval_p95:.2}ms max {interval_max:.2}ms ({} intervals); presented age p50 {age_p50:.2}ms p95 {age_p95:.2}ms max {age_max:.2}ms ({} images)",
            fresh_intervals_ms.len(),
            present_ages_ms.len(),
        );
    }
    if !pose_errors.is_empty() {
        pose_errors.sort_by(f32::total_cmp);
        let count = pose_errors.len();
        let average = pose_errors.iter().sum::<f32>() / count as f32;
        println!(
            "offline eye rotation lag: average {:.2}°, p95 {:.2}°, max {:.2}° over {count} presented images",
            average,
            pose_errors[count * 95 / 100],
            pose_errors[count - 1],
        );
    }
    if turned {
        // Move the head once more after the simulation has stopped: this
        // must redraw the retained scene without another game tick.
        let yaw = (target_yaw + 5.0).to_radians();
        let (s, c) = yaw.sin_cos();
        for eye in &mut eyes {
            eye.view_from_game[0] = [c, 0.0, s, 0.0];
            eye.view_from_game[2] = [-s, 0.0, c, 0.0];
        }
        let serial = gfx.present_serial();
        if !gfx.refresh_vr_eyes_if_stale(eyes, serial) {
            return Err("Offline retained-scene head refresh failed".into());
        }
        let [left, right] = gfx.read_stereo_images().ok_or("No offline stereo image")?;
        if let Some(prefix) = std::env::var_os("SSX3_VR_OFFLINE_SAVE") {
            let prefix = prefix.to_string_lossy();
            std::fs::write(
                format!("{prefix}-world-left.png"),
                png::encode(left.0, left.1, &left.2),
            )?;
            std::fs::write(
                format!("{prefix}-world-right.png"),
                png::encode(right.0, right.1, &right.2),
            )?;
        }
        let nonblack = |pixels: &[u32]| pixels.iter().filter(|&&p| p & 0x00ff_ffff != 0).count();
        let left_nonblack = nonblack(&left.2);
        let right_nonblack = nonblack(&right.2);
        let different = left.2.iter().zip(&right.2).filter(|(a, b)| a != b).count();
        println!(
            "offline head turn + retained refresh: left nonblack {}/{}, right nonblack {}/{}, eyes differ at {} pixels",
            left_nonblack,
            left.2.len(),
            right_nonblack,
            right.2.len(),
            different,
        );
        if left_nonblack < left.2.len() * 9 / 10
            || right_nonblack < right.2.len() * 9 / 10
            || different < left.2.len() / 20
        {
            return Err("Offline head turn produced an empty or duplicated eye".into());
        }
        let stereo = gfx.present_stereo_views().ok_or("No final stereo views")?;
        let hud = gfx.present_vr_ui_view().ok_or("No final spatial HUD")?;
        compositor.blit_stereo(
            &gpu.device,
            &gpu.queue,
            Some([&stereo[0].0, &stereo[1].0]),
            &hud,
            panel_eyes(eyes),
            panel,
        )?;
        let [left_eye, right_eye] = compositor.read_eyes(&gpu.device, &gpu.queue)?;
        let left = nonblack(&left_eye.2);
        let right = nonblack(&right_eye.2);
        let different = left_eye
            .2
            .iter()
            .zip(&right_eye.2)
            .filter(|(a, b)| a != b)
            .count();
        let total = left_eye.2.len();
        println!(
            "offline OpenXR-style composite: left nonblack {left}/{total}, right nonblack {right}/{total}, eyes differ at {different} pixels"
        );
        if let Some(prefix) = std::env::var_os("SSX3_VR_OFFLINE_SAVE") {
            let prefix = prefix.to_string_lossy();
            std::fs::write(
                format!("{prefix}-left.png"),
                png::encode(left_eye.0, left_eye.1, &left_eye.2),
            )?;
            std::fs::write(
                format!("{prefix}-right.png"),
                png::encode(right_eye.0, right_eye.1, &right_eye.2),
            )?;
        }
        if left < total * 9 / 10 || right < total * 9 / 10 || different < total / 20 {
            return Err("Offline compositor produced an empty or duplicated eye".into());
        }
    } else if let Some(prefix) = std::env::var_os("SSX3_VR_OFFLINE_SAVE") {
        let hud = gfx.present_vr_ui_view().ok_or("No final spatial UI")?;
        let stereo = gfx.present_stereo_views();
        compositor.blit_stereo(
            &gpu.device,
            &gpu.queue,
            stereo.as_ref().map(|s| [&s[0].0, &s[1].0]),
            &hud,
            panel_eyes(gfx.present_vr_eyes().unwrap_or(eyes)),
            panel,
        )?;
        let [left, right] = compositor.read_eyes(&gpu.device, &gpu.queue)?;
        let nonblack = |pixels: &[u32]| pixels.iter().filter(|&&p| p & 0x00ff_ffff != 0).count();
        println!(
            "offline final card: world {}, left nonblack {}/{}, right nonblack {}/{}",
            stereo.is_some(),
            nonblack(&left.2),
            left.2.len(),
            nonblack(&right.2),
            right.2.len(),
        );
        if nonblack(&left.2) == 0 || nonblack(&right.2) == 0 {
            return Err("Offline final UI is blank in an eye".into());
        }
        let prefix = prefix.to_string_lossy();
        std::fs::write(
            format!("{prefix}-left.png"),
            png::encode(left.0, left.1, &left.2),
        )?;
        std::fs::write(
            format!("{prefix}-right.png"),
            png::encode(right.0, right.1, &right.2),
        )?;
    }
    Ok(())
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let result = if args.get(1).is_some_and(|arg| arg == "--offline-pipeline") {
        let frames = args
            .get(2)
            .and_then(|n| n.parse::<u64>().ok())
            .unwrap_or(500);
        run_offline_pipeline(frames)
    } else {
        run()
    };
    if let Err(err) = result {
        eprintln!("SSX 3 VR: {err}");
        std::process::exit(1);
    }
}
