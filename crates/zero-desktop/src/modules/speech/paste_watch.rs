//! 全局 Ctrl+V 观察器（仅观察，不拦截）。
//!
//! 自动复制把时间上相邻的多段优化「拼接」成整体写进剪贴板（见
//! [`commands::remote::next_clipboard_text`]），本意是结尾一次性粘贴能拿到完整长句。
//! 但用户若「每段优化即时粘贴」，下一段仍会带上已粘贴过的前一段 → 重复粘贴。
//!
//! 解法：装一个 Windows 低级键盘钩子（`WH_KEYBOARD_LL`）**只观察** Ctrl+V，钩子里照常
//! `CallNextHookEx` 不吞按键。观察到一次粘贴就置位 [`PASTE_SIGNAL`]；remote 接收循环在
//! 下次写剪贴板前 `take` 这个信号，若已粘贴则清空拼接累加器，使下一段重新从空开始。
//!
//! 信号是「边沿」语义：置位后由消费方一次性取走（swap-to-false）。多次 Ctrl+V 只要被取走一次
//! 即可——重置只影响下一段的累加，不动当前剪贴板内容，故不会误伤连续粘贴同一文本。

use std::sync::atomic::{AtomicBool, Ordering};

static PASTE_SIGNAL: AtomicBool = AtomicBool::new(false);

/// 取走「自上次取走以来发生过粘贴」的信号，并清零。返回 true 表示期间用户按过 Ctrl+V。
pub fn take_paste_signal() -> bool {
    PASTE_SIGNAL.swap(false, Ordering::Relaxed)
}

/// 启动全局 Ctrl+V 观察线程。幂等：重复调用只会装第一个钩子（后续线程会因装钩失败而退出）。
/// 非 Windows 平台为 no-op（本应用面向 Windows）。
pub fn start_paste_watcher() {
    #[cfg(windows)]
    win::start();
    #[cfg(not(windows))]
    {
        tracing::info!(target: "speech", "[paste_watch] non-windows build: paste watcher disabled");
    }
}

#[cfg(windows)]
mod win {
    use super::PASTE_SIGNAL;
    use std::ptr;
    use std::sync::atomic::Ordering;
    use tracing::{error, info};
    use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, SetWindowsHookExW, HC_ACTION, KBDLLHOOKSTRUCT, MSG,
        WH_KEYBOARD_LL, WM_KEYDOWN, WM_SYSKEYDOWN,
    };

    const VK_V: u32 = 0x56;
    const KEY_DOWN_MASK: u16 = 0x8000;

    unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32
            && (wparam as u32 == WM_KEYDOWN || wparam as u32 == WM_SYSKEYDOWN)
        {
            let kb = lparam as *const KBDLLHOOKSTRUCT;
            if !kb.is_null() && (*kb).vkCode == VK_V {
                let ctrl_down = (GetAsyncKeyState(VK_CONTROL as i32) as u16 & KEY_DOWN_MASK) != 0;
                if ctrl_down {
                    PASTE_SIGNAL.store(true, Ordering::Relaxed);
                }
            }
        }
        // 始终放行，绝不吞 Ctrl+V。
        CallNextHookEx(ptr::null_mut(), code, wparam, lparam)
    }

    pub fn start() {
        // 低级键盘钩子要求装钩线程自带消息循环，故起一个专用线程常驻。
        std::thread::Builder::new()
            .name("paste-watch".into())
            .spawn(|| unsafe {
                let hook = SetWindowsHookExW(
                    WH_KEYBOARD_LL,
                    Some(hook_proc),
                    ptr::null_mut(),
                    0,
                );
                if hook.is_null() {
                    error!(target: "speech", "[paste_watch] SetWindowsHookExW failed; Ctrl+V reset disabled");
                    return;
                }
                info!(target: "speech", "[paste_watch] global Ctrl+V watcher installed");
                // 阻塞跑消息循环让钩子保活；进程退出时随线程一并结束。
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {}
            })
            .map_err(|e| {
                error!(target: "speech", "[paste_watch] spawn watcher thread failed: {e}");
            })
            .ok();
    }
}
