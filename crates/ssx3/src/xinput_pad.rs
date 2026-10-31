//! Direct Windows XInput polling for the VR game's first conventional pad.
//! WGI can enumerate a controller in SteamVR without delivering its readings;
//! XInput exposes the same Xbox-compatible pad independently of window focus.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pad {
    pub slot: u32,
    /// libpad button bits, active high at the host input boundary.
    pub buttons: u16,
    /// DualShock right X/Y, left X/Y (0x80 is centred).
    pub sticks: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct XInputGamepad {
    buttons: u16,
    left_trigger: u8,
    right_trigger: u8,
    lx: i16,
    ly: i16,
    rx: i16,
    ry: i16,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct XInputState {
    packet: u32,
    gamepad: XInputGamepad,
}

type GetState = unsafe extern "system" fn(user_index: u32, state: *mut XInputState) -> u32;

struct Api {
    _library: libloading::Library,
    get_state: GetState,
}

fn api() -> Option<&'static Api> {
    static API: std::sync::OnceLock<Option<Api>> = std::sync::OnceLock::new();
    API.get_or_init(|| {
        // The DLL stays loaded as long as this function pointer can be used.
        let library = unsafe { libloading::Library::new("xinput1_4.dll") }.ok()?;
        let get_state = unsafe { *library.get::<GetState>(b"XInputGetState").ok()? };
        Some(Api {
            _library: library,
            get_state,
        })
    })
    .as_ref()
}

const LEFT_DEADZONE: i32 = 7849;
const RIGHT_DEADZONE: i32 = 8689;
const TRIGGER_THRESHOLD: u8 = 30;

fn axis(raw: i16, invert: bool) -> u8 {
    let value = (raw as f32 / 32767.0).clamp(-1.0, 1.0);
    let value = if invert { -value } else { value };
    (value * 127.5 + 127.5).round() as u8
}

fn stick_pair(x: i16, y: i16, deadzone: i32) -> [u8; 2] {
    let x32 = x as i32;
    let y32 = y as i32;
    if x32 * x32 + y32 * y32 <= deadzone * deadzone {
        [0x80, 0x80]
    } else {
        [axis(x, false), axis(y, true)]
    }
}

fn translate(slot: u32, pad: XInputGamepad) -> Pad {
    let mut buttons = 0u16;
    for (xinput, libpad) in [
        (0x0020, 1 << 0),  // Back -> Select
        (0x0040, 1 << 1),  // Left stick click
        (0x0080, 1 << 2),  // Right stick click
        (0x0010, 1 << 3),  // Start
        (0x0001, 1 << 4),  // D-pad up
        (0x0008, 1 << 5),  // D-pad right
        (0x0002, 1 << 6),  // D-pad down
        (0x0004, 1 << 7),  // D-pad left
        (0x0100, 1 << 10), // Left shoulder
        (0x0200, 1 << 11), // Right shoulder
        (0x8000, 1 << 12), // Y -> Triangle
        (0x2000, 1 << 13), // B -> Circle
        (0x1000, 1 << 14), // A -> Cross
        (0x4000, 1 << 15), // X -> Square
    ] {
        if pad.buttons & xinput != 0 {
            buttons |= libpad;
        }
    }
    if pad.left_trigger > TRIGGER_THRESHOLD {
        buttons |= 1 << 8;
    }
    if pad.right_trigger > TRIGGER_THRESHOLD {
        buttons |= 1 << 9;
    }
    let right = stick_pair(pad.rx, pad.ry, RIGHT_DEADZONE);
    let left = stick_pair(pad.lx, pad.ly, LEFT_DEADZONE);
    Pad {
        slot,
        buttons,
        sticks: [right[0], right[1], left[0], left[1]],
    }
}

pub fn first_connected() -> Option<Pad> {
    let api = api()?;
    for slot in 0..4 {
        let mut state = XInputState::default();
        // The Windows API writes `state` on success and returns 0.
        if unsafe { (api.get_state)(slot, &mut state) } == 0 {
            return Some(translate(slot, state.gamepad));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xbox_buttons_and_sticks_match_the_desktop_libpad_layout() {
        let pad = translate(
            0,
            XInputGamepad {
                buttons: 0x1000 | 0x0001 | 0x0200,
                left_trigger: 31,
                right_trigger: 30,
                lx: 32767,
                ly: 32767,
                rx: 0,
                ry: 0,
            },
        );
        assert_eq!(pad.buttons, (1 << 14) | (1 << 4) | (1 << 11) | (1 << 8));
        assert_eq!(pad.sticks, [0x80, 0x80, 0xff, 0]);
        assert_eq!(translate(2, XInputGamepad::default()).sticks, [0x80; 4]);
        assert_eq!(stick_pair(100, -100, LEFT_DEADZONE), [0x80; 2]);
    }
}
