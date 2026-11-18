//! Small helpers: hidden-console processes, toasts, explorer/terminal, app icon.

use eframe::egui;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// CREATE_NO_WINDOW: never flash a console window for child processes.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A `Command` that will not pop up a console window.
pub fn hidden_cmd(program: &str) -> Command {
    let mut c = Command::new(program);
    #[cfg(windows)]
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

/// Windows toast notification (fire and forget, off the UI thread).
pub const AUMID: &str = "VibeStorm";

/// Register "VibeStorm" as a toast sender (HKCU\Software\Classes\AppUserModelId),
/// so notifications show our name and icon instead of "Windows PowerShell".
fn ensure_aumid() -> bool {
    static DONE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *DONE.get_or_init(|| {
        let dir = match std::env::var_os("APPDATA") {
            Some(d) => PathBuf::from(d).join("VibeStorm"),
            None => return false,
        };
        let _ = std::fs::create_dir_all(&dir);
        let icon = dir.join("vibestorm.png");
        if !icon.exists() {
            let ic = app_icon();
            if let Some(img) = image::RgbaImage::from_raw(ic.width, ic.height, ic.rgba) {
                let _ = img.save(&icon);
            }
        }
        let key = format!(r"HKCU\Software\Classes\AppUserModelId\{AUMID}");
        let ok1 = hidden_cmd("reg")
            .args(["add", &key, "/v", "DisplayName", "/t", "REG_SZ", "/d", "VibeStorm", "/f"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        let _ = hidden_cmd("reg")
            .args(["add", &key, "/v", "IconUri", "/t", "REG_SZ", "/d", &icon.to_string_lossy(), "/f"])
            .output();
        ok1
    })
}

/// Windows toast. `on_click` runs when the user clicks the notification.
pub fn notify(title: &str, body: &str, sound: bool, on_click: Option<Box<dyn Fn() + Send + Sync + 'static>>) {
    let title = title.to_owned();
    let body = body.to_owned();
    std::thread::spawn(move || {
        use tauri_winrt_notification::{Duration, Sound, Toast};
        let on_click: Option<std::sync::Arc<dyn Fn() + Send + Sync>> = on_click.map(std::sync::Arc::from);
        let make = |app_id: &str| {
            let mut t = Toast::new(app_id)
                .title(&title)
                .text1(&body)
                .duration(Duration::Short)
                .sound(if sound { Some(Sound::Default) } else { None });
            if let Some(cb) = on_click.clone() {
                t = t.on_activated(move |_| {
                    cb();
                    Ok(())
                });
            }
            t
        };
        let ours = ensure_aumid() && make(AUMID).show().is_ok();
        if !ours {
            let _ = make(Toast::POWERSHELL_APP_ID).show();
        }
        // keep the thread (and the click handler) alive while the toast is on screen
        std::thread::sleep(std::time::Duration::from_secs(30));
    });
}

pub fn reveal_in_explorer(path: &Path) {
    let _ = Command::new("explorer")
        .arg(format!("/select,{}", path.display()))
        .spawn();
}

pub fn open_terminal(dir: &Path) {
    // Prefer Windows Terminal, fall back to PowerShell.
    let wt = Command::new("wt").arg("-d").arg(dir).spawn();
    if wt.is_err() {
        let _ = Command::new("cmd")
            .args(["/C", "start", "", "powershell", "-NoExit"])
            .current_dir(dir)
            .spawn();
    }
}

/// Path relative to `root`, with forward slashes.
pub fn rel_path(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

pub fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_owned()
    } else {
        let mut t: String = s.chars().take(max_chars).collect();
        t.push('…');
        t
    }
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// "just now", "5 min ago", "3 h ago", "2 d ago"
pub fn fmt_ago(ts: u64) -> String {
    let s = unix_now().saturating_sub(ts);
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86400 => format!("{} h ago", s / 3600),
        _ => format!("{} d ago", s / 86400),
    }
}

pub fn fmt_duration(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

/// Procedurally drawn window icon (no asset files needed).
pub fn app_icon() -> egui::IconData {
    let s = 64usize;
    let mut rgba = vec![0u8; s * s * 4];
    let r = 14.0f32;
    for y in 0..s {
        for x in 0..s {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            let sz = s as f32;
            // rounded-square mask
            let cx = fx.clamp(r, sz - r);
            let cy = fy.clamp(r, sz - r);
            let d = ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt();
            if d > r {
                continue;
            }
            let t = (fx + fy) / (2.0 * sz);
            let lerp = |a: f32, b: f32| (a + (b - a) * t) as u8;
            let mut px = [lerp(53.0, 150.0), lerp(116.0, 80.0), lerp(240.0, 230.0), 255];
            // lightning bolt
            const BOLT: [(f32, f32); 7] = [(38.0, 6.0), (15.0, 37.0), (29.0, 37.0), (24.0, 59.0), (49.0, 25.0), (34.0, 25.0), (42.0, 6.0)];
            let mut inside = false;
            let mut j = BOLT.len() - 1;
            for i in 0..BOLT.len() {
                let (xi, yi) = BOLT[i];
                let (xj, yj) = BOLT[j];
                if (yi > fy) != (yj > fy) && fx < (xj - xi) * (fy - yi) / (yj - yi) + xi {
                    inside = !inside;
                }
                j = i;
            }
            if inside {
                px = [255, 236, 120, 255];
            }
            let i = (y * s + x) * 4;
            rgba[i..i + 4].copy_from_slice(&px);
        }
    }
    egui::IconData {
        rgba,
        width: s as u32,
        height: s as u32,
    }
}

/// Put a child process into a job that is killed when VibeStorm exits (even on crash),
/// so Claude agents never outlive the IDE.
#[cfg(windows)]
pub fn kill_with_ide(child: &std::process::Child) {
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;
    use windows_sys::Win32::System::JobObjects::*;
    static JOB: OnceLock<usize> = OnceLock::new();
    let job = *JOB.get_or_init(|| unsafe {
        let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if h.is_null() {
            return 0;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        SetInformationJobObject(
            h,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const core::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        h as usize
    });
    if job != 0 {
        unsafe {
            AssignProcessToJobObject(job as _, child.as_raw_handle() as _);
        }
    }
}

/// Make the next file dialog this process opens (within a few seconds) tall: 85% of its
/// monitor's work area, about twice the Windows default. rfd can't size the dialog, so
/// a helper thread waits for the window to appear and resizes it.
#[cfg(windows)]
pub fn tall_file_dialog() {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    // a visible dialog-class (#32770) window of this process
    unsafe extern "system" fn find(hwnd: HWND, out: LPARAM) -> windows_sys::core::BOOL {
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut pid);
            if pid != std::process::id() || IsWindowVisible(hwnd) == 0 {
                return 1;
            }
            let mut class = [0u16; 16];
            let n = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
            if class[..n.max(0) as usize] == *"#32770".encode_utf16().collect::<Vec<_>>() {
                *(out as *mut HWND) = hwnd;
                return 0;
            }
        }
        1
    }

    std::thread::spawn(|| unsafe {
        for _ in 0..100 {
            std::thread::sleep(std::time::Duration::from_millis(30));
            let mut hwnd: HWND = std::ptr::null_mut();
            EnumWindows(Some(find), &mut hwnd as *mut HWND as LPARAM);
            if hwnd.is_null() {
                continue;
            }
            let mut r: RECT = std::mem::zeroed();
            let mut mi: MONITORINFO = std::mem::zeroed();
            mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            if GetWindowRect(hwnd, &mut r) == 0 || GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut mi) == 0 {
                return;
            }
            let work = mi.rcWork;
            let target = (work.bottom - work.top) * 85 / 100;
            if r.bottom - r.top < target {
                // keep it vertically centred on the monitor
                let top = work.top + (work.bottom - work.top - target) / 2;
                SetWindowPos(hwnd, std::ptr::null_mut(), r.left, top, r.right - r.left, target, SWP_NOZORDER | SWP_NOACTIVATE);
            }
            return;
        }
    });
}

#[cfg(not(windows))]
pub fn tall_file_dialog() {}

/// Write the app icon as a .ico file (used for desktop / Start menu shortcuts).
pub fn write_icon(path: &Path) -> Result<(), String> {
    let ic = app_icon();
    let img = image::RgbaImage::from_raw(ic.width, ic.height, ic.rgba).ok_or("bad icon")?;
    img.save_with_format(path, image::ImageFormat::Ico).map_err(|e| e.to_string())
}
