//! Package-manager hints for this distro, and downloads (curl or wget) into the user's home.

use crate::*;

// ---------------------------------------------------------------------------
// Distribution detection + package-manager install hints
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PkgMgr {
    Pacman,
    Apt,
    Dnf,
    Zypper,
    Unknown,
}

pub(crate) fn os_release_field(key: &str) -> Option<String> {
    let content = fs::read_to_string("/etc/os-release").ok()?;
    let needle = format!("{key}=");
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix(&needle) {
            let v = rest.trim().trim_matches('"').trim_matches('\'');
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

pub(crate) fn distro_name() -> String {
    os_release_field("PRETTY_NAME").unwrap_or_else(|| "your distribution".into())
}

pub(crate) fn detect_pkgmgr() -> PkgMgr {
    let id = os_release_field("ID").unwrap_or_default().to_lowercase();
    let like = os_release_field("ID_LIKE")
        .unwrap_or_default()
        .to_lowercase();
    let hay = format!("{id} {like}");
    let has = |s: &str| hay.split_whitespace().any(|w| w == s);
    if has("arch") || has("cachyos") || has("endeavouros") || has("manjaro") || has("garuda") {
        PkgMgr::Pacman
    } else if has("debian") || has("ubuntu") || has("linuxmint") || has("pop") || has("kali") {
        PkgMgr::Apt
    } else if has("fedora") || has("rhel") || has("centos") || has("nobara") {
        PkgMgr::Dnf
    } else if has("opensuse") || has("suse") || has("sles") {
        PkgMgr::Zypper
    } else {
        PkgMgr::Unknown
    }
}

pub(crate) fn install_command(pkgs: &[&str]) -> Option<String> {
    let list = pkgs.join(" ");
    Some(match detect_pkgmgr() {
        PkgMgr::Pacman => format!("sudo pacman -S --needed {list}"),
        PkgMgr::Apt => format!("sudo apt install {list}"),
        PkgMgr::Dnf => format!("sudo dnf install {list}"),
        PkgMgr::Zypper => format!("sudo zypper install {list}"),
        PkgMgr::Unknown => return None,
    })
}

// ---------------------------------------------------------------------------
// Downloads (curl/wget). Everything no-root installs into the user directory.
// ---------------------------------------------------------------------------

/// Fetch a URL. With `dest` set, writes the body there; otherwise returns stdout.
pub(crate) fn http_fetch(url: &str, dest: Option<&Path>) -> Result<String, String> {
    http_fetch_with(url, dest, &[])
}

/// `http_fetch` with extra request headers. curl reads them from stdin, so
/// secrets such as API keys never show up in the process list.
pub(crate) fn http_fetch_with(
    url: &str,
    dest: Option<&Path>,
    headers: &[(&str, &str)],
) -> Result<String, String> {
    if find_in_path("curl").is_some() {
        let mut c = Command::new("curl");
        c.arg("-fSL")
            .arg("--retry")
            .arg("3")
            .arg("-A")
            .arg("gamelo");
        match dest {
            Some(d) => {
                c.arg("-o").arg(d);
            }
            None => {
                c.arg("-s");
            }
        }
        if !headers.is_empty() {
            c.arg("-K").arg("-");
        }
        c.arg(url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().map_err(|e| format!("curl: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            let mut cfg = String::new();
            for (k, v) in headers {
                let line = format!("{k}: {v}").replace(['\r', '\n'], "");
                cfg.push_str(&format!(
                    "header = \"{}\"\n",
                    line.replace('\\', "\\\\").replace('"', "\\\"")
                ));
            }
            let _ = stdin.write_all(cfg.as_bytes());
        }
        let out = child.wait_with_output().map_err(|e| format!("curl: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!("download failed: {}", err.trim()));
        }
        return Ok(String::from_utf8_lossy(&out.stdout).to_string());
    }
    if find_in_path("wget").is_some() {
        let mut c = Command::new("wget");
        c.arg("-q");
        for (k, v) in headers {
            c.arg(format!("--header={k}: {v}"));
        }
        match dest {
            Some(d) => {
                c.arg("-O").arg(d);
            }
            None => {
                c.arg("-O").arg("-");
            }
        }
        c.arg(url);
        let out = c.output().map_err(|e| format!("wget: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!("download failed: {}", err.trim()));
        }
        return Ok(String::from_utf8_lossy(&out.stdout).to_string());
    }
    Err("neither curl nor wget is installed".into())
}

pub(crate) fn make_executable(p: &Path) -> std::io::Result<()> {
    let mut perms = fs::metadata(p)?.permissions();
    perms.set_mode(perms.mode() | 0o755);
    fs::set_permissions(p, perms)
}

/// Breadth-first search for a file named `name` under `root` (shallowest wins).
pub(crate) fn find_file_recursive(root: &Path, name: &str) -> Option<PathBuf> {
    let mut queue = std::collections::VecDeque::from([root.to_path_buf()]);
    while let Some(dir) = queue.pop_front() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                queue.push_back(p);
            } else if p.file_name().and_then(|n| n.to_str()) == Some(name) {
                return Some(p);
            }
        }
    }
    None
}

pub(crate) fn github_latest_asset(
    repo: &str,
    matches: impl Fn(&str) -> bool,
) -> Result<(String, String), String> {
    let api = format!("https://api.github.com/repos/{repo}/releases/latest");
    let body = http_fetch(&api, None)?;
    let v: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("bad release JSON: {e}"))?;
    let assets = v
        .get("assets")
        .and_then(|a| a.as_array())
        .ok_or("latest release has no assets")?;
    assets
        .iter()
        .find_map(|a| {
            let name = a.get("name")?.as_str()?;
            if matches(name) {
                let url = a.get("browser_download_url")?.as_str()?.to_string();
                Some((name.to_string(), url))
            } else {
                None
            }
        })
        .ok_or_else(|| "no matching download found in the latest release".to_string())
}

/// Download and install the latest GE-Proton build into the user's copy of Steam
/// (when present) or into ~/.local/share/gamelo/proton.
pub(crate) fn install_ge_proton(steam_root: Option<PathBuf>) -> Result<String, String> {
    let (name, url) = github_latest_asset("GloriousEggroll/proton-ge-custom", |n| {
        n.ends_with("-x86_64.tar.gz")
    })
    .or_else(|_| {
        github_latest_asset("GloriousEggroll/proton-ge-custom", |n| {
            n.ends_with(".tar.gz")
        })
    })?;
    let base = name
        .trim_end_matches(".tar.gz")
        .trim_end_matches("-x86_64")
        .to_string();

    let dest_root = match steam_root {
        Some(sr) => sr.join("compatibilitytools.d"),
        None => data_dir().join("proton"),
    };
    fs::create_dir_all(&dest_root).map_err(|e| e.to_string())?;
    let final_dir = dest_root.join(&base);
    if final_dir.join("proton").is_file() {
        return Ok(format!("{base} is already installed"));
    }

    let dl = data_dir().join("downloads");
    fs::create_dir_all(&dl).map_err(|e| e.to_string())?;
    let tar_path = dl.join(&name);
    http_fetch(&url, Some(&tar_path))?;

    // Extract into a staging dir on the same filesystem, then move the tool dir.
    let staging = dest_root.join(format!(".ptmp-{base}"));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let status = Command::new("tar")
        .arg("-xf")
        .arg(&tar_path)
        .arg("-C")
        .arg(&staging)
        .status()
        .map_err(|e| format!("tar: {e}"))?;
    let _ = fs::remove_file(&tar_path);
    if !status.success() {
        let _ = fs::remove_dir_all(&staging);
        return Err("extracting the Proton archive failed".into());
    }
    let proton = find_file_recursive(&staging, "proton")
        .ok_or("the archive did not contain a Proton build")?;
    let tool_dir = proton
        .parent()
        .ok_or("unexpected archive layout")?
        .to_path_buf();
    if final_dir.exists() {
        let _ = fs::remove_dir_all(&final_dir);
    }
    fs::rename(&tool_dir, &final_dir).map_err(|e| format!("moving Proton build: {e}"))?;
    let _ = fs::remove_dir_all(&staging);
    let _ = make_executable(&final_dir.join("proton"));
    Ok(format!("Installed {base} to {}", final_dir.display()))
}

/// Download the latest umu-launcher zipapp and install `umu-run` into ~/.local/bin.
pub(crate) fn install_umu() -> Result<String, String> {
    let (name, url) = github_latest_asset("Open-Wine-Components/umu-launcher", |n| {
        n.ends_with("-zipapp.tar")
    })?;

    let dl = data_dir().join("downloads");
    fs::create_dir_all(&dl).map_err(|e| e.to_string())?;
    let tar_path = dl.join(&name);
    http_fetch(&url, Some(&tar_path))?;

    let staging = dl.join("umu-staging");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let status = Command::new("tar")
        .arg("-xf")
        .arg(&tar_path)
        .arg("-C")
        .arg(&staging)
        .status()
        .map_err(|e| format!("tar: {e}"))?;
    let _ = fs::remove_file(&tar_path);
    if !status.success() {
        let _ = fs::remove_dir_all(&staging);
        return Err("extracting the umu archive failed".into());
    }
    let src = find_file_recursive(&staging, "umu-run").ok_or("umu-run not found in archive")?;
    let bin_dir = home().join(".local/bin");
    fs::create_dir_all(&bin_dir).map_err(|e| e.to_string())?;
    let dest = bin_dir.join("umu-run");
    fs::copy(&src, &dest).map_err(|e| format!("installing umu-run: {e}"))?;
    let _ = make_executable(&dest);
    let _ = fs::remove_dir_all(&staging);
    Ok(format!("Installed umu-run to {}", dest.display()))
}
