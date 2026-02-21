use base64::Engine;
use clipboard_rs::{Clipboard, ClipboardContext};
use ffmpeg_sidecar::download::{download_ffmpeg_package, ffmpeg_download_url, unpack_ffmpeg};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    env,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    thread,
};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;
use walkdir::WalkDir;
use zip::ZipArchive;

const ROOT_NODE_ID: &str = "root";

#[derive(Default)]
struct AppState {
    scans: Mutex<HashMap<String, ScanState>>,
    temp_paths: Mutex<Vec<PathBuf>>,
}

#[derive(Debug, Clone)]
struct ScanState {
    status: ScanLifecycle,
    scanned_containers: usize,
    total_containers: usize,
    error: Option<String>,
    cancelled: bool,
    assets: Vec<AssetRecord>,
    asset_index: HashMap<String, usize>,
    search_index: HashMap<String, AssetSearchRecord>,
    tree_children: HashMap<String, Vec<TreeNode>>,
}

impl ScanState {
    fn new() -> Self {
        let mut tree_children = HashMap::new();
        tree_children.insert(ROOT_NODE_ID.to_string(), Vec::new());

        Self {
            status: ScanLifecycle::Scanning,
            scanned_containers: 0,
            total_containers: 0,
            error: None,
            cancelled: false,
            assets: Vec::new(),
            asset_index: HashMap::new(),
            search_index: HashMap::new(),
            tree_children,
        }
    }

    fn as_status(&self, scan_id: &str) -> ScanStatus {
        ScanStatus {
            scan_id: scan_id.to_string(),
            lifecycle: self.status.clone(),
            scanned_containers: self.scanned_containers,
            total_containers: self.total_containers,
            asset_count: self.assets.len(),
            error: self.error.clone(),
        }
    }
}

#[derive(Debug, Clone)]
struct AssetSearchRecord {
    tokens: Vec<String>,
    compact: String,
    key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrismRootCandidate {
    path: String,
    exists: bool,
    valid: bool,
    source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstanceInfo {
    folder_name: String,
    display_name: String,
    path: String,
    minecraft_version: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum AssetSourceType {
    Vanilla,
    Mod,
    ResourcePack,
}

impl AssetSourceType {
    fn tree_root_name(&self) -> &'static str {
        match self {
            AssetSourceType::Vanilla => "vanilla",
            AssetSourceType::Mod => "mods",
            AssetSourceType::ResourcePack => "resourcepacks",
        }
    }

    fn key_prefix(&self) -> &'static str {
        match self {
            AssetSourceType::Vanilla => "vanilla",
            AssetSourceType::Mod => "mod",
            AssetSourceType::ResourcePack => "resourcepack",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum AssetContainerType {
    Directory,
    Zip,
    Jar,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AssetRecord {
    asset_id: String,
    key: String,
    source_type: AssetSourceType,
    source_name: String,
    namespace: String,
    relative_asset_path: String,
    extension: String,
    is_image: bool,
    is_audio: bool,
    container_path: String,
    container_type: AssetContainerType,
    entry_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TreeNode {
    id: String,
    name: String,
    node_type: TreeNodeType,
    has_children: bool,
    asset_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
enum TreeNodeType {
    Folder,
    File,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartScanRequest {
    prism_root: String,
    instance_folder: String,
    include_vanilla: bool,
    include_mods: bool,
    include_resourcepacks: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StartScanResponse {
    scan_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanStatus {
    scan_id: String,
    lifecycle: ScanLifecycle,
    scanned_containers: usize,
    total_containers: usize,
    asset_count: usize,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
enum ScanLifecycle {
    Scanning,
    Completed,
    Cancelled,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanProgressEvent {
    scan_id: String,
    scanned_containers: usize,
    total_containers: usize,
    asset_count: usize,
    current_source: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanChunkEvent {
    scan_id: String,
    assets: Vec<AssetRecord>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanCompletedEvent {
    scan_id: String,
    lifecycle: ScanLifecycle,
    asset_count: usize,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchRequest {
    scan_id: String,
    query: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchResponse {
    total: usize,
    assets: Vec<AssetRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListTreeChildrenRequest {
    scan_id: String,
    node_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AssetPreviewResponse {
    mime: String,
    base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum AudioFormat {
    Original,
    Mp3,
    Wav,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveAssetsRequest {
    scan_id: String,
    asset_ids: Vec<String>,
    destination_dir: String,
    audio_format: Option<AudioFormat>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CopyAssetsRequest {
    scan_id: String,
    asset_ids: Vec<String>,
    audio_format: Option<AudioFormat>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveAssetsResult {
    saved_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CopyResult {
    copied_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConvertAudioRequest {
    scan_id: String,
    asset_id: String,
    format: AudioFormat,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConvertedTempFileRef {
    path: String,
    format: AudioFormat,
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

#[derive(Debug, Clone)]
struct ScanContainer {
    source_type: AssetSourceType,
    source_name: String,
    container_type: AssetContainerType,
    container_path: PathBuf,
}

#[derive(Debug, Clone)]
struct AssetCandidate {
    source_type: AssetSourceType,
    source_name: String,
    namespace: String,
    relative_asset_path: String,
    container_path: PathBuf,
    container_type: AssetContainerType,
    entry_path: String,
    extension: String,
    is_image: bool,
    is_audio: bool,
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

#[tauri::command]
fn start_scan(
    app: AppHandle,
    state: State<'_, AppState>,
    req: StartScanRequest,
) -> Result<StartScanResponse, String> {
    let scan_id = Uuid::new_v4().to_string();

    {
        let mut scans = state
            .scans
            .lock()
            .map_err(|_| "Failed to lock scans state".to_string())?;

        let mut scan_state = ScanState::new();
        scan_state.total_containers = estimate_container_count(&req)?;
        scans.insert(scan_id.clone(), scan_state);
    }

    let _ = app.emit(
        "scan://started",
        serde_json::json!({
            "scanId": scan_id,
        }),
    );

    let scan_id_for_worker = scan_id.clone();
    let app_for_worker = app.clone();
    thread::spawn(move || {
        run_scan_worker(app_for_worker, scan_id_for_worker, req);
    });

    Ok(StartScanResponse { scan_id })
}

#[tauri::command]
fn get_scan_status(scan_id: String, state: State<'_, AppState>) -> Result<ScanStatus, String> {
    let scans = state
        .scans
        .lock()
        .map_err(|_| "Failed to lock scans state".to_string())?;

    let scan = scans
        .get(&scan_id)
        .ok_or_else(|| format!("Unknown scan id: {scan_id}"))?;

    Ok(scan.as_status(&scan_id))
}

#[tauri::command]
fn cancel_scan(scan_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let mut scans = state
        .scans
        .lock()
        .map_err(|_| "Failed to lock scans state".to_string())?;

    let scan = scans
        .get_mut(&scan_id)
        .ok_or_else(|| format!("Unknown scan id: {scan_id}"))?;

    scan.cancelled = true;
    scan.status = ScanLifecycle::Cancelled;
    Ok(())
}

#[tauri::command]
fn list_tree_children(
    req: ListTreeChildrenRequest,
    state: State<'_, AppState>,
) -> Result<Vec<TreeNode>, String> {
    let scans = state
        .scans
        .lock()
        .map_err(|_| "Failed to lock scans state".to_string())?;

    let scan = scans
        .get(&req.scan_id)
        .ok_or_else(|| format!("Unknown scan id: {}", req.scan_id))?;

    let node_id = req.node_id.unwrap_or_else(|| ROOT_NODE_ID.to_string());
    let mut children = scan
        .tree_children
        .get(&node_id)
        .cloned()
        .unwrap_or_default();

    children.sort_by(|left, right| {
        let left_rank = match left.node_type {
            TreeNodeType::Folder => 0,
            TreeNodeType::File => 1,
        };
        let right_rank = match right.node_type {
            TreeNodeType::Folder => 0,
            TreeNodeType::File => 1,
        };

        left_rank
            .cmp(&right_rank)
            .then(left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });

    Ok(children)
}

#[tauri::command]
fn search_assets(req: SearchRequest, state: State<'_, AppState>) -> Result<SearchResponse, String> {
    let scans = state
        .scans
        .lock()
        .map_err(|_| "Failed to lock scans state".to_string())?;

    let scan = scans
        .get(&req.scan_id)
        .ok_or_else(|| format!("Unknown scan id: {}", req.scan_id))?;

    let offset = req.offset.unwrap_or(0);
    let limit = req.limit.unwrap_or(200).min(1000);

    if req.query.trim().is_empty() {
        let total = scan.assets.len();
        let assets = scan
            .assets
            .iter()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect();

        return Ok(SearchResponse { total, assets });
    }

    let query_tokens = split_tokens(&req.query);
    let query_compact = compact_text(&req.query);

    let mut ranked = Vec::new();

    for asset in &scan.assets {
        let Some(index) = scan.search_index.get(&asset.asset_id) else {
            continue;
        };

        if let Some(score) = score_query(index, &query_tokens, &query_compact, &req.query) {
            ranked.push((score, asset));
        }
    }

    ranked.sort_by(|left, right| right.0.cmp(&left.0).then(left.1.key.cmp(&right.1.key)));

    let total = ranked.len();
    let assets = ranked
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|(_, asset)| asset.clone())
        .collect();

    Ok(SearchResponse { total, assets })
}

#[tauri::command]
fn get_asset_preview(
    scan_id: String,
    asset_id: String,
    state: State<'_, AppState>,
) -> Result<AssetPreviewResponse, String> {
    let asset = get_asset_from_state(&state, &scan_id, &asset_id)?;

    if !asset.is_image {
        return Err("Preview is only available for image assets".to_string());
    }

    let bytes = extract_asset_bytes(&asset)?;
    let base64 = base64::engine::general_purpose::STANDARD.encode(bytes);

    Ok(AssetPreviewResponse {
        mime: mime_for_extension(&asset.extension).to_string(),
        base64,
    })
}

#[tauri::command]
fn save_assets(
    app: AppHandle,
    req: SaveAssetsRequest,
    state: State<'_, AppState>,
) -> Result<SaveAssetsResult, String> {
    if req.asset_ids.is_empty() {
        return Ok(SaveAssetsResult {
            saved_files: Vec::new(),
        });
    }

    let destination_dir = expand_home(&req.destination_dir);
    fs::create_dir_all(&destination_dir)
        .map_err(|error| format!("Failed to create destination directory: {error}"))?;

    let requested_assets = collect_assets(&state, &req.scan_id, &req.asset_ids)?;
    let mut used_names = HashSet::new();
    let mut saved_files = Vec::new();

    for asset in requested_assets {
        let path = materialize_asset(
            &app,
            &asset,
            &destination_dir,
            req.audio_format.clone().unwrap_or(AudioFormat::Original),
            &mut used_names,
        )?;
        saved_files.push(path.to_string_lossy().to_string());
    }

    Ok(SaveAssetsResult { saved_files })
}

#[tauri::command]
fn copy_assets_to_clipboard(
    app: AppHandle,
    req: CopyAssetsRequest,
    state: State<'_, AppState>,
) -> Result<CopyResult, String> {
    if req.asset_ids.is_empty() {
        return Ok(CopyResult {
            copied_files: Vec::new(),
        });
    }

    let requested_assets = collect_assets(&state, &req.scan_id, &req.asset_ids)?;
    let temp_root = app
        .path()
        .app_cache_dir()
        .map_err(|error| format!("Failed to get app cache directory: {error}"))?
        .join("clipboard-assets")
        .join(Uuid::new_v4().to_string());

    fs::create_dir_all(&temp_root)
        .map_err(|error| format!("Failed to create temporary copy directory: {error}"))?;

    let mut used_names = HashSet::new();
    let mut copied_paths = Vec::new();

    for asset in requested_assets {
        let output_path = materialize_asset(
            &app,
            &asset,
            &temp_root,
            req.audio_format.clone().unwrap_or(AudioFormat::Original),
            &mut used_names,
        )?;
        copied_paths.push(output_path);
    }

    let clipboard = ClipboardContext::new()
        .map_err(|error| format!("Failed to open clipboard context: {error}"))?;

    clipboard
        .set_files(
            copied_paths
                .iter()
                .map(|path| path.to_string_lossy().to_string())
                .collect(),
        )
        .map_err(|error| format!("Failed to copy files to clipboard: {error}"))?;

    {
        let mut temp_paths = state
            .temp_paths
            .lock()
            .map_err(|_| "Failed to lock temp paths".to_string())?;
        temp_paths.push(temp_root);
    }

    Ok(CopyResult {
        copied_files: copied_paths
            .iter()
            .map(|path| path.to_string_lossy().to_string())
            .collect(),
    })
}

#[tauri::command]
fn convert_audio_asset(
    app: AppHandle,
    req: ConvertAudioRequest,
    state: State<'_, AppState>,
) -> Result<ConvertedTempFileRef, String> {
    if req.format == AudioFormat::Original {
        return Err("Use save/copy with original format instead of convert command".to_string());
    }

    let asset = get_asset_from_state(&state, &req.scan_id, &req.asset_id)?;
    if !asset.is_audio {
        return Err("Selected asset is not an audio file".to_string());
    }

    let temp_root = app
        .path()
        .app_cache_dir()
        .map_err(|error| format!("Failed to get app cache directory: {error}"))?
        .join("converted-audio")
        .join(Uuid::new_v4().to_string());

    fs::create_dir_all(&temp_root)
        .map_err(|error| format!("Failed to create temporary conversion directory: {error}"))?;

    let mut used_names = HashSet::new();
    let output_path = materialize_asset(&app, &asset, &temp_root, req.format.clone(), &mut used_names)?;

    {
        let mut temp_paths = state
            .temp_paths
            .lock()
            .map_err(|_| "Failed to lock temp paths".to_string())?;
        temp_paths.push(temp_root);
    }

    Ok(ConvertedTempFileRef {
        path: output_path.to_string_lossy().to_string(),
        format: req.format,
    })
}

fn run_scan_worker(app: AppHandle, scan_id: String, req: StartScanRequest) {
    let result = run_scan_worker_inner(&app, &scan_id, &req);

    if let Err(error) = result {
        update_scan_error(&app, &scan_id, &error);
        let _ = app.emit(
            "scan://error",
            serde_json::json!({
                "scanId": scan_id,
                "error": error,
            }),
        );
    }
}

fn run_scan_worker_inner(app: &AppHandle, scan_id: &str, req: &StartScanRequest) -> Result<(), String> {
    let prism_root = expand_home(&req.prism_root);
    validate_prism_root(&prism_root)?;

    let instance_dir = resolve_instance_dir(&prism_root, &req.instance_folder)?;
    let mc_version = parse_minecraft_version(&instance_dir.join("mmc-pack.json"))
        .ok_or_else(|| "Failed to resolve Minecraft version from mmc-pack.json".to_string())?;

    let containers = collect_scan_containers(&prism_root, &instance_dir, &mc_version, req)?;

    {
        let state = app.state::<AppState>();
        let mut scans = state
            .scans
            .lock()
            .map_err(|_| "Failed to lock scans state".to_string())?;

        if let Some(scan) = scans.get_mut(scan_id) {
            scan.total_containers = containers.len();
        }
    }

    let mut key_counts = HashMap::<String, usize>::new();

    for (index, container) in containers.iter().enumerate() {
        if is_scan_cancelled(app, scan_id)? {
            complete_scan_with_lifecycle(app, scan_id, ScanLifecycle::Cancelled, None)?;
            return Ok(());
        }

        let candidates = scan_container(container)?;
        let assets = finalize_assets(candidates, &mut key_counts);

        append_assets_chunk(app, scan_id, &assets, index + 1, containers.len(), Some(container.source_name.clone()))?;
    }

    complete_scan_with_lifecycle(app, scan_id, ScanLifecycle::Completed, None)?;
    Ok(())
}

fn append_assets_chunk(
    app: &AppHandle,
    scan_id: &str,
    chunk: &[AssetRecord],
    scanned_containers: usize,
    total_containers: usize,
    current_source: Option<String>,
) -> Result<(), String> {
    let asset_count;

    {
        let state = app.state::<AppState>();
        let mut scans = state
            .scans
            .lock()
            .map_err(|_| "Failed to lock scans state".to_string())?;

        let scan = scans
            .get_mut(scan_id)
            .ok_or_else(|| format!("Unknown scan id: {scan_id}"))?;

        scan.scanned_containers = scanned_containers;
        scan.total_containers = total_containers;

        for asset in chunk {
            if scan.asset_index.contains_key(&asset.asset_id) {
                continue;
            }

            let index = scan.assets.len();
            scan.asset_index.insert(asset.asset_id.clone(), index);
            scan.search_index
                .insert(asset.asset_id.clone(), build_search_record(asset));
            scan.assets.push(asset.clone());
            add_asset_to_tree(&mut scan.tree_children, asset);
        }

        asset_count = scan.assets.len();
    }

    if !chunk.is_empty() {
        let _ = app.emit(
            "scan://chunk",
            ScanChunkEvent {
                scan_id: scan_id.to_string(),
                assets: chunk.to_vec(),
            },
        );
    }

    let _ = app.emit(
        "scan://progress",
        ScanProgressEvent {
            scan_id: scan_id.to_string(),
            scanned_containers,
            total_containers,
            asset_count,
            current_source,
        },
    );

    Ok(())
}

fn update_scan_error(app: &AppHandle, scan_id: &str, error: &str) {
    let state = app.state::<AppState>();
    let lock_result = state.scans.lock();
    if let Ok(mut scans) = lock_result {
        if let Some(scan) = scans.get_mut(scan_id) {
            scan.status = ScanLifecycle::Error;
            scan.error = Some(error.to_string());
        }
    }
}

fn complete_scan_with_lifecycle(
    app: &AppHandle,
    scan_id: &str,
    lifecycle: ScanLifecycle,
    error: Option<String>,
) -> Result<(), String> {
    let asset_count;

    {
        let state = app.state::<AppState>();
        let mut scans = state
            .scans
            .lock()
            .map_err(|_| "Failed to lock scans state".to_string())?;

        let scan = scans
            .get_mut(scan_id)
            .ok_or_else(|| format!("Unknown scan id: {scan_id}"))?;

        scan.status = lifecycle.clone();
        scan.error = error.clone();
        scan.scanned_containers = scan.total_containers;
        asset_count = scan.assets.len();
    }

    let _ = app.emit(
        "scan://completed",
        ScanCompletedEvent {
            scan_id: scan_id.to_string(),
            lifecycle,
            asset_count,
            error,
        },
    );

    Ok(())
}

fn is_scan_cancelled(app: &AppHandle, scan_id: &str) -> Result<bool, String> {
    let state = app.state::<AppState>();
    let scans = state
        .scans
        .lock()
        .map_err(|_| "Failed to lock scans state".to_string())?;

    let scan = scans
        .get(scan_id)
        .ok_or_else(|| format!("Unknown scan id: {scan_id}"))?;

    Ok(scan.cancelled)
}

fn estimate_container_count(req: &StartScanRequest) -> Result<usize, String> {
    let prism_root = expand_home(&req.prism_root);
    validate_prism_root(&prism_root)?;
    let instance_dir = resolve_instance_dir(&prism_root, &req.instance_folder)?;
    let version = parse_minecraft_version(&instance_dir.join("mmc-pack.json"))
        .ok_or_else(|| "Failed to resolve Minecraft version from mmc-pack.json".to_string())?;

    Ok(collect_scan_containers(&prism_root, &instance_dir, &version, req)?.len())
}

fn collect_scan_containers(
    prism_root: &Path,
    instance_dir: &Path,
    mc_version: &str,
    req: &StartScanRequest,
) -> Result<Vec<ScanContainer>, String> {
    let mut containers = Vec::new();
    let minecraft_dir = instance_dir.join("minecraft");

    if req.include_mods {
        let mods_dir = minecraft_dir.join("mods");
        if mods_dir.is_dir() {
            let entries = fs::read_dir(&mods_dir)
                .map_err(|error| format!("Failed to read mods directory: {error}"))?;

            for entry in entries {
                let entry = match entry {
                    Ok(value) => value,
                    Err(_) => continue,
                };

                let path = entry.path();
                let extension = path
                    .extension()
                    .map(|value| value.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();

                if extension == "jar" {
                    let source_name = path
                        .file_stem()
                        .map(|value| value.to_string_lossy().to_string())
                        .unwrap_or_else(|| "unknown-mod".to_string());

                    containers.push(ScanContainer {
                        source_type: AssetSourceType::Mod,
                        source_name,
                        container_type: AssetContainerType::Jar,
                        container_path: path,
                    });
                }
            }
        }
    }

    if req.include_resourcepacks {
        let resourcepacks_dir = minecraft_dir.join("resourcepacks");
        if resourcepacks_dir.is_dir() {
            let entries = fs::read_dir(&resourcepacks_dir)
                .map_err(|error| format!("Failed to read resourcepacks directory: {error}"))?;

            for entry in entries {
                let entry = match entry {
                    Ok(value) => value,
                    Err(_) => continue,
                };

                let path = entry.path();
                if path.is_dir() {
                    let source_name = path
                        .file_name()
                        .map(|value| value.to_string_lossy().to_string())
                        .unwrap_or_else(|| "resourcepack".to_string());

                    containers.push(ScanContainer {
                        source_type: AssetSourceType::ResourcePack,
                        source_name,
                        container_type: AssetContainerType::Directory,
                        container_path: path,
                    });
                } else if path
                    .extension()
                    .map(|value| value.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default()
                    == "zip"
                {
                    let source_name = path
                        .file_stem()
                        .map(|value| value.to_string_lossy().to_string())
                        .unwrap_or_else(|| "resourcepack".to_string());

                    containers.push(ScanContainer {
                        source_type: AssetSourceType::ResourcePack,
                        source_name,
                        container_type: AssetContainerType::Zip,
                        container_path: path,
                    });
                }
            }
        }
    }

    if req.include_vanilla {
        let client_jar = prism_root
            .join("libraries")
            .join("com")
            .join("mojang")
            .join("minecraft")
            .join(mc_version)
            .join(format!("minecraft-{mc_version}-client.jar"));

        if client_jar.is_file() {
            containers.push(ScanContainer {
                source_type: AssetSourceType::Vanilla,
                source_name: format!("minecraft-{mc_version}"),
                container_type: AssetContainerType::Jar,
                container_path: client_jar,
            });
        }
    }

    Ok(containers)
}

fn scan_container(container: &ScanContainer) -> Result<Vec<AssetCandidate>, String> {
    match container.container_type {
        AssetContainerType::Directory => scan_directory_container(container),
        AssetContainerType::Zip | AssetContainerType::Jar => scan_archive_container(container),
    }
}

fn scan_directory_container(container: &ScanContainer) -> Result<Vec<AssetCandidate>, String> {
    let mut assets = Vec::new();

    for entry in WalkDir::new(&container.container_path)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        if !entry.file_type().is_file() {
            continue;
        }

        let Ok(relative) = entry.path().strip_prefix(&container.container_path) else {
            continue;
        };

        let relative_normalized = normalize_archive_path(relative);
        let Some(parsed) = parse_asset_relative_path(&relative_normalized) else {
            continue;
        };

        let extension = parsed
            .relative_asset_path
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();

        assets.push(AssetCandidate {
            source_type: container.source_type.clone(),
            source_name: container.source_name.clone(),
            namespace: parsed.namespace,
            relative_asset_path: parsed.relative_asset_path,
            container_path: container.container_path.clone(),
            container_type: container.container_type.clone(),
            entry_path: relative_normalized,
            is_image: is_image_extension(&extension),
            is_audio: is_audio_extension(&extension),
            extension,
        });
    }

    Ok(assets)
}

fn scan_archive_container(container: &ScanContainer) -> Result<Vec<AssetCandidate>, String> {
    let file = fs::File::open(&container.container_path)
        .map_err(|error| format!("Failed to open archive {}: {error}", container.container_path.display()))?;

    let mut archive = ZipArchive::new(file)
        .map_err(|error| format!("Failed to read archive {}: {error}", container.container_path.display()))?;

    let mut assets = Vec::new();

    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index(index) else {
            continue;
        };

        if entry.is_dir() {
            continue;
        }

        let path = normalize_archive_path(Path::new(entry.name()));
        let Some(parsed) = parse_asset_relative_path(&path) else {
            continue;
        };

        let extension = parsed
            .relative_asset_path
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();

        assets.push(AssetCandidate {
            source_type: container.source_type.clone(),
            source_name: container.source_name.clone(),
            namespace: parsed.namespace,
            relative_asset_path: parsed.relative_asset_path,
            container_path: container.container_path.clone(),
            container_type: container.container_type.clone(),
            entry_path: path,
            is_image: is_image_extension(&extension),
            is_audio: is_audio_extension(&extension),
            extension,
        });
    }

    Ok(assets)
}

#[derive(Debug, Clone)]
struct ParsedAssetPath {
    namespace: String,
    relative_asset_path: String,
}

fn parse_asset_relative_path(path: &str) -> Option<ParsedAssetPath> {
    let segments: Vec<&str> = path.split('/').filter(|segment| !segment.is_empty()).collect();
    let assets_index = segments.iter().position(|segment| *segment == "assets")?;

    if segments.len() <= assets_index + 2 {
        return None;
    }

    let namespace = segments.get(assets_index + 1)?.to_string();
    let relative_asset_path = segments[assets_index + 2..].join("/");

    if relative_asset_path.is_empty() {
        return None;
    }

    Some(ParsedAssetPath {
        namespace,
        relative_asset_path,
    })
}

fn normalize_archive_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn finalize_assets(
    candidates: Vec<AssetCandidate>,
    key_counts: &mut HashMap<String, usize>,
) -> Vec<AssetRecord> {
    candidates
        .into_iter()
        .map(|candidate| {
            let base_key = build_base_key(&candidate);
            let key = unique_key(base_key, key_counts);

            AssetRecord {
                asset_id: key.clone(),
                key,
                source_type: candidate.source_type,
                source_name: candidate.source_name,
                namespace: candidate.namespace,
                relative_asset_path: candidate.relative_asset_path,
                extension: candidate.extension,
                is_image: candidate.is_image,
                is_audio: candidate.is_audio,
                container_path: candidate.container_path.to_string_lossy().to_string(),
                container_type: candidate.container_type,
                entry_path: candidate.entry_path,
            }
        })
        .collect()
}

fn build_base_key(candidate: &AssetCandidate) -> String {
    let source = normalize_key_segment(&candidate.source_name);
    let namespace = normalize_key_segment(&candidate.namespace);
    let path = candidate
        .relative_asset_path
        .split('/')
        .map(normalize_key_segment)
        .collect::<Vec<_>>()
        .join(".");

    format!(
        "{}.{}.{}.{}",
        candidate.source_type.key_prefix(),
        source,
        namespace,
        path
    )
}

fn unique_key(base_key: String, key_counts: &mut HashMap<String, usize>) -> String {
    let counter = key_counts.entry(base_key.clone()).or_insert(0);
    if *counter == 0 {
        *counter = 1;
        return base_key;
    }

    let key = format!("{}.dup{}", base_key, *counter);
    *counter += 1;
    key
}

fn normalize_key_segment(value: &str) -> String {
    let mut output = String::new();
    let mut previous_was_separator = false;

    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            output.push(character.to_ascii_lowercase());
            previous_was_separator = false;
        } else if !previous_was_separator {
            output.push('_');
            previous_was_separator = true;
        }
    }

    output.trim_matches('_').to_string()
}

fn build_search_record(asset: &AssetRecord) -> AssetSearchRecord {
    let mut token_set = HashSet::new();

    for token in split_tokens(&asset.key) {
        token_set.insert(token);
    }

    for token in split_tokens(&asset.source_name) {
        token_set.insert(token);
    }

    for token in split_tokens(&asset.namespace) {
        token_set.insert(token);
    }

    for token in split_tokens(&asset.relative_asset_path) {
        token_set.insert(token);
    }

    let mut tokens = token_set.into_iter().collect::<Vec<_>>();
    tokens.sort();

    AssetSearchRecord {
        compact: compact_text(&format!(
            "{} {} {} {}",
            asset.key, asset.source_name, asset.namespace, asset.relative_asset_path
        )),
        key: asset.key.to_lowercase(),
        tokens,
    }
}

fn compact_text(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

fn split_tokens(value: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            current.push(character.to_ascii_lowercase());
        } else if !current.is_empty() {
            tokens.push(current.clone());
            current.clear();
        }
    }

    if !current.is_empty() {
        tokens.push(current);
    }

    tokens
}

fn score_query(
    index: &AssetSearchRecord,
    query_tokens: &[String],
    query_compact: &str,
    raw_query: &str,
) -> Option<i64> {
    let mut score = 0i64;

    for query_token in query_tokens {
        let mut token_score = 0i64;

        for token in &index.tokens {
            if token == query_token {
                token_score = token_score.max(140);
            } else if token.starts_with(query_token) {
                token_score = token_score.max(100);
            } else if token.contains(query_token) {
                token_score = token_score.max(75);
            }
        }

        if token_score == 0 {
            return None;
        }

        score += token_score;
    }

    if !query_compact.is_empty() && index.compact.contains(query_compact) {
        score += 120;
    }

    let lower_query = raw_query.to_ascii_lowercase();
    if index.key.contains(&lower_query) {
        score += 80;
    }

    Some(score)
}

fn add_asset_to_tree(tree_children: &mut HashMap<String, Vec<TreeNode>>, asset: &AssetRecord) {
    let mut parent_id = ROOT_NODE_ID.to_string();
    let mut folders = Vec::new();

    folders.push(asset.source_type.tree_root_name().to_string());
    folders.push(asset.source_name.clone());
    folders.push(asset.namespace.clone());

    let path = Path::new(&asset.relative_asset_path);
    if let Some(parent) = path.parent() {
        for segment in parent.iter() {
            folders.push(segment.to_string_lossy().to_string());
        }
    }

    for segment in folders {
        let node_name = if segment.is_empty() { "(root)" } else { &segment };
        let node_id = build_folder_node_id(&parent_id, node_name);

        upsert_tree_node(
            tree_children,
            &parent_id,
            TreeNode {
                id: node_id.clone(),
                name: node_name.to_string(),
                node_type: TreeNodeType::Folder,
                has_children: true,
                asset_id: None,
            },
        );

        tree_children.entry(node_id.clone()).or_default();
        parent_id = node_id;
    }

    let file_name = Path::new(&asset.relative_asset_path)
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| asset.relative_asset_path.clone());

    let file_node_id = format!("{parent_id}/file:{}", asset.asset_id);
    upsert_tree_node(
        tree_children,
        &parent_id,
        TreeNode {
            id: file_node_id,
            name: file_name,
            node_type: TreeNodeType::File,
            has_children: false,
            asset_id: Some(asset.asset_id.clone()),
        },
    );
}

fn build_folder_node_id(parent: &str, segment: &str) -> String {
    let escaped = segment.replace('/', "∕");
    if parent == ROOT_NODE_ID {
        format!("{ROOT_NODE_ID}/{escaped}")
    } else {
        format!("{parent}/{escaped}")
    }
}

fn upsert_tree_node(tree_children: &mut HashMap<String, Vec<TreeNode>>, parent_id: &str, node: TreeNode) {
    let children = tree_children.entry(parent_id.to_string()).or_default();
    if children.iter().any(|child| child.id == node.id) {
        return;
    }

    children.push(node);
}

fn collect_assets(
    state: &State<'_, AppState>,
    scan_id: &str,
    asset_ids: &[String],
) -> Result<Vec<AssetRecord>, String> {
    let scans = state
        .scans
        .lock()
        .map_err(|_| "Failed to lock scans state".to_string())?;

    let scan = scans
        .get(scan_id)
        .ok_or_else(|| format!("Unknown scan id: {scan_id}"))?;

    let mut assets = Vec::new();

    for asset_id in asset_ids {
        let index = scan
            .asset_index
            .get(asset_id)
            .ok_or_else(|| format!("Unknown asset id: {asset_id}"))?;

        assets.push(scan.assets[*index].clone());
    }

    Ok(assets)
}

fn get_asset_from_state(
    state: &State<'_, AppState>,
    scan_id: &str,
    asset_id: &str,
) -> Result<AssetRecord, String> {
    let mut asset_ids = Vec::new();
    asset_ids.push(asset_id.to_string());

    collect_assets(state, scan_id, &asset_ids)
        .map(|mut assets| assets.remove(0))
        .map_err(|error| error.to_string())
}

fn materialize_asset(
    app: &AppHandle,
    asset: &AssetRecord,
    destination_dir: &Path,
    audio_format: AudioFormat,
    used_names: &mut HashSet<String>,
) -> Result<PathBuf, String> {
    let original_name = Path::new(&asset.relative_asset_path)
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| asset.asset_id.clone());

    let (base_stem, mut extension) = split_file_name(&original_name);
    if asset.is_audio {
        match audio_format {
            AudioFormat::Original => {}
            AudioFormat::Mp3 => extension = "mp3".to_string(),
            AudioFormat::Wav => extension = "wav".to_string(),
        }
    }

    let target_name = dedupe_file_name(&base_stem, &extension, destination_dir, used_names);
    let output_path = destination_dir.join(target_name);

    if asset.is_audio && audio_format != AudioFormat::Original {
        convert_asset_audio_to_file(app, asset, &output_path, &audio_format)?;
        return Ok(output_path);
    }

    let bytes = extract_asset_bytes(asset)?;
    fs::write(&output_path, bytes)
        .map_err(|error| format!("Failed to write output file {}: {error}", output_path.display()))?;

    Ok(output_path)
}

fn split_file_name(file_name: &str) -> (String, String) {
    let path = Path::new(file_name);
    let stem = path
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "asset".to_string());
    let extension = path
        .extension()
        .map(|value| value.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    (stem, extension)
}

fn dedupe_file_name(
    base_stem: &str,
    extension: &str,
    destination_dir: &Path,
    used_names: &mut HashSet<String>,
) -> String {
    let mut index = 0;

    loop {
        let candidate = if index == 0 {
            if extension.is_empty() {
                base_stem.to_string()
            } else {
                format!("{base_stem}.{extension}")
            }
        } else if extension.is_empty() {
            format!("{base_stem}_{index}")
        } else {
            format!("{base_stem}_{index}.{extension}")
        };

        if used_names.contains(&candidate) || destination_dir.join(&candidate).exists() {
            index += 1;
            continue;
        }

        used_names.insert(candidate.clone());
        return candidate;
    }
}

fn convert_asset_audio_to_file(
    app: &AppHandle,
    asset: &AssetRecord,
    output_path: &Path,
    format: &AudioFormat,
) -> Result<(), String> {
    let ffmpeg_path = resolve_ffmpeg_path(app)?;

    let temp_input = output_path.with_extension(format!("{}.tmp", asset.extension));
    let bytes = extract_asset_bytes(asset)?;
    fs::write(&temp_input, bytes)
        .map_err(|error| format!("Failed to write temporary audio input: {error}"))?;

    let mut command = Command::new(&ffmpeg_path);
    command.arg("-y");
    command.arg("-i");
    command.arg(&temp_input);
    command.arg("-vn");

    match format {
        AudioFormat::Original => {
            command.arg("-c:a");
            command.arg("copy");
        }
        AudioFormat::Mp3 => {
            command.arg("-c:a");
            command.arg("libmp3lame");
            command.arg("-q:a");
            command.arg("2");
        }
        AudioFormat::Wav => {
            command.arg("-c:a");
            command.arg("pcm_s16le");
        }
    }

    command.arg(output_path);

    let status = command
        .status()
        .map_err(|error| format!("Failed to start ffmpeg: {error}"))?;

    let _ = fs::remove_file(&temp_input);

    if !status.success() {
        return Err(
            "FFmpeg conversion failed. Install FFmpeg or retry download in app settings.".to_string(),
        );
    }

    Ok(())
}

fn resolve_ffmpeg_path(app: &AppHandle) -> Result<PathBuf, String> {
    if ffmpeg_works(Path::new("ffmpeg")) {
        return Ok(PathBuf::from("ffmpeg"));
    }

    let base_dir = app
        .path()
        .app_cache_dir()
        .map_err(|error| format!("Failed to resolve app cache directory: {error}"))?
        .join("ffmpeg-runtime");

    fs::create_dir_all(&base_dir)
        .map_err(|error| format!("Failed to create FFmpeg runtime directory: {error}"))?;

    let ffmpeg_binary = if cfg!(windows) {
        base_dir.join("ffmpeg.exe")
    } else {
        base_dir.join("ffmpeg")
    };

    if ffmpeg_works(&ffmpeg_binary) {
        return Ok(ffmpeg_binary);
    }

    let url = ffmpeg_download_url().map_err(|error| format!("Failed to resolve FFmpeg URL: {error}"))?;
    let archive_path = download_ffmpeg_package(url, &base_dir)
        .map_err(|error| format!("Failed to download FFmpeg runtime: {error}"))?;
    unpack_ffmpeg(&archive_path, &base_dir)
        .map_err(|error| format!("Failed to unpack FFmpeg runtime: {error}"))?;

    if !ffmpeg_works(&ffmpeg_binary) {
        return Err(
            "FFmpeg was downloaded but is not executable. Install FFmpeg manually and add it to PATH."
                .to_string(),
        );
    }

    Ok(ffmpeg_binary)
}

fn ffmpeg_works(path: &Path) -> bool {
    let mut command = Command::new(path);
    command.arg("-version");

    command
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn extract_asset_bytes(asset: &AssetRecord) -> Result<Vec<u8>, String> {
    let container_path = PathBuf::from(&asset.container_path);

    match asset.container_type {
        AssetContainerType::Directory => {
            let file_path = container_path.join(Path::new(&asset.entry_path));
            fs::read(&file_path)
                .map_err(|error| format!("Failed to read file {}: {error}", file_path.display()))
        }
        AssetContainerType::Zip | AssetContainerType::Jar => {
            let file = fs::File::open(&container_path).map_err(|error| {
                format!(
                    "Failed to open archive {}: {error}",
                    container_path.display()
                )
            })?;

            let mut archive = ZipArchive::new(file).map_err(|error| {
                format!(
                    "Failed to read archive {}: {error}",
                    container_path.display()
                )
            })?;

            let mut entry = archive
                .by_name(&asset.entry_path)
                .map_err(|error| format!("Failed to open archive entry {}: {error}", asset.entry_path))?;

            let mut buffer = Vec::new();
            entry
                .read_to_end(&mut buffer)
                .map_err(|error| format!("Failed to read archive entry {}: {error}", asset.entry_path))?;

            Ok(buffer)
        }
    }
}

fn mime_for_extension(extension: &str) -> &'static str {
    match extension {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "ico" => "image/x-icon",
        "tga" => "image/x-tga",
        _ => "application/octet-stream",
    }
}

fn is_image_extension(extension: &str) -> bool {
    matches!(
        extension,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tga" | "tif" | "tiff" | "ico"
    )
}

fn is_audio_extension(extension: &str) -> bool {
    matches!(
        extension,
        "ogg" | "wav" | "mp3" | "flac" | "m4a" | "aac" | "opus" | "oga"
    )
}

fn dedupe_candidates(candidates: Vec<PrismRootCandidate>) -> Result<Vec<PrismRootCandidate>, String> {
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

fn resolve_instance_dir(prism_root: &Path, instance_folder: &str) -> Result<PathBuf, String> {
    let requested = expand_home(instance_folder);
    if requested.is_dir()
        && requested
            .parent()
            .map(|parent| parent.ends_with("instances"))
            .unwrap_or(false)
    {
        return Ok(requested);
    }

    let folder_name = Path::new(instance_folder)
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| instance_folder.to_string());

    let path = prism_root.join("instances").join(folder_name);
    if !path.is_dir() {
        return Err(format!("Instance directory not found: {}", path.display()));
    }

    Ok(path)
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

fn cleanup_temp_paths(state: &AppState) {
    let Ok(mut paths) = state.temp_paths.lock() else {
        return;
    };

    for path in &*paths {
        if path.is_dir() {
            let _ = fs::remove_dir_all(path);
        } else if path.is_file() {
            let _ = fs::remove_file(path);
        }
    }

    paths.clear();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .manage(AppState::default())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            detect_prism_roots,
            list_instances,
            start_scan,
            get_scan_status,
            cancel_scan,
            list_tree_children,
            search_assets,
            get_asset_preview,
            save_assets,
            copy_assets_to_clipboard,
            convert_audio_asset,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|app_handle, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            let state = app_handle.state::<AppState>();
            cleanup_temp_paths(&state);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minecraft_component_version() {
        let data = r#"{
            "components": [
                { "uid": "foo", "version": "1.0.0" },
                { "uid": "net.minecraft", "version": "1.20.1" }
            ]
        }"#;

        let parsed: MmcPack = serde_json::from_str(data).expect("valid json");
        let version = parsed
            .components
            .into_iter()
            .find(|component| component.uid == "net.minecraft")
            .and_then(|component| component.version);

        assert_eq!(version.as_deref(), Some("1.20.1"));
    }

    #[test]
    fn smart_search_scores_atm_star_query() {
        let record = AssetSearchRecord {
            tokens: split_tokens("allthemodium.textures.item.atm_star.png"),
            compact: compact_text("allthemodium textures item atm_star png"),
            key: "allthemodium.textures.item.atm_star.png".to_string(),
        };

        let tokens = split_tokens("atm star");
        let score = score_query(&record, &tokens, &compact_text("atm star"), "atm star");

        assert!(score.is_some());
    }

    #[test]
    fn parse_assets_path_from_nested_prefix() {
        let parsed = parse_asset_relative_path("nested/content/assets/example/textures/item/star.png")
            .expect("must parse");

        assert_eq!(parsed.namespace, "example");
        assert_eq!(parsed.relative_asset_path, "textures/item/star.png");
    }
}
