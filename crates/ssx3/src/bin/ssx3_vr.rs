//! Experimental SteamVR front end for the existing native SSX 3 game.
//! The game's chase camera is reprojected into two tracked scene views.

#[cfg(all(feature = "ee-threads", not(feature = "ee-unwind")))]
extern crate ssx3_ee_threads as ssx3_ee;
#[cfg(feature = "ee-unwind")]
extern crate ssx3_ee_unwind as ssx3_ee;

use std::error::Error;
use std::time::{Duration, Instant};

use gilrs::{Axis, Button, Gilrs};
use ssx3_vr::openxr as xr;
use ssx3_vr::xr_wgpu;
use ssx3_vr::xr_wgpu_session::{self, StereoTarget, VIEW_TYPE};

const STAGE_NAMES: [&str; 10] = [
    "xr_wait",
    "xr_begin",
    "game_tick",
    "gfx_flush",
    "xr_acquire",
    "blit_encode",
    "queue_wait",
    "xr_release",
    "xr_end",
    "view_refresh",
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
            out.push_str(&format!(
                " {name}[n={n} p50={:.2} p95={:.2} max={:.2}ms]",
                values[n / 2],
                values[p95],
                values[n - 1]
            ));
            values.clear();
        }
        out
    }
}

#[path = "../vr_camera.rs"]
mod vr_camera;

#[allow(dead_code)]
#[path = "../game.rs"]
mod game;
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

fn run() -> Result<(), Box<dyn Error>> {
    // Ask Nash to retain nearby world nodes in every head direction before
    // the typed frame is published. Desktop/headless runs keep their cull.
    std::env::set_var("SSX3_VR_VISIBILITY", "1");
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
    let mut cpu = game::boot(true);
    let gfx = game::install_renderer(&mut cpu);
    if gfx.is_none() {
        return Err("Native game renderer did not start on the OpenXR GPU".into());
    }
    let gfx = gfx.unwrap();
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
    let mut gilrs = Gilrs::new().map(Some).unwrap_or_else(|err| {
        eprintln!("[vr pad] gamepad API unavailable: {err}");
        None
    });
    let exit_after = std::env::var("SSX3_EXIT_AFTER_FRAMES")
        .ok()
        .and_then(|v| v.parse::<u64>().ok());
    let no_submit = std::env::var_os("SSX3_VR_NO_SUBMIT").is_some();
    let clear_only = std::env::var_os("SSX3_VR_CLEAR_ONLY").is_some();
    let start = Instant::now();
    let mut pacer = ssx3_game::tick::Pacer::default();
    let mut frames = 0u64;
    let mut submitted = 0u64;
    let mut fresh_stereo = 0u64;
    let mut last_fresh_stereo = 0u64;
    let mut last_gfx_frames = 0u64;
    let mut game_rate_mark = Instant::now();
    let mut last_submitted = 0u64;
    let mut run_ms = 0.0f64;
    let mut flush_ms = 0.0f64;
    let mut blit_ms = 0.0f64;
    let mut stages = StageWindow::new();
    let mut events = xr::EventDataBuffer::new();
    let mut running = false;
    let mut requested_exit = false;
    let mut anchor: Option<vr_camera::Anchor> = None;
    let mut current_eyes: Option<[ssx3_gfx::vr::Eye; 2]> = None;
    let units_per_metre = std::env::var("SSX3_VR_UNITS_PER_M")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(100.0);
    let ui_setting = |name: &str, default: f32, min: f32, max: f32| {
        std::env::var(name)
            .ok()
            .and_then(|s| s.parse::<f32>().ok())
            .filter(|v| v.is_finite() && (min..=max).contains(v))
            .unwrap_or(default)
    };
    let panel = xr_wgpu_session::PanelConfig {
        width: ui_setting("SSX3_VR_UI_WIDTH_M", 1.3, 0.5, 3.0) * units_per_metre,
        height: ui_setting("SSX3_VR_UI_HEIGHT_M", 1.3, 0.5, 3.0) * units_per_metre,
        distance: ui_setting("SSX3_VR_UI_DISTANCE_M", 2.0, 1.0, 5.0) * units_per_metre,
        y: ui_setting("SSX3_VR_UI_HEIGHT_OFFSET_M", 0.0, -1.0, 1.0) * units_per_metre,
    };
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
                            session.begin(VIEW_TYPE)?;
                            running = true;
                        }
                        xr::SessionState::FOCUSED => {
                            anchor = None;
                        }
                        xr::SessionState::STOPPING => {
                            session.end()?;
                            running = false;
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
        stream.begin()?;
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
                ssx3_game::game::main::ssxapp::set_vr_gaze([view[0][2], view[1][2], view[2][2]]);
            }
            current_eyes = eyes;
            gfx.set_vr_eyes(eyes);
        }
        let game_hz = ssx3_ps2::machine::vsync_hz() as f64 / 1.001;
        let display_hz = 1_000_000_000.0 / frame.predicted_display_period.as_nanos() as f64;
        // The predicted period can briefly say 72 Hz while an idle HMD
        // wakes this app at 14.4 Hz. Cap wall-clock debt independently of
        // that prediction so the game still advances at 60 Hz.
        let plan = pacer.plan(start.elapsed().as_secs_f64(), 1.0 / game_hz, 8, true);
        ssx3_ps2::machine::set_audio_lead_frames((pacer.audio_lead() * 48000.0) as u32);
        ssx3_game::tick::set_updates_per_vsync(plan.updates_per_vsync.max(1));
        for _ in 0..plan.vsyncs {
            let run_start = Instant::now();
            update_pad(&mut gilrs, &mut cpu);
            if !ssx3_ps2::machine::run_frame(&mut cpu) {
                return Err(format!("Game halted: {:?}", ssx3_ps2::m!(cpu).kernel.halted).into());
            }
            frames += 1;
            run_ms += run_start.elapsed().as_secs_f64() * 1000.0;
            stages.elapsed(2, run_start);
            if frames % 60 == 0 {
                let elapsed = game_rate_mark.elapsed().as_secs_f64().max(1e-6);
                game_rate_mark = Instant::now();
                let rate = 60.0 / elapsed;
                let image_rate = (submitted - last_submitted) as f64 / elapsed;
                let fresh_rate = (fresh_stereo - last_fresh_stereo) as f64 / elapsed;
                last_submitted = submitted;
                last_fresh_stereo = fresh_stereo;
                let stats = gfx.stats();
                let present = gfx
                    .present_stereo_views()
                    .map(|p| format!("{}x{} stereo", p[0].1, p[0].2))
                    .unwrap_or_else(|| "none".into());
                eprintln!(
                    "[vr {:.1}s] {} | game {:.1} Hz, OpenXR {:.1} Hz, submissions {:.1}/s, fresh stereo {:.1}/s, gfx frames {}, present {present}; run {:.1} ms/frame, flush {:.1} ms, blit {:.1} ms",
                    start.elapsed().as_secs_f64(),
                    game::status(&cpu),
                    rate,
                    display_hz,
                    image_rate,
                    fresh_rate,
                    stats.frames,
                    run_ms / 60.0,
                    flush_ms,
                    blit_ms,
                );
                eprintln!("[vr stages]{}", stages.report_and_reset());
                eprintln!("[vr gfx] {}", gfx.0.lock().unwrap().timing_report());
                run_ms = 0.0;
                flush_ms = 0.0;
                blit_ms = 0.0;
            }
        }
        // OpenXR's Vulkan binding may touch the same VkQueue in acquire,
        // release and end_frame. Vulkan requires host-side synchronization
        // against the renderer worker's queue submissions. Until the queue
        // is wrapped in a shared lock, finish its frame before those calls.
        let flush_start = Instant::now();
        gfx.flush();
        flush_ms += flush_start.elapsed().as_secs_f64() * 1000.0;
        stages.elapsed(3, flush_start);
        let built_frames = gfx.stats().frames;
        let new_game_image = built_frames > last_gfx_frames;
        last_gfx_frames = built_frames;
        // At headset rates above the 60 Hz game tick, submit a new view of
        // the retained scene with this frame's tracked eye poses. The VIF/GS
        // stream and gameplay simulation are not replayed.
        let refreshed = if plan.vsyncs == 0 && frame.should_render && !no_submit {
            let refresh_start = Instant::now();
            let drawn = current_eyes.is_some_and(|eyes| gfx.refresh_vr_eyes(eyes));
            stages.elapsed(9, refresh_start);
            drawn
        } else {
            false
        };
        if frame.should_render && !no_submit {
            let blit_start = Instant::now();
            let mut submitted_fresh_stereo = false;
            if clear_only {
                target.clear_eyes(&graphics)?;
            } else if let (Some(hud), Some(eyes)) = (gfx.present_vr_ui_view(), current_eyes) {
                let stereo = gfx.present_stereo_views();
                let sources = stereo.as_ref().map(|s| [&s[0].0, &s[1].0]);
                let panel_eyes = eyes.map(|eye| xr_wgpu_session::PanelEye {
                    game_from_eye: ssx3_gfx::vr::inverse(eye.view_from_game)
                        .unwrap_or(ssx3_gfx::vr::IDENTITY),
                    clip: [
                        eye.projection[0][0],
                        eye.projection[1][1],
                        eye.projection[2][0],
                        eye.projection[2][1],
                    ],
                });
                let timing = target.blit_stereo(&graphics, sources, &hud, panel_eyes, panel)?;
                submitted_fresh_stereo = stereo.is_some() && (new_game_image || refreshed);
                stages.push(4, timing.acquire_wait_ms);
                stages.push(5, timing.encode_submit_ms);
                stages.push(6, timing.gpu_wait_ms);
                stages.push(7, timing.release_ms);
            } else {
                target.clear_eyes(&graphics)?;
            }
            let xr_end_start = Instant::now();
            target.submit_frame(
                &mut stream,
                frame.predicted_display_time,
                blend,
                &local,
                &views,
            )?;
            stages.elapsed(8, xr_end_start);
            submitted += 1;
            fresh_stereo += u64::from(submitted_fresh_stereo);
            blit_ms += blit_start.elapsed().as_secs_f64() * 1000.0;
        } else {
            stream.end(frame.predicted_display_time, blend, &[])?;
        }
        if !requested_exit && exit_after.is_some_and(|n| frames >= n) {
            session.request_exit()?;
            requested_exit = true;
        }
    }
}

fn main() {
    if let Err(err) = run() {
        eprintln!("SSX 3 VR: {err}");
        std::process::exit(1);
    }
}
