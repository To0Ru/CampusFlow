//! 用 Windows DPAPI 加密配置里的账号和密码。
//!
//! 为什么用它：DPAPI 的密钥由 Windows 从**你的登录凭据**派生，加密出来的东西
//! 换台机器、换个用户账户都解不开。所以 config.json 被人拷走也读不出密码。
//!
//! **它防的是"意外泄露"，不是"恶意窃取"：**
//!
//! | 场景 | 能防住吗 |
//! |---|---|
//! | 把 config.json 发给别人排查 / 云盘同步 / 备份 | ✅ |
//! | 同机器上另一个非管理员账户想读 | ✅ |
//! | **盗号木马** | ❌ 它以你的身份运行，调同一个 API 就能解 |
//! | 别人知道了你的 Windows 密码 | ❌ |
//!
//! ⚠️ 副作用：加密后 config.json **不能拷到别的电脑用了**，换机器要重填密码。
//!
//! 存储格式是给字符串加个 `dpapi:` 前缀，见到前缀才解密。
//! 这样旧版留下的明文配置能无痛迁移（读到明文就重新加密写回）。

use std::ffi::c_void;
use std::ptr::{null, null_mut};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;

const PREFIX: &str = "dpapi:";

/// 不弹任何 UI。不加这个的话在无人值守场景下 DPAPI 可能会尝试弹框。
const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x01;

#[repr(C)]
struct DataBlob {
    cb_data: u32,
    pb_data: *mut u8,
}

#[link(name = "crypt32")]
extern "system" {
    fn CryptProtectData(
        data_in: *const DataBlob,
        descr: *const u16,
        entropy: *const DataBlob,
        reserved: *mut c_void,
        prompt: *mut c_void,
        flags: u32,
        data_out: *mut DataBlob,
    ) -> i32;

    fn CryptUnprotectData(
        data_in: *const DataBlob,
        descr_out: *mut *mut u16,
        entropy: *const DataBlob,
        reserved: *mut c_void,
        prompt: *mut c_void,
        flags: u32,
        data_out: *mut DataBlob,
    ) -> i32;
}

#[link(name = "kernel32")]
extern "system" {
    fn LocalFree(mem: *mut c_void) -> *mut c_void;
}

fn last_error() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

/// 取出 DPAPI 分配的缓冲区并释放。
unsafe fn take_blob(out: DataBlob) -> Vec<u8> {
    if out.pb_data.is_null() || out.cb_data == 0 {
        return Vec::new();
    }
    let bytes = std::slice::from_raw_parts(out.pb_data, out.cb_data as usize).to_vec();
    LocalFree(out.pb_data as *mut c_void);
    bytes
}

fn protect(plain: &[u8]) -> Result<Vec<u8>, String> {
    unsafe {
        let input = DataBlob {
            cb_data: plain.len() as u32,
            pb_data: plain.as_ptr() as *mut u8,
        };
        let mut output = DataBlob {
            cb_data: 0,
            pb_data: null_mut(),
        };

        let ok = CryptProtectData(
            &input,
            null(),
            null(),
            null_mut(),
            null_mut(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        );
        if ok == 0 {
            return Err(format!("CryptProtectData 失败（错误码 {}）", last_error()));
        }
        Ok(take_blob(output))
    }
}

fn unprotect(blob: &[u8]) -> Result<Vec<u8>, String> {
    unsafe {
        let input = DataBlob {
            cb_data: blob.len() as u32,
            pb_data: blob.as_ptr() as *mut u8,
        };
        let mut output = DataBlob {
            cb_data: 0,
            pb_data: null_mut(),
        };
        let mut descr: *mut u16 = null_mut();

        let ok = CryptUnprotectData(
            &input,
            &mut descr,
            null(),
            null_mut(),
            null_mut(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        );
        if !descr.is_null() {
            LocalFree(descr as *mut c_void);
        }
        if ok == 0 {
            return Err(format!("CryptUnprotectData 失败（错误码 {}）", last_error()));
        }
        Ok(take_blob(output))
    }
}

/// 这个值是不是加密过的。
pub fn is_encrypted(stored: &str) -> bool {
    stored.starts_with(PREFIX)
}

/// 加密。空字符串原样返回（不需要为"没设密码"造一个密文）。
pub fn encrypt(plain: &str) -> Result<String, String> {
    if plain.is_empty() {
        return Ok(String::new());
    }
    let blob = protect(plain.as_bytes())?;
    Ok(format!("{PREFIX}{}", B64.encode(blob)))
}

/// 解密。旧版留下的明文会原样返回（调用方据此触发一次迁移写回）。
///
/// 解不开时返回空串——通常是换了机器或换了 Windows 账户。
/// 这样 `is_ready()` 会变 false，界面会提示重新填写，而不是拿着乱码去认证。
pub fn decrypt(stored: &str) -> String {
    if stored.is_empty() {
        return String::new();
    }
    let Some(b64) = stored.strip_prefix(PREFIX) else {
        // 没有前缀 = 旧版明文
        return stored.to_string();
    };
    B64.decode(b64)
        .map_err(|e| e.to_string())
        .and_then(|b| unprotect(&b))
        .and_then(|b| String::from_utf8(b).map_err(|e| e.to_string()))
        .unwrap_or_default()
}
