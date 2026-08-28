//! 平台相关小工具：磁盘剩余空间、路径安全。

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

/// 将内容写入同目录临时文件，再原子替换目标文件。
///
/// Windows 的 `std::fs::rename` 不能可靠覆盖已存在文件，因此必须使用
/// `MoveFileExW(MOVEFILE_REPLACE_EXISTING)`；否则设置和设备名只能首次写入成功。
pub fn atomic_write(path: &Path, content: &[u8], private: bool) -> Result<()> {
    let parent = path.parent().context("目标文件缺少父目录")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("创建目录 {} 失败", parent.display()))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("lunote-data");
    let tmp = parent.join(format!(".{}.{}.tmp", file_name, uuid::Uuid::new_v4()));

    let result = (|| -> Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .with_context(|| format!("创建临时文件 {} 失败", tmp.display()))?;
        file.write_all(content)
            .with_context(|| format!("写入临时文件 {} 失败", tmp.display()))?;
        file.flush()
            .with_context(|| format!("刷新临时文件 {} 失败", tmp.display()))?;
        file.sync_all()
            .with_context(|| format!("同步临时文件 {} 失败", tmp.display()))?;

        #[cfg(unix)]
        if private {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("设置文件权限 {} 失败", tmp.display()))?;
        }

        #[cfg(not(unix))]
        let _ = private;

        atomic_replace(&tmp, path).with_context(|| format!("原子替换 {} 失败", path.display()))?;
        Ok(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(windows)]
fn atomic_replace(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source: Vec<u16> = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let target: Vec<u16> = target
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let ok = unsafe {
        MoveFileExW(
            source.as_ptr(),
            target.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn atomic_replace(source: &Path, target: &Path) -> std::io::Result<()> {
    std::fs::rename(source, target)
}

/// 查询目录所在卷的剩余空间（字节）
#[cfg(windows)]
pub fn free_space(path: &std::path::Path) -> Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let wide: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut avail: u64 = 0;
    let mut total: u64 = 0;
    let mut total_free: u64 = 0;
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut avail as *mut u64,
            &mut total as *mut u64,
            &mut total_free as *mut u64,
        )
    };
    if ok == 0 {
        anyhow::bail!("GetDiskFreeSpaceExW 失败（路径 {}）", path.display());
    }
    Ok(avail)
}

#[cfg(not(windows))]
pub fn free_space(path: &std::path::Path) -> Result<u64> {
    use std::ffi::CString;
    let cpath = CString::new(path.as_os_str().as_encoded_bytes()).context("路径包含 NUL")?;
    let mut vfs: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(cpath.as_ptr(), &mut vfs) };
    if rc != 0 {
        anyhow::bail!("statvfs 失败（路径 {}）", path.display());
    }
    Ok((vfs.f_frsize as u64).saturating_mul(vfs.f_bavail as u64))
}

/// 把远端文件名净化成安全文件名：去路径分隔、去 `.`/`..`、控制字符替换。
/// 返回值只含安全的文件名（不含目录）。
pub fn sanitize_file_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        let ok = !matches!(
            c,
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0'
        ) && !c.is_control();
        if ok {
            out.push(c);
        } else {
            out.push('_');
        }
    }
    let trimmed = out.trim();
    let trimmed = if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        "unnamed"
    } else {
        trimmed
    };
    let mut t = trimmed.to_string();
    while t.len() > 240 {
        t.pop();
    }
    t
}

/// 检查相对路径是否安全（拒绝绝对路径、`..`、空段、盘符），并返回净化后的组件序列
pub fn sanitize_rel_path(rel: &str) -> Option<Vec<String>> {
    if rel.starts_with('/') || rel.starts_with('\\') || rel.contains('\0') {
        return None;
    }
    let mut parts = Vec::new();
    for raw in rel.split(['/', '\\']) {
        // 盘符（C:）与冒号一律拒绝
        if raw.contains(':') {
            return None;
        }
        let part = sanitize_file_name(raw);
        if part == "unnamed" && !raw.trim().is_empty() {
            return None;
        }
        if part.is_empty() {
            return None;
        }
        parts.push(part);
    }
    Some(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_sanitize() {
        assert_eq!(
            sanitize_file_name("a/b\\c:d*e?f\"g<h>i|j"),
            "a_b_c_d_e_f_g_h_i_j"
        );
        assert_eq!(sanitize_file_name(".."), "unnamed");
        assert_eq!(sanitize_file_name("."), "unnamed");
        assert_eq!(sanitize_file_name("   "), "unnamed");
        assert_eq!(sanitize_file_name("正常文件.txt"), "正常文件.txt");
        assert_eq!(sanitize_file_name("中文 空格.txt"), "中文 空格.txt");
    }

    #[test]
    fn relpath_sanitize() {
        assert!(sanitize_rel_path("a/b/c.txt").is_some());
        assert!(sanitize_rel_path("../x.txt").is_none());
        assert!(sanitize_rel_path("a/../../x").is_none());
        assert!(sanitize_rel_path("/abs/x").is_none());
        assert!(sanitize_rel_path("C:\\evil").is_none());
        assert!(sanitize_rel_path("正常/子目录/文件.txt").is_some());
    }

    #[test]
    fn atomic_write_replaces_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        atomic_write(&path, b"first", false).unwrap();
        atomic_write(&path, b"second", false).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            1,
            "不应遗留临时文件"
        );
    }
}
