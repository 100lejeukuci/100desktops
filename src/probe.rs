//! Test helper: a small topmost window that prints the input it receives, so remote control can be verified
//! end to end without touching any other application. Client area 400x300 at screen position (100,100).

use anyhow::Result;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetStockObject, HBRUSH, WHITE_BRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let x = (lp.0 & 0xFFFF) as i16;
        let y = ((lp.0 >> 16) & 0xFFFF) as i16;
        match msg {
            WM_LBUTTONDOWN => println!("LBUTTONDOWN client=({x},{y})"),
            WM_LBUTTONUP => println!("LBUTTONUP client=({x},{y})"),
            WM_RBUTTONDOWN => println!("RBUTTONDOWN client=({x},{y})"),
            WM_RBUTTONUP => println!("RBUTTONUP client=({x},{y})"),
            WM_MOUSEMOVE if wp.0 & 1 != 0 => println!("DRAG client=({x},{y})"),
            WM_MOUSEWHEEL => println!("WHEEL delta={}", ((wp.0 >> 16) & 0xFFFF) as i16),
            WM_KEYDOWN | WM_KEYUP => println!(
                "{} vk=0x{:X} scan=0x{:X} extended={}",
                if msg == WM_KEYDOWN { "KEYDOWN" } else { "KEYUP" },
                wp.0,
                (lp.0 >> 16) & 0xFF,
                (lp.0 >> 24) & 1
            ),
            WM_DESTROY => PostQuitMessage(0),
            _ => {}
        }
        DefWindowProcW(hwnd, msg, wp, lp)
    }
}

pub fn run(seconds: u64) -> Result<()> {
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            lpszClassName: w!("RemoteDeskProbe"),
            hbrBackground: HBRUSH(GetStockObject(WHITE_BRUSH).0),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        RegisterClassW(&class);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST,
            w!("RemoteDeskProbe"),
            w!("Remote Desk probe"),
            WS_POPUP | WS_VISIBLE, // no frame, so the client area is exactly the window rectangle
            100,
            100,
            400,
            300,
            None,
            None,
            Some(hinst.into()),
            None,
        )?;
        let _ = SetForegroundWindow(hwnd);
        println!("probe ready: client area 400x300 at screen (100,100), listening for {seconds} s");

        let end = Instant::now() + Duration::from_secs(seconds);
        let mut msg = MSG::default();
        'outer: while Instant::now() < end {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    break 'outer;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let _ = DestroyWindow(hwnd);
        println!("probe done");
    }
    Ok(())
}
