//! Small, dependency-free Windows screen-region capture used by the dashboard OCR flow.
//!
//! The picker owns its Win32 window and runs on a short-lived worker thread, so a
//! cancelled or failed picker cannot wedge the dashboard event loop.

/// PNG plus the selected rectangle in physical virtual-desktop coordinates.
pub struct Capture {
    pub image: String,
    pub bounds: [i32; 4],
}

#[cfg(windows)]
mod windows_capture {
    use super::Capture;
    use std::mem::{size_of, zeroed};
    use std::ptr::{null, null_mut};
    use std::sync::mpsc;
    use std::thread;
    use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::*;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        ReleaseCapture, SetCapture, SetFocus, VK_ESCAPE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    const CLASS: &[u16] = &[
        b'K' as u16,
        b'a' as u16,
        b'n' as u16,
        b'e' as u16,
        b'R' as u16,
        b'e' as u16,
        b'g' as u16,
        b'i' as u16,
        b'o' as u16,
        b'n' as u16,
        0,
    ];
    const WM_PICK: u32 = WM_APP + 1;
    const WM_CANCEL: u32 = WM_APP + 2;

    fn coord(v: LPARAM, high: bool) -> i32 {
        if high {
            ((v >> 16) as i16) as i32
        } else {
            (v as i16) as i32
        }
    }

    #[derive(Clone, Copy)]
    struct Picker {
        hwnd: HWND,
        screen_x: i32,
        screen_y: i32,
        origin: POINT,
        current: POINT,
        dragging: bool,
        done: bool,
        cancel: bool,
    }

    unsafe extern "system" fn picker_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Picker;
        match msg {
            WM_NCCREATE => {
                let cs = lp as *const CREATESTRUCTW;
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, (*cs).lpCreateParams as isize);
                1
            }
            WM_LBUTTONDOWN if !state.is_null() => {
                (*state).origin = POINT {
                    x: coord(lp, false),
                    y: coord(lp, true),
                };
                (*state).current = (*state).origin;
                (*state).dragging = true;
                SetCapture(hwnd);
                InvalidateRect(hwnd, null(), 0);
                0
            }
            WM_RBUTTONDOWN if !state.is_null() => {
                (*state).dragging = false;
                (*state).cancel = true;
                PostMessageW(hwnd, WM_CANCEL, 0, 0);
                0
            }
            WM_CAPTURECHANGED if !state.is_null() && (*state).dragging => {
                (*state).dragging = false;
                (*state).cancel = true;
                PostMessageW(hwnd, WM_CANCEL, 0, 0);
                0
            }
            WM_MOUSEMOVE if !state.is_null() && (*state).dragging => {
                (*state).current = POINT {
                    x: coord(lp, false),
                    y: coord(lp, true),
                };
                InvalidateRect(hwnd, null(), 0);
                0
            }
            WM_LBUTTONUP if !state.is_null() && (*state).dragging => {
                (*state).current = POINT {
                    x: coord(lp, false),
                    y: coord(lp, true),
                };
                (*state).dragging = false;
                (*state).done = true;
                ReleaseCapture();
                PostMessageW(hwnd, WM_PICK, 0, 0);
                0
            }
            WM_KEYDOWN if wp as u32 == VK_ESCAPE as u32 && !state.is_null() => {
                (*state).cancel = true;
                PostMessageW(hwnd, WM_CANCEL, 0, 0);
                0
            }
            WM_PICK | WM_CANCEL if !state.is_null() => {
                DestroyWindow(hwnd);
                0
            }
            WM_PAINT if !state.is_null() => {
                let mut ps: PAINTSTRUCT = zeroed();
                let dc = BeginPaint(hwnd, &mut ps);
                let mut rc: RECT = zeroed();
                GetClientRect(hwnd, &mut rc);
                let brush = CreateSolidBrush(0x00404040);
                FillRect(dc, &rc, brush);
                DeleteObject(brush as _);
                if (*state).dragging || (*state).done {
                    let x1 = (*state).origin.x.min((*state).current.x);
                    let y1 = (*state).origin.y.min((*state).current.y);
                    let x2 = (*state).origin.x.max((*state).current.x);
                    let y2 = (*state).origin.y.max((*state).current.y);
                    let pen = CreatePen(PS_SOLID, 2, 0x00ffffff);
                    let old = SelectObject(dc, pen as _);
                    SelectObject(dc, GetStockObject(NULL_BRUSH) as _);
                    Rectangle(dc, x1, y1, x2, y2);
                    SelectObject(dc, old);
                    DeleteObject(pen as _);
                }
                EndPaint(hwnd, &ps);
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }

    fn point_rect(a: POINT, b: POINT) -> Option<RECT> {
        let r = RECT {
            left: a.x.min(b.x),
            top: a.y.min(b.y),
            right: a.x.max(b.x),
            bottom: a.y.max(b.y),
        };
        (r.right - r.left >= 2 && r.bottom - r.top >= 2).then_some(r)
    }

    pub fn capture() -> Result<Option<Capture>, String> {
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("kane-region-picker".into())
            .spawn(move || {
                let result = unsafe { pick_and_capture() };
                let _ = tx.send(result);
            })
            .map_err(|e| e.to_string())?;
        rx.recv().map_err(|e| e.to_string())?
    }

    unsafe fn pick_and_capture() -> Result<Option<Capture>, String> {
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let w = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let h = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        if w <= 0 || h <= 0 {
            return Err("unable to determine virtual screen".into());
        }
        let mut state = Picker {
            hwnd: null_mut(),
            screen_x: x,
            screen_y: y,
            origin: POINT { x: 0, y: 0 },
            current: POINT { x: 0, y: 0 },
            dragging: false,
            done: false,
            cancel: false,
        };
        let wc = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(picker_proc),
            hInstance: null_mut() as HINSTANCE,
            lpszClassName: CLASS.as_ptr(),
            hCursor: LoadCursorW(null_mut(), IDC_CROSS),
            hbrBackground: null_mut(),
            cbClsExtra: 0,
            cbWndExtra: 0,
            ..zeroed()
        };
        RegisterClassW(&wc);
        state.hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_LAYERED,
            CLASS.as_ptr(),
            CLASS.as_ptr(),
            WS_POPUP,
            x,
            y,
            w,
            h,
            null_mut(),
            null_mut(),
            null_mut(),
            &mut state as *mut _ as _,
        );
        if state.hwnd.is_null() {
            return Err("unable to create region picker".into());
        }
        SetLayeredWindowAttributes(state.hwnd, 0, 80, LWA_ALPHA);
        ShowWindow(state.hwnd, SW_SHOW);
        SetForegroundWindow(state.hwnd);
        SetFocus(state.hwnd);
        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
            if state.cancel || state.done {
                break;
            }
        }
        if !state.hwnd.is_null() {
            DestroyWindow(state.hwnd);
        }
        if state.cancel {
            return Ok(None);
        }
        let mut r = match point_rect(state.origin, state.current) {
            Some(r) => r,
            None => return Ok(None),
        };
        r.left += state.screen_x;
        r.right += state.screen_x;
        r.top += state.screen_y;
        r.bottom += state.screen_y;
        capture_rect(r)
    }

    unsafe fn capture_rect(r: RECT) -> Result<Option<Capture>, String> {
        let w = (r.right - r.left) as i32;
        let h = (r.bottom - r.top) as i32;
        // Raw RGBA is intentionally uncompressed, so keep the resulting base64 well under the
        // dashboard's 8 MiB request limit (about 5.3 MiB for a one-million-pixel selection).
        if w <= 0 || h <= 0 || (w as u64) * (h as u64) > 1_000_000 {
            return Err("selected region is too large".into());
        }
        let screen = GetDC(null_mut());
        if screen.is_null() {
            return Err("unable to open screen DC".into());
        }
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, w, h);
        if mem.is_null() || bmp.is_null() {
            if !bmp.is_null() {
                DeleteObject(bmp as _);
            }
            if !mem.is_null() {
                DeleteDC(mem);
            }
            ReleaseDC(null_mut(), screen);
            return Err("unable to allocate capture bitmap".into());
        }
        let old_bmp = SelectObject(mem, bmp as _);
        let ok = BitBlt(mem, 0, 0, w, h, screen, r.left, r.top, SRCCOPY | CAPTUREBLT);
        let mut info: BITMAPINFO = zeroed();
        info.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = w;
        info.bmiHeader.biHeight = -h;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut pixels = vec![0u8; w as usize * h as usize * 4];
        SelectObject(mem, old_bmp);
        let got = GetDIBits(
            mem,
            bmp,
            0,
            h as u32,
            pixels.as_mut_ptr() as _,
            &mut info,
            DIB_RGB_COLORS,
        );
        DeleteObject(bmp as _);
        DeleteDC(mem);
        ReleaseDC(null_mut(), screen);
        if ok == 0 || got == 0 {
            return Err("unable to read captured pixels".into());
        }
        for px in pixels.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }
        Ok(Some(Capture {
            image: format!("data:image/png;base64,{}", base64(&png(w as u32, h as u32, &pixels))),
            bounds: [r.left, r.top, w, h],
        }))
    }

    fn png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
        let mut raw = Vec::with_capacity((w as usize * 4 + 1) * h as usize);
        for row in rgba.chunks_exact(w as usize * 4) {
            raw.push(0);
            raw.extend_from_slice(row);
        }
        let mut z = vec![0x78, 0x01];
        let mut at = 0;
        while at < raw.len() {
            let n = (raw.len() - at).min(65_535);
            let final_block = at + n == raw.len();
            z.push(final_block as u8);
            z.extend_from_slice(&(n as u16).to_le_bytes());
            z.extend_from_slice(&(!(n as u16)).to_le_bytes());
            z.extend_from_slice(&raw[at..at + n]);
            at += n;
        }
        let mut s1 = 1u32;
        let mut s2 = 0u32;
        for &b in &raw {
            s1 = (s1 + b as u32) % 65_521;
            s2 = (s2 + s1) % 65_521;
        }
        z.extend_from_slice(&((s2 << 16) | s1).to_be_bytes());
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        chunk(
            &mut out,
            b"IHDR",
            &[
                w.to_be_bytes().as_slice(),
                h.to_be_bytes().as_slice(),
                [8u8, 6, 0, 0, 0].as_slice(),
            ]
            .concat(),
        );
        chunk(&mut out, b"IDAT", &z);
        chunk(&mut out, b"IEND", &[]);
        out
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let mut c = 0xffff_ffffu32;
        for &b in kind.iter().chain(data) {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    (c >> 1) ^ 0xedb8_8320
                } else {
                    c >> 1
                };
            }
        }
        out.extend_from_slice(&(!c).to_be_bytes());
    }
    fn base64(bytes: &[u8]) -> String {
        const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut s = String::with_capacity((bytes.len() + 2) / 3 * 4);
        for c in bytes.chunks(3) {
            let n = ((c[0] as u32) << 16)
                | ((*c.get(1).unwrap_or(&0) as u32) << 8)
                | *c.get(2).unwrap_or(&0) as u32;
            s.push(A[(n >> 18 & 63) as usize] as char);
            s.push(A[(n >> 12 & 63) as usize] as char);
            s.push(if c.len() > 1 {
                A[(n >> 6 & 63) as usize] as char
            } else {
                '='
            });
            s.push(if c.len() > 2 {
                A[(n & 63) as usize] as char
            } else {
                '='
            });
        }
        s
    }

    #[cfg(test)]
    mod tests {
        use super::png;

        #[test]
        fn png_has_valid_signature_and_ihdr_layout() {
            let image = png(1, 1, &[255, 0, 0, 255]);
            assert_eq!(&image[..8], b"\x89PNG\r\n\x1a\n");
            assert_eq!(&image[8..12], &[0, 0, 0, 13]);
            assert_eq!(&image[12..16], b"IHDR");
            assert_eq!(&image[16..29], &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]);
        }
    }
}

#[cfg(windows)]
pub use windows_capture::capture;

#[cfg(not(windows))]
pub fn capture() -> Result<Option<Capture>, String> {
    Err("screen region capture is only available on Windows".into())
}
