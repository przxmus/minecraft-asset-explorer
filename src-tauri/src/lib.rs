use serde::{Deserialize, Serialize};
use std::{
    env,
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PrismRootCandidate {
    path: String,
    exists: bool,
    valid: bool,
    source: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct InstanceInfo {
    folder_name: String,
    display_name: String,
    path: String,
    minecraft_version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MmcPack {
    components: Vec<MmcComponent>,
}

#[derive(Debug, Deserialize)]
struct MmcComponent {
    uid: String,
    version: Option<String>,
}

#[tauri::command]
fn detect_prism_roots() -> Result<Vec<PrismRootCandidate>, String> {
    let mut candidates = Vec::new();

    if let Some(home) = home_dir() {
        candidates.push(build_candidate(
            home.join("Library/Application Support/PrismLauncher"),
            "macos-default",
        ));
        candidates.push(build_candidate(
            home.join(".local/share/PrismLauncher"),
            "linux-default",
        ));
        candidates.push(build_candidate(home.join("PrismLauncher"), "portable-home"));
    }

    if let Some(app_data) = env::var_os("APPDATA") {
        candidates.push(build_candidate(
            PathBuf::from(app_data).join("PrismLauncher"),
            "windows-default",
        ));
    }

    if let Ok(custom_root) = env::var("PRISM_ROOT") {
        candidates.push(build_candidate(PathBuf::from(custom_root), "env-prism-root"));
    }

    dedupe_candidates(candidates)
}

#[tauri::command]
fn list_instances(prism_root: String) -> Result<Vec<InstanceInfo>, String> {
    let prism_root = expand_home(&prism_root);
    validate_prism_root(&prism_root)?;

    let instances_dir = prism_root.join("instances");
    if !instances_dir.exists() {
        return Ok(Vec::new());
    }

    let mut instances = Vec::new();

    let entries = fs::read_dir(&instances_dir)
        .map_err(|error| format!("Failed to read instances directory: {error}"))?;

    for entry in entries {
        let entry = match entry {
            Ok(value) => value,
            Err(_) => continue,
        };

        let instance_path = entry.path();
        if !instance_path.is_dir() {
            continue;
        }

        let folder_name = entry.file_name().to_string_lossy().to_string();
        let display_name = instance_display_name(&instance_path).unwrap_or_else(|| folder_name.clone());
        let minecraft_version = parse_minecraft_version(&instance_path.join("mmc-pack.json"));

        instances.push(InstanceInfo {
            folder_name,
            display_name,
            path: instance_path.to_string_lossy().to_string(),
            minecraft_version,
        });
    }

    instances.sort_by(|left, right| left.display_name.cmp(&right.display_name));
    Ok(instances)
}

fn dedupe_candidates(candidates: Vec<PrismRootCandidate>) -> Result<Vec<PrismRootCandidate>, String> {
    use std::collections::HashSet;

    let mut seen = HashSet::new();
    let mut deduped = Vec::new();

    for candidate in candidates {
        if seen.insert(candidate.path.clone()) {
            deduped.push(candidate);
        }
    }

    if deduped.is_empty() {
        return Err("No Prism Launcher candidates were found on this machine".to_string());
    }

    Ok(deduped)
}

fn build_candidate(path: PathBuf, source: &str) -> PrismRootCandidate {
    let exists = path.exists();
    let valid = is_valid_prism_root(&path);
    PrismRootCandidate {
        path: path.to_string_lossy().to_string(),
        exists,
        valid,
        source: source.to_string(),
    }
}

fn validate_prism_root(path: &Path) -> Result<(), String> {
    if !is_valid_prism_root(path) {
        return Err(format!(
            "Invalid Prism root: {} (expected folders: instances and libraries)",
            path.to_string_lossy()
        ));
    }
    Ok(())
}

fn is_valid_prism_root(path: &Path) -> bool {
    path.is_dir() && path.join("instances").is_dir() && path.join("libraries").is_dir()
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

fn expand_home(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(stripped);
        }
    }

    if path == "~" {
        if let Some(home) = home_dir() {
            return home;
        }
    }

    PathBuf::from(path)
}

fn instance_display_name(instance_dir: &Path) -> Option<String> {
    let config_path = instance_dir.join("instance.cfg");
    let content = fs::read_to_string(config_path).ok()?;

    let mut in_general_section = false;

    for raw_line in content.lines() {
        let line = raw_line.trim();

        if line.starts_with('[') && line.ends_with(']') {
            in_general_section = line.eq_ignore_ascii_case("[General]");
            continue;
        }

        if in_general_section && line.starts_with("name=") {
            let name = line.trim_start_matches("name=").trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }

    None
}

fn parse_minecraft_version(mmc_pack_path: &Path) -> Option<String> {
    let content = fs::read_to_string(mmc_pack_path).ok()?;
    let parsed: MmcPack = serde_json::from_str(&content).ok()?;

    parsed
        .components
        .into_iter()
        .find(|component| component.uid == "net.minecraft")
        .and_then(|component| component.version)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![detect_prism_roots, list_instances])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
