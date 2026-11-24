//! Windows message loop of the tray process: a hidden window, a slow timer that refreshes the tooltip, and the
//! quit message other threads post. Nothing here polls fast.

use std::sync::atomic::{AtomicIsize, Ordering};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer, PostMessageW, PostQuitMessage, RegisterClassW, SetTimer, TranslateMessage, MSG,
    WNDCLASSW,
};

const WM_TIMER: u32 = 0x0113;
const WM_ENDSESSION: u32 = 0x0016;
const WM_QUERYENDSESSION: u32 = 0x0011;
const WM_APP: u32 = 0x8000;
pub const WM_QUIT_REQUEST: u32 = WM_APP + 1;
const TIMER_ID: usize = 1;
/// How often the tray tooltip is refreshed.
const TICK_MS: u32 = 2000;

static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            crate::background::on_tick();
            0
        }
        WM_QUERYENDSESSION => 1,
        WM_ENDSESSION => {
            if wp != 0 {
                crate::background::on_shutdown();
            }
            0
        }
        WM_QUIT_REQUEST => {
            // SAFETY: plain Win32 calls on our own window.
            unsafe {
                KillTimer(hwnd, TIMER_ID);
                DestroyWindow(hwnd);
                PostQuitMessage(0);
            }
            0
        }
        // SAFETY: forwarding to the default procedure with the original arguments.
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Asks the message loop to end (from any thread).
pub fn request_quit() {
    let h = MAIN_HWND.load(Ordering::Relaxed);
    if h != 0 {
        // SAFETY: posting to a window handle of this process.
        unsafe { PostMessageW(h as HWND, WM_QUIT_REQUEST, 0, 0) };
    }
}

/// Runs until told to quit. `on_ready` runs once the window exists (the tray is created there, on this thread).
pub fn run(on_ready: impl FnOnce()) {
    // SAFETY: standard window class registration and message loop; every pointer outlives its use.
    unsafe {
        let hinst = GetModuleHandleW(std::ptr::null());
        let class = wide("ThermoraBackgroundWindow");
        let wc = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class.as_ptr(),
        };
        RegisterClassW(&wc);
        let title = wide("Thermora background");
        let hwnd = CreateWindowExW(0, class.as_ptr(), title.as_ptr(), 0, 0, 0, 0, 0, std::ptr::null_mut(), std::ptr::null_mut(), hinst, std::ptr::null());
        if hwnd.is_null() {
            return;
        }
        MAIN_HWND.store(hwnd as isize, Ordering::Relaxed);
        SetTimer(hwnd, TIMER_ID, TICK_MS, None);
        on_ready();
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
