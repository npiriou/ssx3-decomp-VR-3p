//! Experimental SteamVR front end for the existing native SSX 3 game.
//! The current image handoff is a flat game panel in each eye; the renderer's
//! two distinct world views are the next step before this is a VR prototype.

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
    game::init_standalone();
    settings::load();
    // VR's first target is fixed at 60 game updates per second. Display
    // refresh follows OpenXR and may be higher or lower than 60 Hz.
    std::env::set_var("SSX3_TICK_HZ", "60");
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
    let mut game_rate_mark = Instant::now();
    let mut last_submitted = 0u64;
    let mut events = xr::EventDataBuffer::new();
    let mut running = false;
    let mut requested_exit = false;
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
        let frame = waiter.wait()?;
        stream.begin()?;
        let (_flags, views) =
            session.locate_views(VIEW_TYPE, frame.predicted_display_time, &local)?;
        let game_hz = ssx3_ps2::machine::vsync_hz() as f64 / 1.001;
        let display_hz = 1_000_000_000.0 / frame.predicted_display_period.as_nanos() as f64;
        // The predicted period can briefly say 72 Hz while an idle HMD
        // wakes this app at 14.4 Hz. Cap wall-clock debt independently of
        // that prediction so the game still advances at 60 Hz.
        let plan = pacer.plan(start.elapsed().as_secs_f64(), 1.0 / game_hz, 8, true);
        ssx3_ps2::machine::set_audio_lead_frames((pacer.audio_lead() * 48000.0) as u32);
        ssx3_game::tick::set_updates_per_vsync(plan.updates_per_vsync.max(1));
        for _ in 0..plan.vsyncs {
            update_pad(&mut gilrs, &mut cpu);
            if !ssx3_ps2::machine::run_frame(&mut cpu) {
                return Err(format!("Game halted: {:?}", ssx3_ps2::m!(cpu).kernel.halted).into());
            }
            frames += 1;
            if frames % 60 == 0 {
                let elapsed = game_rate_mark.elapsed().as_secs_f64().max(1e-6);
                game_rate_mark = Instant::now();
                let rate = 60.0 / elapsed;
                let image_rate = (submitted - last_submitted) as f64 / elapsed;
                last_submitted = submitted;
                let stats = gfx.stats();
                let present = gfx
                    .present_view()
                    .map(|(_, w, h)| format!("{w}x{h}"))
                    .unwrap_or_else(|| "none".into());
                eprintln!(
                    "[vr {:.1}s] {} | game {:.1} Hz, OpenXR {:.1} Hz, images {:.1}/s, gfx frames {}, present {present}",
                    start.elapsed().as_secs_f64(),
                    game::status(&cpu),
                    rate,
                    display_hz,
                    image_rate,
                    stats.frames,
                );
            }
        }
        // OpenXR's Vulkan binding may touch the same VkQueue in acquire,
        // release and end_frame. Vulkan requires host-side synchronization
        // against the renderer worker's queue submissions. Until the queue
        // is wrapped in a shared lock, finish its frame before those calls.
        gfx.flush();
        if frame.should_render && !no_submit {
            if clear_only {
                target.clear_eyes(&graphics)?;
            } else if let Some((view, _, _)) = gfx.present_view() {
                target.blit_game(&graphics, &view)?;
            } else {
                target.clear_eyes(&graphics)?;
            }
            target.submit_frame(
                &mut stream,
                frame.predicted_display_time,
                blend,
                &local,
                &views,
            )?;
            submitted += 1;
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
