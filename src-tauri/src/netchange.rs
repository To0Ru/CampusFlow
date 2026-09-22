//! 监听 Windows 的网络地址变化。
//!
//! 用 iphlpapi 的 `NotifyAddrChange`。为了能取消（用户关掉自动重连时线程要能立刻退出），
//! 这里用「事件对象 + OVERLAPPED」的形式：
//!   NotifyAddrChange 立刻返回 ERROR_IO_PENDING，
//!   地址真的变化时把 event 置为有信号。
//!
//! **没有引 `windows` crate。** 那个 crate 很大，会明显拖慢 CI 编译并增大二进制；
//! 我们只要 5 个函数，直接手写 `extern "system"` 声明反而更划算。
//!
//! 注意 `NotifyAddrChange` 只在 **IP 地址发生变化** 时触发。
//! 校园网 AC 静默踢会话（IP 不变）是**不会**触发这个事件的——
//! 所以「网络变化时检查」模式在界面上必须如实标注这个局限。

use std::ffi::c_void;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

type Handle = *mut c_void;
type Bool = i32;
type Dword = u32;

const ERROR_IO_PENDING: Dword = 997;
const WAIT_OBJECT_0: Dword = 0;
const WAIT_TIMEOUT: Dword = 258;

/// 对应 Windows 的 OVERLAPPED。x64 下是 32 字节。
#[repr(C)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    /// Offset / OffsetHigh / Pointer 的联合体，x64 下 8 字节
    union: usize,
    h_event: Handle,
}

#[link(name = "iphlpapi")]
extern "system" {
    fn NotifyAddrChange(handle: *mut Handle, overlapped: *mut Overlapped) -> Dword;
    fn CancelIPChangeNotify(overlapped: *mut Overlapped) -> Bool;
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateEventW(
        attrs: *mut c_void,
        manual_reset: Bool,
        initial: Bool,
        name: *const u16,
    ) -> Handle;
    fn WaitForSingleObject(handle: Handle, millis: Dword) -> Dword;
    fn CloseHandle(handle: Handle) -> Bool;
}

/// 分片睡眠，保证能及时响应停止指令。
pub fn sleep_interruptible(stop: &AtomicBool, total: Duration) {
    let step = Duration::from_millis(200);
    let mut waited = Duration::ZERO;
    while waited < total && !stop.load(Ordering::Relaxed) {
        std::thread::sleep(step);
        waited += step;
    }
}

/// 阻塞直到网络地址变化，或被 stop 叫停。
///
/// 返回 `true` = 真的检测到网络变化；`false` = 被停止（调用方应退出循环）。
pub fn wait_for_change(stop: &AtomicBool) -> bool {
    unsafe {
        let event = CreateEventW(null_mut(), 1, 0, null());
        if event.is_null() {
            // 建不出事件对象就退化成分片睡眠，至少不会卡死
            sleep_interruptible(stop, Duration::from_secs(30));
            return !stop.load(Ordering::Relaxed);
        }

        let mut ov: Overlapped = std::mem::zeroed();
        ov.h_event = event;

        let mut handle: Handle = null_mut();
        let rc = NotifyAddrChange(&mut handle, &mut ov);
        if rc != 0 && rc != ERROR_IO_PENDING {
            // 注册失败，同上退化处理
            CloseHandle(event);
            sleep_interruptible(stop, Duration::from_secs(30));
            return !stop.load(Ordering::Relaxed);
        }

        let mut changed = false;
        loop {
            if stop.load(Ordering::Relaxed) {
                break;
            }
            match WaitForSingleObject(event, 500) {
                WAIT_OBJECT_0 => {
                    changed = true;
                    break;
                }
                WAIT_TIMEOUT => continue,
                _ => break,
            }
        }

        CancelIPChangeNotify(&mut ov);
        CloseHandle(event);
        changed
    }
}
