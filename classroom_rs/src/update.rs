//! 程序自动更新（对应 Python check_for_updates / _do_update / make_update_package /
//! apply_self_update / raw_download / gitee_blob_download）。
//!
//! 流程：启动后静默读仓库 version.json（Contents API + token）→ 三元组版本比较 →
//! 有新版本则下载 exe（Gitee raw 端点被 403 拦截，自动降级 git/blobs API）→
//! 写临时 exe + RollCall_updater.bat → 隐藏窗口启动批处理并退出当前进程，
//! 由批处理等待 2 秒后覆盖 exe、重启程序并自删。

use crate::config::Config;
use crate::gitee::{app_has_update, build_raw_url, GiteeApi};
use crate::logging;
use crate::theme::APP_VERSION;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 启动时静默检查一次更新；发现新版本全自动下载升级（无需人工确认）。
/// 挂在后台线程调用，下载耗时（最长 600s）不影响主程序。
pub fn run_silent_check(cfg: Arc<Mutex<Config>>) {
    let snapshot = cfg.lock().unwrap().clone();
    match do_check(&snapshot) {
        Ok(Some((remote, exe_bytes))) => {
            logging::info(&format!(
                "新版本 v{remote} 下载完成（{} KB），启动自替换",
                exe_bytes.len() / 1024
            ));
            apply_self_update(&exe_bytes);
        }
        Ok(None) => {} // 已是最新 / 未启用
        Err(e) => logging::warn(&format!("检查更新失败: {e}")),
    }
}

/// 设置窗口手动检查：发现新版本直接自动升级（与静默路径一致，内部会退出进程）。
pub fn check_manual(cfg: Config) -> String {
    match do_check(&cfg) {
        Ok(Some((remote, data))) => {
            logging::info(&format!("新版本 v{remote} 下载完成，启动自替换"));
            apply_self_update(&data); // 内部 std::process::exit(0)，正常不会走到下一行
            format!("发现新版本 v{remote}，正在升级…")
        }
        Ok(None) => format!("已是最新版本 v{}", APP_VERSION),
        Err(e) => format!("检查更新失败: {e}"),
    }
}

/// 检查并下载。Ok(None) = 不需要更新。对应 Python check_for_updates.worker + _do_update.worker。
/// 成功时返回 (远端版本, 主程序字节)。单文件模式：只更新 app.exe，升级时由 bat 删除遗留的 desktop_board.exe。
fn do_check(cfg: &Config) -> Result<Option<(String, Vec<u8>)>, String> {
    if !cfg.update_enabled {
        return Ok(None);
    }
    let api = GiteeApi::new(&cfg.gitee_owner, &cfg.gitee_repo, &cfg.gitee_token);
    let raw = api.download_file_content("version.json")?;
    let info: Value = serde_json::from_slice(&raw)
        .map_err(|e| format!("version.json 解析失败: {e}"))?;
    let remote = info
        .get("version")
        .and_then(|x| x.as_str())
        .unwrap_or("0.0.0")
        .to_string();
    if !app_has_update(APP_VERSION, &remote) {
        return Ok(None);
    }
    let notes = info.get("notes").and_then(|x| x.as_str()).unwrap_or("");
    logging::info(&format!(
        "发现新版本 v{remote}（当前 v{APP_VERSION}），自动升级: {notes}"
    ));
    let exe_path = {
        let p = cfg.update_exe_path.trim().to_string();
        if p.is_empty() {
            info
                .get("exe_path")
                .and_then(|x| x.as_str())
                .unwrap_or("dist/app.exe")
                .to_string()
        } else {
            p
        }
    };
    let exe_url = info
        .get("exe_url")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            build_raw_url(
                &cfg.gitee_owner,
                &cfg.gitee_repo,
                cfg.update_branch.trim(),
                &exe_path,
            )
        });
    let data = raw_download(&api, &exe_url, &cfg.gitee_token)?;
    Ok(Some((remote, data)))
}

/// 下载文件内容；Gitee raw 端点已被拦截（403），自动降级为 git/blobs API 通道。
/// 对应 Python raw_download + gitee_blob_download（blobs 通道已由 GiteeApi::blob_download 实现）。
fn raw_download(api: &GiteeApi, url: &str, token: &str) -> Result<Vec<u8>, String> {
    if let Some((owner, repo, path)) = parse_gitee_raw(url) {
        let sub = GiteeApi::new(&owner, &repo, token);
        return sub.blob_download(&path, 600);
    }
    // 非 gitee raw：直接 GET（带 token query）
    let url2 = if !token.is_empty() && !url.contains("access_token=") {
        let sep = if url.contains('?') { "&" } else { "?" };
        format!("{url}{sep}access_token={token}")
    } else {
        url.to_string()
    };
    api.download_large_file(&url2)
}

/// 解析 `https://gitee.com/{owner}/{repo}/raw/{branch}/{path}`，返回 (owner, repo, 未转义 path)。
fn parse_gitee_raw(url: &str) -> Option<(String, String, String)> {
    let rest = url.strip_prefix("https://gitee.com/")?;
    let mut segs = rest.splitn(5, '/');
    let owner = segs.next()?.to_string();
    let repo = segs.next()?.to_string();
    let raw_kw = segs.next()?;
    if raw_kw != "raw" {
        return None;
    }
    let _branch = segs.next()?;
    let path = segs.next()?;
    if owner.is_empty() || repo.is_empty() || path.is_empty() {
        return None;
    }
    Some((owner, repo, crate::goeasy::percent_decode(path)))
}

/// 把新 exe 写入临时文件并生成 updater.bat，返回 (tmp_path, bat_path)。
/// 单文件模式（v1.0.24）：升级时顺带删除同目录遗留的 desktop_board.exe
/// （先杀进程再删；app.exe 已内嵌全部大屏代码，独立 exe 属冗余双版本）。
/// 不直接执行替换/退出，便于单元测试；真正的替换由 apply_self_update 调用。
///
/// 批处理要点（v1.0.17 修复自替换失败）：
/// - 运行中的 exe 在 Windows 下**不能 copy 覆盖，但可以 rename**（已实测）。
///   旧版用 copy /Y 直接覆盖 DST，而守护进程（同 exe 的 --guard 实例）仍持有该文件句柄，
///   导致 copy 15 次全败 → exe..update_failed.log，永远停在旧版本。
/// - 新流程：ren DST → DST.old（运行中可成功）→ copy SRC → DST（原名已空）→ start DST。
/// - copy 失败则把 DST.old 改回原名回滚，绝不留下「无程序」状态。
/// - 延时仍用 ping（timeout 在无窗口+无效 stdin 下立即 errorlevel=125 抢跑）。
pub fn make_update_package(exe_bytes: &[u8]) -> Result<(PathBuf, PathBuf), String> {
    let tmp = std::env::temp_dir().join("RollCall_update_tmp.exe");
    std::fs::write(&tmp, exe_bytes).map_err(|e| format!("写临时 exe 失败: {e}"))?;
    let current = std::env::current_exe().map_err(|e| format!("取当前 exe 路径失败: {e}"))?;
    let dst_name = current
        .file_name()
        .and_then(|x| x.to_str())
        .unwrap_or("classroom_rollcall.exe")
        .to_string();
    // 唯一 old 名：上次升级的 .old 若被守护占用没删掉，固定名会导致本次 ren 失败。
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let old_name = format!("RollCall_old_{stamp}.exe");
    let old_path = current.with_file_name(&old_name);
    let bat = std::env::temp_dir().join("RollCall_updater.bat");
    // 清理遗留的独立大屏 exe：先杀进程再删（删除失败不影响主程序升级）。
    let bexe = current.with_file_name("desktop_board.exe");
    let cleanup_section = format!(
        "\r\ntaskkill /F /IM desktop_board.exe >nul 2>&1\r\nif exist \"{bexe}\" del /F /Q \"{bexe}\" >nul 2>&1",
        bexe = bexe.display(),
    );
    // 全部路径在 Rust 侧算好，bat 只做纯字符串替换：避免 %~nxVAR% 对环境变量无效的问题。
    // 注意：清理/删除段必须放在 `del "%~f0"`（bat 自删）之前，自删后 cmd 对
    // 后续行的行为不可靠；主程序已 start，随后清理遗留文件、最后自删退出。
    let content = format!(
        "@echo off\r\nsetlocal\r\nset \"SRC={src}\"\r\nset \"DST={dst}\"\r\nset \"OLD={oldpath}\"\r\nset \"OLDNAME={oldname}\"\r\nset \"DSTNAME={dstname}\"\r\nping -n 3 127.0.0.1 >nul\r\nren \"%DST%\" \"%OLDNAME%\" >nul 2>&1\r\nif errorlevel 1 goto :failed\r\ncopy /Y \"%SRC%\" \"%DST%\" >nul 2>&1\r\nif errorlevel 1 goto :rollback\r\nstart \"\" \"%DST%\"\r\ndel /F /Q \"%OLD%\" >nul 2>&1{cleanup}\r\ndel \"%~f0\"\r\nexit /b 0\r\n:rollback\r\nren \"%OLD%\" \"%DSTNAME%\" >nul 2>&1\r\n:failed\r\necho [RollCall updater] rename/copy failed >> \"%DST%..update_failed.log\"\r\ndel \"%~f0\"\r\nexit /b 1\r\n",
        src = tmp.display(),
        dst = current.display(),
        oldpath = old_path.display(),
        oldname = old_name,
        dstname = dst_name,
        cleanup = cleanup_section,
    );
    std::fs::write(&bat, content).map_err(|e| format!("写 updater.bat 失败: {e}"))?;
    Ok((tmp, bat))
}

/// 下载完成后：写临时 exe + updater.bat，隐藏窗口启动批处理并退出当前进程，
/// 由批处理覆盖并重启（升级时顺带删除遗留的 desktop_board.exe）。
/// 退出前给日志留 0.5s 落盘时间。
pub fn apply_self_update(exe_bytes: &[u8]) {
    // 防循环护栏：下载内容与本地 exe 完全相同时绝不替换重启
    // （version.json 与 dist/app.exe 版本不一致时会形成无限下载循环）。
    if let Ok(cur) = std::env::current_exe() {
        if let Ok(local) = std::fs::read(&cur) {
            if local.len() == exe_bytes.len() && local.as_slice() == exe_bytes {
                logging::warn("新版本文件与本地 exe 内容相同，跳过替换（请检查仓库 dist/app.exe 是否漏发）");
                return;
            }
        }
    }
    let (_tmp, bat) = match make_update_package(exe_bytes) {
        Ok(x) => x,
        Err(e) => {
            logging::warn(&format!("准备更新包失败: {e}"));
            return;
        }
    };
    use std::os::windows::process::CommandExt;
    let spawn = std::process::Command::new("cmd.exe")
        .args(["/c", &bat.display().to_string()])
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    match spawn {
        Ok(_) => logging::info("更新批处理已启动，程序即将退出，由批处理完成替换与重启"),
        Err(e) => {
            logging::warn(&format!("启动更新批处理失败: {e}"));
            return;
        }
    }
    std::thread::sleep(Duration::from_millis(500));
    std::process::exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_cmp() {
        assert!(app_has_update("1.0.2", "1.0.3"));
        assert!(app_has_update("1.0.2", "v1.1.0"));
        assert!(!app_has_update("1.0.2", "1.0.2"));
        assert!(!app_has_update("2.0.0", "1.9.9"));
    }

    #[test]
    fn parse_raw_url() {
        let (o, r, p) = parse_gitee_raw(
            "https://gitee.com/gzzxgch/kejian/raw/master/dist/app.exe",
        )
        .unwrap();
        assert_eq!(o, "gzzxgch");
        assert_eq!(r, "kejian");
        assert_eq!(p, "dist/app.exe");
        assert!(parse_gitee_raw("https://github.com/a/b/raw/master/x").is_none());
        assert!(parse_gitee_raw("https://gitee.com/a/b/notraw/master/x").is_none());
    }

    #[test]
    fn update_package() {
        // 不带大屏包：不应出现大屏替换段，且 bat 自删必须仍在 exit /b 0 之前
        let (tmp, bat) = make_update_package(b"MZ-fake-exe").unwrap();
        assert_eq!(std::fs::read(&tmp).unwrap(), b"MZ-fake-exe");
        let content = std::fs::read_to_string(&bat).unwrap();
        // 自替换修复回归测试：必须 ren 旧 exe → copy 新 exe → start；
        // 延时用 ping 不用 timeout；copy 失败有回滚标签；路径全部预置为变量。
        assert!(!content.contains("timeout"), "禁止使用 timeout 延时");
        assert!(content.contains("ping -n 3 127.0.0.1"));
        assert!(content.contains("ren \"%DST%\" \"%OLDNAME%\""));
        assert!(content.contains("copy /Y \"%SRC%\" \"%DST%\""));
        assert!(content.contains(":rollback"));
        assert!(content.contains("ren \"%OLD%\" \"%DSTNAME%\""));
        assert!(content.contains("start \"\" \"%DST%\""));
        assert!(!content.contains("%~nxDST%"), "ren 目标不能用 %~nx 修饰环境变量");
        // 单文件模式清理段：杀遗留大屏进程 + 删除遗留 desktop_board.exe，
        // 必须在 bat 自删（del "%~f0"）之前执行。
        assert!(content.contains("taskkill /F /IM desktop_board.exe"));
        assert!(content.contains("desktop_board.exe\" >nul 2>&1"), "应删除遗留大屏 exe");
        assert!(!content.contains("%~nxB"), "路径全部预置为变量，不用 %~nx 修饰");
        assert!(content.find("taskkill /F /IM desktop_board.exe").unwrap()
            < content.find("del \"%~f0\"").unwrap());
        let _ = std::fs::remove_file(&tmp);
        let _ = std::fs::remove_file(&bat);
    }
}
