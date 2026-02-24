use base64::Engine;
use clipboard_rs::{Clipboard, ClipboardContext};
use ffmpeg_sidecar::download::{download_ffmpeg_package, ffmpeg_download_url, unpack_ffmpeg};
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering as CmpOrdering,
    collections::{HashMap, HashSet},
    env, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicUsize, Ordering as AtomicOrdering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use strsim::damerau_levenshtein;
use tauri::{
    menu::{MenuBuilder, SubmenuBuilder},
    AppHandle, Emitter, Manager, State,
};
use uuid::Uuid;
use walkdir::WalkDir;
use zip::ZipArchive;

const ROOT_NODE_ID: &str = "root";
const MAX_SCAN_WORKERS: usize = 4;
const MAX_EXPORT_WORKERS: usize = 16;
const SCAN_CACHE_SCHEMA_VERSION: u32 = 1;
const SCAN_CACHE_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const SCAN_CANCEL_CHECK_INTERVAL: usize = 128;

#[derive(Default)]
struct AppState {
    scans: Mutex<HashMap<String, ScanState>>,
    export_operations: Mutex<HashMap<String, ExportOperationState>>,
    temp_paths: Mutex<Vec<PathBuf>>,
}

#[derive(Debug, Clone)]
struct ExportOperationState {
    cancelled: bool,
}

impl ExportOperationState {
    fn new() -> Self {
        Self { cancelled: false }
    }
}

#[derive(Debug, Clone)]
struct ScanState {
    status: ScanLifecycle,
    is_refreshing: bool,
    scanned_containers: usize,
    total_containers: usize,
    error: Option<String>,
    cancelled: bool,
    assets: Vec<AssetRecord>,
    asset_index: HashMap<String, usize>,
    search_records: Vec<AssetSearchRecord>,
    tree_children: HashMap<String, Vec<TreeNode>>,
    container_assets: HashMap<String, Vec<AssetRecord>>,
    container_signatures: HashMap<String, ContainerSignature>,
    id_aliases: HashMap<String, String>,
    cache_key: Option<String>,
    last_progress_emit_at: Option<Instant>,
}

impl ScanState {
    fn new() -> Self {
        let mut tree_children = HashMap::new();
        tree_children.insert(ROOT_NODE_ID.to_string(), Vec::new());

        Self {
            status: ScanLifecycle::Scanning,
            is_refreshing: false,
            scanned_containers: 0,
            total_containers: 0,
            error: None,
            cancelled: false,
            assets: Vec::new(),
            asset_index: HashMap::new(),
            search_records: Vec::new(),
            tree_children,
            container_assets: HashMap::new(),
            container_signatures: HashMap::new(),
            id_aliases: HashMap::new(),
            cache_key: None,
            last_progress_emit_at: None,
        }
    }

    fn as_status(&self, scan_id: &str) -> ScanStatus {
        ScanStatus {
            scan_id: scan_id.to_string(),
            lifecycle: self.status.clone(),
            is_refreshing: self.is_refreshing,
            scanned_containers: self.scanned_containers,
            total_containers: self.total_containers,
            asset_count: self.assets.len(),
            error: self.error.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AssetSearchRecord {
    all_tokens: Vec<String>,
    filename_tokens: Vec<String>,
    path_tokens: Vec<String>,
    namespace_tokens: Vec<String>,
    source_tokens: Vec<String>,
    compact_all: String,
    compact_filename: String,
    compact_filename_stem: String,
    key: String,
    folder_node_id: String,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum AssetContainerType {
    Directory,
    Zip,
    Jar,
    AssetIndex,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TreeNode {
    id: String,
    name: String,
    node_type: TreeNodeType,
    has_children: bool,
    asset_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    force_rescan: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StartScanResponse {
    scan_id: String,
    cache_hit: bool,
    refresh_started: bool,
    refresh_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanStatus {
    scan_id: String,
    lifecycle: ScanLifecycle,
    is_refreshing: bool,
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
#[serde(rename_all = "lowercase")]
enum ScanPhase {
    Estimating,
    Scanning,
    Refreshing,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanProgressEvent {
    scan_id: String,
    scanned_containers: usize,
    total_containers: usize,
    asset_count: usize,
    phase: ScanPhase,
    current_source: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ScanCompletedEvent {
    scan_id: String,
    lifecycle: ScanLifecycle,
    asset_count: usize,
    error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
enum ExportOperationKind {
    Save,
    Copy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchRequest {
    scan_id: String,
    query: String,
    offset: Option<usize>,
    limit: Option<usize>,
    folder_node_id: Option<String>,
    include_images: Option<bool>,
    include_audio: Option<bool>,
    include_other: Option<bool>,
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
    operation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CopyAssetsRequest {
    scan_id: String,
    asset_ids: Vec<String>,
    audio_format: Option<AudioFormat>,
    operation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportFailure {
    asset_id: String,
    key: String,
    error: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportProgressEvent {
    operation_id: String,
    kind: ExportOperationKind,
    requested_count: usize,
    processed_count: usize,
    success_count: usize,
    failed_count: usize,
    cancelled: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportCompletedEvent {
    operation_id: String,
    kind: ExportOperationKind,
    requested_count: usize,
    processed_count: usize,
    success_count: usize,
    failed_count: usize,
    cancelled: bool,
    failures: Vec<ExportFailure>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveAssetsResult {
    operation_id: String,
    requested_count: usize,
    processed_count: usize,
    success_count: usize,
    failed_count: usize,
    cancelled: bool,
    failures: Vec<ExportFailure>,
    saved_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CopyResult {
    operation_id: String,
    requested_count: usize,
    processed_count: usize,
    success_count: usize,
    failed_count: usize,
    cancelled: bool,
    failures: Vec<ExportFailure>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReconcileAssetIdsRequest {
    scan_id: String,
    asset_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReconcileAssetIdsResponse {
    id_map: HashMap<String, String>,
    asset_ids: Vec<String>,
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MinecraftMetaVersion {
    asset_index: Option<MinecraftMetaAssetIndex>,
    assets: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MinecraftMetaAssetIndex {
    id: String,
}

#[derive(Debug, Deserialize)]
struct MinecraftAssetIndexFile {
    objects: HashMap<String, MinecraftAssetIndexObject>,
}

#[derive(Debug, Deserialize)]
struct MinecraftAssetIndexObject {
    hash: String,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct ContainerSignature {
    kind: AssetContainerType,
    path: String,
    mtime_ms: u64,
    size: u64,
    file_count: u64,
    newest_mtime_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScanSnapshot {
    schema_version: u32,
    cache_key: String,
    prism_root: String,
    instance_folder: String,
    include_vanilla: bool,
    include_mods: bool,
    include_resourcepacks: bool,
    created_at: u64,
    last_used_at: u64,
    app_version: String,
    assets: Vec<AssetRecord>,
    search_records: Vec<AssetSearchRecord>,
    tree_children: HashMap<String, Vec<TreeNode>>,
    container_assets: HashMap<String, Vec<AssetRecord>>,
    container_signatures: HashMap<String, ContainerSignature>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScanCacheManifest {
    schema_version: u32,
    entries: HashMap<String, ScanCacheManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScanCacheManifestEntry {
    file_name: String,
    size_bytes: u64,
    last_accessed_at: u64,
}

impl Default for ScanCacheManifest {
    fn default() -> Self {
        Self {
            schema_version: SCAN_CACHE_SCHEMA_VERSION,
            entries: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
struct ScanRefreshPlan {
    unchanged_keys: Vec<String>,
    changed_or_new: Vec<ScanContainer>,
    removed_keys: Vec<String>,
    signatures_by_key: HashMap<String, ContainerSignature>,
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
        candidates.push(build_candidate(
            PathBuf::from(custom_root),
            "env-prism-root",
        ));
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
        if folder_name.starts_with('.') {
            continue;
        }

        // Real Prism instances should contain profile metadata and minecraft folder.
        if !instance_path.join("mmc-pack.json").is_file()
            || !instance_path.join("minecraft").is_dir()
        {
            continue;
        }

        let display_name =
            instance_display_name(&instance_path).unwrap_or_else(|| folder_name.clone());
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

fn unix_timestamp_ms() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis().min(u128::from(u64::MAX)) as u64,
        Err(_) => 0,
    }
}

fn file_mtime_ms(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn scan_container_key(container: &ScanContainer) -> String {
    format!(
        "{}::{}::{}::{}",
        container.source_type.key_prefix(),
        container.source_name,
        container_type_key(&container.container_type),
        container.container_path.to_string_lossy()
    )
}

fn container_type_key(container_type: &AssetContainerType) -> &'static str {
    match container_type {
        AssetContainerType::Directory => "directory",
        AssetContainerType::Zip => "zip",
        AssetContainerType::Jar => "jar",
        AssetContainerType::AssetIndex => "asset_index",
    }
}

fn scan_cache_key_for_request(req: &StartScanRequest) -> String {
    let prism_root = expand_home(&req.prism_root);
    let prism_root = prism_root.to_string_lossy();
    format!(
        "{}::{}::{}{}{}",
        prism_root,
        req.instance_folder.trim(),
        if req.include_vanilla { 'v' } else { '-' },
        if req.include_mods { 'm' } else { '-' },
        if req.include_resourcepacks { 'r' } else { '-' },
    )
}

fn fnv1a64(value: &str) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut hash = OFFSET_BASIS;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash
}

fn scan_cache_root(app: &AppHandle) -> Result<PathBuf, String> {
    let root = app
        .path()
        .app_data_dir()
        .map_err(|error| format!("Failed to resolve app data directory: {error}"))?
        .join("scan-cache")
        .join("v1");
    fs::create_dir_all(&root)
        .map_err(|error| format!("Failed to create scan cache directory: {error}"))?;
    Ok(root)
}

fn scan_cache_manifest_path(cache_root: &Path) -> PathBuf {
    cache_root.join("manifest.json")
}

fn scan_cache_snapshot_file_name(cache_key: &str) -> String {
    format!("{:016x}.json", fnv1a64(cache_key))
}

fn scan_cache_snapshot_path(cache_root: &Path, cache_key: &str) -> PathBuf {
    cache_root.join(scan_cache_snapshot_file_name(cache_key))
}

fn write_json_atomically<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let temp_path = path.with_extension(format!("tmp-{}", Uuid::new_v4()));
    let bytes = serde_json::to_vec(value)
        .map_err(|error| format!("Failed to serialize {}: {error}", path.display()))?;
    fs::write(&temp_path, bytes)
        .map_err(|error| format!("Failed to write {}: {error}", temp_path.display()))?;
    fs::rename(&temp_path, path)
        .map_err(|error| format!("Failed to replace {}: {error}", path.display()))?;
    Ok(())
}

fn load_scan_cache_manifest(cache_root: &Path) -> Result<ScanCacheManifest, String> {
    let manifest_path = scan_cache_manifest_path(cache_root);
    if !manifest_path.is_file() {
        return Ok(ScanCacheManifest::default());
    }

    let data = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("Failed to read cache manifest: {error}"))?;
    let parsed: ScanCacheManifest = serde_json::from_str(&data)
        .map_err(|error| format!("Failed to parse cache manifest: {error}"))?;
    if parsed.schema_version != SCAN_CACHE_SCHEMA_VERSION {
        return Ok(ScanCacheManifest::default());
    }
    Ok(parsed)
}

fn save_scan_cache_manifest(cache_root: &Path, manifest: &ScanCacheManifest) -> Result<(), String> {
    write_json_atomically(&scan_cache_manifest_path(cache_root), manifest)
}

fn remove_cache_entry(cache_root: &Path, manifest: &mut ScanCacheManifest, cache_key: &str) {
    let file_name = scan_cache_snapshot_file_name(cache_key);
    let path = cache_root.join(&file_name);
    let _ = fs::remove_file(path);
    manifest.entries.remove(cache_key);
}

fn prune_scan_cache(cache_root: &Path, manifest: &mut ScanCacheManifest) {
    let mut total_size = manifest
        .entries
        .values()
        .map(|entry| entry.size_bytes)
        .sum::<u64>();
    if total_size <= SCAN_CACHE_MAX_BYTES {
        return;
    }

    let mut eviction_order = manifest
        .entries
        .iter()
        .map(|(cache_key, entry)| (cache_key.clone(), entry.last_accessed_at))
        .collect::<Vec<_>>();
    eviction_order.sort_by(|left, right| left.1.cmp(&right.1));

    for (cache_key, _) in eviction_order {
        let Some(entry) = manifest.entries.remove(&cache_key) else {
            continue;
        };
        let path = cache_root.join(&entry.file_name);
        let _ = fs::remove_file(path);
        total_size = total_size.saturating_sub(entry.size_bytes);
        if total_size <= SCAN_CACHE_MAX_BYTES {
            break;
        }
    }
}

fn load_cached_snapshot(app: &AppHandle, cache_key: &str) -> Result<Option<ScanSnapshot>, String> {
    let cache_root = scan_cache_root(app)?;
    let mut manifest = load_scan_cache_manifest(&cache_root)?;
    let snapshot_path = scan_cache_snapshot_path(&cache_root, cache_key);
    if !snapshot_path.is_file() {
        manifest.entries.remove(cache_key);
        let _ = save_scan_cache_manifest(&cache_root, &manifest);
        return Ok(None);
    }

    let data = match fs::read_to_string(&snapshot_path) {
        Ok(value) => value,
        Err(_) => {
            remove_cache_entry(&cache_root, &mut manifest, cache_key);
            let _ = save_scan_cache_manifest(&cache_root, &manifest);
            return Ok(None);
        }
    };
    let parsed: ScanSnapshot = match serde_json::from_str(&data) {
        Ok(value) => value,
        Err(_) => {
            remove_cache_entry(&cache_root, &mut manifest, cache_key);
            let _ = save_scan_cache_manifest(&cache_root, &manifest);
            return Ok(None);
        }
    };
    if parsed.schema_version != SCAN_CACHE_SCHEMA_VERSION {
        remove_cache_entry(&cache_root, &mut manifest, cache_key);
        let _ = save_scan_cache_manifest(&cache_root, &manifest);
        return Ok(None);
    }

    let now = unix_timestamp_ms();
    let entry = manifest
        .entries
        .entry(cache_key.to_string())
        .or_insert(ScanCacheManifestEntry {
            file_name: scan_cache_snapshot_file_name(cache_key),
            size_bytes: fs::metadata(&snapshot_path).map(|meta| meta.len()).unwrap_or(0),
            last_accessed_at: now,
        });
    entry.last_accessed_at = now;
    let _ = save_scan_cache_manifest(&cache_root, &manifest);
    Ok(Some(parsed))
}

fn save_snapshot_to_cache(app: &AppHandle, snapshot: &ScanSnapshot) -> Result<(), String> {
    let cache_root = scan_cache_root(app)?;
    let mut manifest = load_scan_cache_manifest(&cache_root)?;
    let snapshot_path = scan_cache_snapshot_path(&cache_root, &snapshot.cache_key);
    write_json_atomically(&snapshot_path, snapshot)?;
    let size_bytes = fs::metadata(&snapshot_path)
        .map(|meta| meta.len())
        .map_err(|error| format!("Failed to stat cache snapshot {}: {error}", snapshot_path.display()))?;
    manifest.entries.insert(
        snapshot.cache_key.clone(),
        ScanCacheManifestEntry {
            file_name: scan_cache_snapshot_file_name(&snapshot.cache_key),
            size_bytes,
            last_accessed_at: unix_timestamp_ms(),
        },
    );
    prune_scan_cache(&cache_root, &mut manifest);
    save_scan_cache_manifest(&cache_root, &manifest)
}

fn container_signature_for_path(
    container_path: &Path,
    container_type: &AssetContainerType,
) -> Result<ContainerSignature, String> {
    let metadata = fs::metadata(container_path)
        .map_err(|error| format!("Failed to read metadata {}: {error}", container_path.display()))?;
    let mut file_count = 0u64;
    let mut total_size: u64;
    let mut newest_mtime_ms: u64;

    if matches!(container_type, AssetContainerType::Directory) {
        total_size = 0;
        file_count = 0;
        newest_mtime_ms = 0;
        for entry in WalkDir::new(container_path)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                file_count = file_count.saturating_add(1);
                total_size = total_size.saturating_add(meta.len());
                newest_mtime_ms = newest_mtime_ms.max(file_mtime_ms(&meta));
            }
        }
    } else {
        total_size = metadata.len();
        newest_mtime_ms = file_mtime_ms(&metadata);
    }

    Ok(ContainerSignature {
        kind: container_type.clone(),
        path: container_path.to_string_lossy().to_string(),
        mtime_ms: file_mtime_ms(&metadata),
        size: total_size,
        file_count,
        newest_mtime_ms,
    })
}

fn build_scan_refresh_plan(
    cached_signatures: &HashMap<String, ContainerSignature>,
    current_containers: &[ScanContainer],
) -> Result<ScanRefreshPlan, String> {
    let mut unchanged_keys = Vec::new();
    let mut changed_or_new = Vec::new();
    let mut signatures_by_key = HashMap::new();

    for container in current_containers {
        let key = scan_container_key(container);
        let signature = container_signature_for_path(&container.container_path, &container.container_type)?;
        let is_unchanged = cached_signatures
            .get(&key)
            .map(|cached| cached == &signature)
            .unwrap_or(false);
        if is_unchanged {
            unchanged_keys.push(key.clone());
        } else {
            changed_or_new.push(container.clone());
        }
        signatures_by_key.insert(key, signature);
    }

    let mut removed_keys = cached_signatures
        .keys()
        .filter(|key| !signatures_by_key.contains_key(*key))
        .cloned()
        .collect::<Vec<_>>();
    removed_keys.sort();
    unchanged_keys.sort();
    changed_or_new.sort_by(|left, right| scan_container_key(left).cmp(&scan_container_key(right)));

    Ok(ScanRefreshPlan {
        unchanged_keys,
        changed_or_new,
        removed_keys,
        signatures_by_key,
    })
}

fn asset_identity(asset: &AssetRecord) -> String {
    format!(
        "{}::{}::{}::{}::{}::{}",
        asset.source_type.key_prefix(),
        asset.source_name,
        asset.namespace,
        asset.relative_asset_path,
        asset.container_path,
        asset.entry_path
    )
}

fn build_asset_reconciliation_map(
    previous_assets: &[AssetRecord],
    next_assets: &[AssetRecord],
) -> HashMap<String, String> {
    let mut next_by_id = HashSet::new();
    let mut next_by_identity = HashMap::<String, Vec<String>>::new();
    for asset in next_assets {
        next_by_id.insert(asset.asset_id.clone());
        next_by_identity
            .entry(asset_identity(asset))
            .or_default()
            .push(asset.asset_id.clone());
    }

    let mut identity_cursor = HashMap::<String, usize>::new();
    let mut id_map = HashMap::<String, String>::new();
    for asset in previous_assets {
        if next_by_id.contains(&asset.asset_id) {
            id_map.insert(asset.asset_id.clone(), asset.asset_id.clone());
            continue;
        }

        let identity = asset_identity(asset);
        let Some(candidates) = next_by_identity.get(&identity) else {
            continue;
        };
        let cursor = identity_cursor.entry(identity).or_insert(0);
        let resolved = candidates
            .get(*cursor)
            .or_else(|| candidates.last())
            .cloned();
        if let Some(mapped_id) = resolved {
            id_map.insert(asset.asset_id.clone(), mapped_id);
            *cursor = cursor.saturating_add(1);
        }
    }

    id_map
}

#[tauri::command]
fn start_scan(
    app: AppHandle,
    state: State<'_, AppState>,
    req: StartScanRequest,
) -> Result<StartScanResponse, String> {
    let scan_id = Uuid::new_v4().to_string();
    let force_rescan = req.force_rescan.unwrap_or(false);
    let cache_key = scan_cache_key_for_request(&req);

    {
        let mut scans = state
            .scans
            .lock()
            .map_err(|_| "Failed to lock scans state".to_string())?;

        scans.insert(scan_id.clone(), ScanState::new());
    }

    let _ = app.emit(
        "scan://started",
        serde_json::json!({
            "scanId": scan_id,
        }),
    );

    emit_scan_progress(
        &app,
        ScanProgressEvent {
            scan_id: scan_id.clone(),
            scanned_containers: 0,
            total_containers: 0,
            asset_count: 0,
            phase: ScanPhase::Estimating,
            current_source: None,
        },
    );

    let scan_id_for_worker = scan_id.clone();
    let app_for_worker = app.clone();
    let req_for_worker = req.clone();
    let cache_key_for_worker = cache_key.clone();
    thread::spawn(move || {
        run_scan_bootstrap_worker(
            app_for_worker,
            scan_id_for_worker,
            req_for_worker,
            cache_key_for_worker,
            force_rescan,
        );
    });

    Ok(StartScanResponse {
        scan_id,
        cache_hit: false,
        refresh_started: false,
        refresh_mode: None,
    })
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
    scan.is_refreshing = false;
    scan.status = ScanLifecycle::Cancelled;
    Ok(())
}

#[tauri::command]
fn cancel_export(operation_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let mut operations = state
        .export_operations
        .lock()
        .map_err(|_| "Failed to lock export operations state".to_string())?;

    let operation = operations
        .get_mut(&operation_id)
        .ok_or_else(|| format!("Unknown export operation id: {operation_id}"))?;

    operation.cancelled = true;
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
    let limit = req.limit.unwrap_or(200).clamp(1, 1000);
    let include_images = req.include_images.unwrap_or(true);
    let include_audio = req.include_audio.unwrap_or(true);
    let include_other = req.include_other.unwrap_or(true);
    let folder_filter = req
        .folder_node_id
        .as_deref()
        .filter(|value| !value.trim().is_empty() && *value != ROOT_NODE_ID);
    let query_tokens = split_tokens(&req.query);
    let query_compact = compact_text(&req.query);
    let normalized_query = query_tokens.join(" ");

    if !(include_images || include_audio || include_other) {
        return Ok(SearchResponse {
            total: 0,
            assets: Vec::new(),
        });
    }

    if query_tokens.is_empty() {
        let mut matched = Vec::<usize>::new();
        for (index, asset) in scan.assets.iter().enumerate() {
            if !asset_matches_media(asset, include_images, include_audio, include_other) {
                continue;
            }
            let search_record = &scan.search_records[index];
            if !asset_matches_folder(search_record, folder_filter) {
                continue;
            }
            matched.push(index);
        }

        matched.sort_unstable_by(|left, right| {
            idle_asset_cmp(&scan.assets[*left], &scan.assets[*right])
        });
        let total = matched.len();
        let assets = matched
            .into_iter()
            .skip(offset)
            .take(limit)
            .map(|index| scan.assets[index].clone())
            .collect();

        return Ok(SearchResponse { total, assets });
    }

    let mut ranked = Vec::new();
    for (index, asset) in scan.assets.iter().enumerate() {
        if !asset_matches_media(asset, include_images, include_audio, include_other) {
            continue;
        }

        let search_record = &scan.search_records[index];
        if !asset_matches_folder(search_record, folder_filter) {
            continue;
        }

        if let Some(score) = score_query(
            search_record,
            &query_tokens,
            &query_compact,
            &normalized_query,
        ) {
            ranked.push((score, index));
        }
    }

    let total = ranked.len();
    let wanted = offset.saturating_add(limit).max(1);
    if ranked.len() > wanted {
        ranked.select_nth_unstable_by(wanted - 1, |left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| scan.assets[left.1].key.cmp(&scan.assets[right.1].key))
        });
        ranked.truncate(wanted);
    }

    ranked.sort_unstable_by(|left, right| {
        right
            .0
            .cmp(&left.0)
            .then_with(|| scan.assets[left.1].key.cmp(&scan.assets[right.1].key))
    });

    let assets = ranked
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|(_, index)| scan.assets[index].clone())
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

    if !asset.is_image && !asset.is_audio && !is_json_extension(&asset.extension) {
        return Err("Preview is only available for image, audio or JSON assets".to_string());
    }

    let bytes = extract_asset_bytes(&asset)?;
    let base64 = base64::engine::general_purpose::STANDARD.encode(bytes);

    Ok(AssetPreviewResponse {
        mime: mime_for_extension(&asset.extension).to_string(),
        base64,
    })
}

#[tauri::command]
fn get_asset_record(
    scan_id: String,
    asset_id: String,
    state: State<'_, AppState>,
) -> Result<AssetRecord, String> {
    get_asset_from_state(&state, &scan_id, &asset_id)
}

#[tauri::command]
fn reconcile_asset_ids(
    req: ReconcileAssetIdsRequest,
    state: State<'_, AppState>,
) -> Result<ReconcileAssetIdsResponse, String> {
    let scans = state
        .scans
        .lock()
        .map_err(|_| "Failed to lock scans state".to_string())?;
    let scan = scans
        .get(&req.scan_id)
        .ok_or_else(|| format!("Unknown scan id: {}", req.scan_id))?;

    let mut id_map = HashMap::<String, String>::new();
    let mut resolved = Vec::<String>::new();
    let mut seen = HashSet::<String>::new();

    for asset_id in req.asset_ids {
        let mapped = if scan.asset_index.contains_key(&asset_id) {
            Some(asset_id.clone())
        } else {
            scan.id_aliases
                .get(&asset_id)
                .filter(|next| scan.asset_index.contains_key(*next))
                .cloned()
        };

        if let Some(mapped_id) = mapped {
            id_map.insert(asset_id, mapped_id.clone());
            if seen.insert(mapped_id.clone()) {
                resolved.push(mapped_id);
            }
        }
    }

    Ok(ReconcileAssetIdsResponse {
        id_map,
        asset_ids: resolved,
    })
}

#[tauri::command]
fn save_assets(
    app: AppHandle,
    req: SaveAssetsRequest,
    state: State<'_, AppState>,
) -> Result<SaveAssetsResult, String> {
    let operation_id = resolve_operation_id(req.operation_id);
    let requested_count = req.asset_ids.len();

    if req.asset_ids.is_empty() {
        return Ok(SaveAssetsResult {
            operation_id,
            requested_count,
            processed_count: 0,
            success_count: 0,
            failed_count: 0,
            cancelled: false,
            failures: Vec::new(),
            saved_files: Vec::new(),
        });
    }

    let destination_dir = expand_home(&req.destination_dir);
    fs::create_dir_all(&destination_dir)
        .map_err(|error| format!("Failed to create destination directory: {error}"))?;

    let requested_assets = collect_assets(&state, &req.scan_id, &req.asset_ids)?;
    register_export_operation(&state, &operation_id)?;

    let run_result = run_export_operation(
        &app,
        ExportOperationKind::Save,
        &operation_id,
        requested_assets,
        &destination_dir,
        req.audio_format.unwrap_or(AudioFormat::Original),
    );

    unregister_export_operation(&state, &operation_id);

    let outcome = run_result?;
    Ok(SaveAssetsResult {
        operation_id,
        requested_count,
        processed_count: outcome.processed_count,
        success_count: outcome.success_count,
        failed_count: outcome.failed_count,
        cancelled: outcome.cancelled,
        failures: outcome.failures,
        saved_files: outcome.output_files,
    })
}

#[tauri::command]
fn copy_assets_to_clipboard(
    app: AppHandle,
    req: CopyAssetsRequest,
    state: State<'_, AppState>,
) -> Result<CopyResult, String> {
    let operation_id = resolve_operation_id(req.operation_id);
    let requested_count = req.asset_ids.len();

    if req.asset_ids.is_empty() {
        return Ok(CopyResult {
            operation_id,
            requested_count,
            processed_count: 0,
            success_count: 0,
            failed_count: 0,
            cancelled: false,
            failures: Vec::new(),
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

    register_export_operation(&state, &operation_id)?;

    let run_result = run_export_operation(
        &app,
        ExportOperationKind::Copy,
        &operation_id,
        requested_assets,
        &temp_root,
        req.audio_format.unwrap_or(AudioFormat::Original),
    );

    unregister_export_operation(&state, &operation_id);

    let outcome = run_result?;
    let copied_paths: Vec<PathBuf> = outcome.output_files.iter().map(PathBuf::from).collect();

    let clipboard = ClipboardContext::new()
        .map_err(|error| format!("Failed to open clipboard context: {error}"))?;

    if !copied_paths.is_empty() {
        clipboard
            .set_files(
                copied_paths
                    .iter()
                    .map(|path| path.to_string_lossy().to_string())
                    .collect(),
            )
            .map_err(|error| format!("Failed to copy files to clipboard: {error}"))?;
    }

    {
        let mut temp_paths = state
            .temp_paths
            .lock()
            .map_err(|_| "Failed to lock temp paths".to_string())?;
        temp_paths.push(temp_root);
    }

    Ok(CopyResult {
        operation_id,
        requested_count,
        processed_count: outcome.processed_count,
        success_count: outcome.success_count,
        failed_count: outcome.failed_count,
        cancelled: outcome.cancelled,
        failures: outcome.failures,
        copied_files: outcome.output_files,
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

    let original_name = Path::new(&asset.relative_asset_path)
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| asset.asset_id.clone());
    let (base_stem, _) = split_file_name(&original_name);
    let extension = match req.format {
        AudioFormat::Original => asset.extension.clone(),
        AudioFormat::Mp3 => "mp3".to_string(),
        AudioFormat::Wav => "wav".to_string(),
    };

    let mut used_names = HashSet::new();
    let output_name = dedupe_file_name(&base_stem, &extension, &temp_root, &mut used_names);
    let output_path = temp_root.join(output_name);

    let ffmpeg_path = resolve_ffmpeg_path(&app)?;
    let mut archive_cache = HashMap::<String, ZipArchive<fs::File>>::new();
    let bytes = extract_asset_bytes_with_archive_cache(&asset, &mut archive_cache)?;
    convert_audio_bytes_to_file(&ffmpeg_path, &bytes, &output_path, &req.format)?;

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

fn run_scan_bootstrap_worker(
    app: AppHandle,
    scan_id: String,
    req: StartScanRequest,
    cache_key: String,
    force_rescan: bool,
) {
    let result = run_scan_bootstrap_worker_inner(&app, &scan_id, &req, &cache_key, force_rescan);
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

fn run_scan_bootstrap_worker_inner(
    app: &AppHandle,
    scan_id: &str,
    req: &StartScanRequest,
    cache_key: &str,
    force_rescan: bool,
) -> Result<(), String> {
    if !force_rescan {
        if let Some(snapshot) = load_cached_snapshot(app, cache_key)? {
            let cached_asset_count = snapshot.assets.len();
            {
                let state = app.state::<AppState>();
                let mut scans = state
                    .scans
                    .lock()
                    .map_err(|_| "Failed to lock scans state".to_string())?;
                if let Some(scan) = scans.get_mut(scan_id) {
                    scan.status = ScanLifecycle::Completed;
                    scan.is_refreshing = true;
                    scan.scanned_containers = snapshot.container_signatures.len();
                    scan.total_containers = snapshot.container_signatures.len();
                    scan.error = None;
                    scan.cancelled = false;
                    scan.assets = snapshot.assets;
                    scan.asset_index = scan
                        .assets
                        .iter()
                        .enumerate()
                        .map(|(index, asset)| (asset.asset_id.clone(), index))
                        .collect();
                    scan.search_records = if snapshot.search_records.len() == scan.assets.len() {
                        snapshot.search_records
                    } else {
                        scan.assets.iter().map(build_search_record).collect()
                    };
                    scan.tree_children = snapshot.tree_children;
                    scan.container_assets = snapshot.container_assets;
                    scan.container_signatures = snapshot.container_signatures;
                    scan.id_aliases = HashMap::new();
                    scan.cache_key = Some(cache_key.to_string());
                }
            }

            let _ = app.emit(
                "scan://completed",
                ScanCompletedEvent {
                    scan_id: scan_id.to_string(),
                    lifecycle: ScanLifecycle::Completed,
                    asset_count: cached_asset_count,
                    error: None,
                },
            );

            run_refresh_worker_inner(app, scan_id, req, cache_key)?;
            return Ok(());
        }
    }

    run_scan_worker(app.clone(), scan_id.to_string(), req.clone(), cache_key.to_string());
    Ok(())
}

fn run_scan_worker(app: AppHandle, scan_id: String, req: StartScanRequest, cache_key: String) {
    let result = run_scan_worker_inner(&app, &scan_id, &req, &cache_key);

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

fn run_scan_worker_inner(
    app: &AppHandle,
    scan_id: &str,
    req: &StartScanRequest,
    cache_key: &str,
) -> Result<(), String> {
    {
        let state = app.state::<AppState>();
        let lock_result = state.scans.lock();
        if let Ok(mut scans) = lock_result {
            if let Some(scan) = scans.get_mut(scan_id) {
                scan.cache_key = Some(cache_key.to_string());
                scan.is_refreshing = false;
            }
        }
    }

    let prism_root = expand_home(&req.prism_root);
    validate_prism_root(&prism_root)?;

    let instance_dir = resolve_instance_dir(&prism_root, &req.instance_folder)?;
    let mc_version = parse_minecraft_version(&instance_dir.join("mmc-pack.json"))
        .ok_or_else(|| "Failed to resolve Minecraft version from mmc-pack.json".to_string())?;

    let containers = collect_scan_containers(&prism_root, &instance_dir, &mc_version, req)?;
    let total_containers = containers.len();

    emit_scan_progress(
        app,
        ScanProgressEvent {
            scan_id: scan_id.to_string(),
            scanned_containers: 0,
            total_containers,
            asset_count: 0,
            phase: ScanPhase::Scanning,
            current_source: None,
        },
    );

    if total_containers == 0 {
        complete_scan_with_lifecycle(app, scan_id, ScanLifecycle::Completed, None)?;
        persist_scan_snapshot(app, scan_id, req, cache_key)?;
        return Ok(());
    }

    enum ScanWorkerResult {
        Container {
            container_key: String,
            source_name: String,
            signature: ContainerSignature,
            candidates: Vec<AssetCandidate>,
        },
        Error(String),
    }

    let workers = thread::available_parallelism()
        .map(|value| value.get().saturating_sub(2))
        .unwrap_or(1)
        .clamp(1, MAX_SCAN_WORKERS)
        .min(total_containers);

    let (sender, receiver) = mpsc::channel::<ScanWorkerResult>();
    let next_index = Arc::new(AtomicUsize::new(0));
    let containers = Arc::new(containers);
    let scan_id_owned = scan_id.to_string();

    for _ in 0..workers {
        let sender = sender.clone();
        let next_index = Arc::clone(&next_index);
        let containers = Arc::clone(&containers);
        let app = app.clone();
        let scan_id = scan_id_owned.clone();

        thread::spawn(move || loop {
            if is_scan_cancelled(&app, &scan_id).unwrap_or(true) {
                break;
            }

            let index = next_index.fetch_add(1, AtomicOrdering::Relaxed);
            if index >= containers.len() {
                break;
            }

            let container = &containers[index];
            let container_key = scan_container_key(container);
            let signature =
                match container_signature_for_path(&container.container_path, &container.container_type)
                {
                    Ok(value) => value,
                    Err(error) => {
                        let _ = sender.send(ScanWorkerResult::Error(error));
                        break;
                    }
                };
            match scan_container(container, &|| is_scan_cancelled(&app, &scan_id).unwrap_or(true)) {
                Ok(candidates) => {
                    if sender
                        .send(ScanWorkerResult::Container {
                            container_key,
                            source_name: container.source_name.clone(),
                            signature,
                            candidates,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Err(error) => {
                    if is_scan_cancelled(&app, &scan_id).unwrap_or(true) {
                        break;
                    }
                    let _ = sender.send(ScanWorkerResult::Error(error));
                    break;
                }
            }
        });
    }

    drop(sender);

    let mut key_counts = HashMap::<String, usize>::new();
    let mut scanned_containers = 0usize;

    while scanned_containers < total_containers {
        if is_scan_cancelled(app, scan_id)? {
            complete_scan_with_lifecycle(app, scan_id, ScanLifecycle::Cancelled, None)?;
            return Ok(());
        }

        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(ScanWorkerResult::Container {
                container_key,
                source_name,
                signature,
                candidates,
            }) => {
                scanned_containers += 1;
                let assets = finalize_assets(candidates, &mut key_counts);
                append_assets_chunk(
                    app,
                    scan_id,
                    &container_key,
                    &signature,
                    &assets,
                    scanned_containers,
                    total_containers,
                    ScanPhase::Scanning,
                    Some(source_name),
                )?;
            }
            Ok(ScanWorkerResult::Error(error)) => return Err(error),
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    if scanned_containers < total_containers && !is_scan_cancelled(app, scan_id)? {
        return Err("Scan workers disconnected before processing all containers".to_string());
    }

    complete_scan_with_lifecycle(app, scan_id, ScanLifecycle::Completed, None)?;
    persist_scan_snapshot(app, scan_id, req, cache_key)?;

    Ok(())
}

fn persist_scan_snapshot(
    app: &AppHandle,
    scan_id: &str,
    req: &StartScanRequest,
    cache_key: &str,
) -> Result<(), String> {
    let snapshot = {
        let state = app.state::<AppState>();
        let scans = state
            .scans
            .lock()
            .map_err(|_| "Failed to lock scans state".to_string())?;
        let scan = scans
            .get(scan_id)
            .ok_or_else(|| format!("Unknown scan id: {scan_id}"))?;

        ScanSnapshot {
            schema_version: SCAN_CACHE_SCHEMA_VERSION,
            cache_key: cache_key.to_string(),
            prism_root: req.prism_root.clone(),
            instance_folder: req.instance_folder.clone(),
            include_vanilla: req.include_vanilla,
            include_mods: req.include_mods,
            include_resourcepacks: req.include_resourcepacks,
            created_at: unix_timestamp_ms(),
            last_used_at: unix_timestamp_ms(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            assets: scan.assets.clone(),
            search_records: scan.search_records.clone(),
            tree_children: scan.tree_children.clone(),
            container_assets: scan.container_assets.clone(),
            container_signatures: scan.container_signatures.clone(),
        }
    };

    save_snapshot_to_cache(app, &snapshot)
}

fn build_scan_indexes(
    assets: &[AssetRecord],
) -> (
    HashMap<String, usize>,
    Vec<AssetSearchRecord>,
    HashMap<String, Vec<TreeNode>>,
) {
    let mut asset_index = HashMap::<String, usize>::new();
    let mut search_records = Vec::<AssetSearchRecord>::new();
    let mut tree_children = HashMap::<String, Vec<TreeNode>>::new();
    tree_children.insert(ROOT_NODE_ID.to_string(), Vec::new());

    for (index, asset) in assets.iter().enumerate() {
        asset_index.insert(asset.asset_id.clone(), index);
        search_records.push(build_search_record(asset));
        add_asset_to_tree(&mut tree_children, asset);
    }

    (asset_index, search_records, tree_children)
}

fn run_refresh_worker_inner(
    app: &AppHandle,
    scan_id: &str,
    req: &StartScanRequest,
    cache_key: &str,
) -> Result<(), String> {
    {
        let state = app.state::<AppState>();
        let mut scans = state
            .scans
            .lock()
            .map_err(|_| "Failed to lock scans state".to_string())?;
        let scan = scans
            .get_mut(scan_id)
            .ok_or_else(|| format!("Unknown scan id: {scan_id}"))?;
        if scan.cancelled {
            scan.is_refreshing = false;
            return Ok(());
        }
        scan.is_refreshing = true;
        scan.status = ScanLifecycle::Completed;
        scan.error = None;
        scan.cache_key = Some(cache_key.to_string());
    }

    let prism_root = expand_home(&req.prism_root);
    validate_prism_root(&prism_root)?;
    let instance_dir = resolve_instance_dir(&prism_root, &req.instance_folder)?;
    let mc_version = parse_minecraft_version(&instance_dir.join("mmc-pack.json"))
        .ok_or_else(|| "Failed to resolve Minecraft version from mmc-pack.json".to_string())?;
    let containers = collect_scan_containers(&prism_root, &instance_dir, &mc_version, req)?;

    let (cached_container_assets, cached_signatures, previous_assets) = {
        let state = app.state::<AppState>();
        let scans = state
            .scans
            .lock()
            .map_err(|_| "Failed to lock scans state".to_string())?;
        let scan = scans
            .get(scan_id)
            .ok_or_else(|| format!("Unknown scan id: {scan_id}"))?;
        (
            scan.container_assets.clone(),
            scan.container_signatures.clone(),
            scan.assets.clone(),
        )
    };

    let plan = build_scan_refresh_plan(&cached_signatures, &containers)?;
    let mut containers_by_key = HashMap::<String, ScanContainer>::new();
    for container in &containers {
        containers_by_key.insert(scan_container_key(container), container.clone());
    }

    let mut unchanged_keys = Vec::new();
    let mut changed_containers = plan.changed_or_new;
    for key in plan.unchanged_keys {
        if cached_container_assets.contains_key(&key) {
            unchanged_keys.push(key);
        } else if let Some(container) = containers_by_key.get(&key) {
            changed_containers.push(container.clone());
        }
    }
    changed_containers.sort_by(|left, right| scan_container_key(left).cmp(&scan_container_key(right)));

    let mut merged_container_assets = HashMap::<String, Vec<AssetRecord>>::new();
    let mut unchanged_assets = Vec::<AssetRecord>::new();
    for key in &unchanged_keys {
        if let Some(assets) = cached_container_assets.get(key) {
            unchanged_assets.extend(assets.clone());
            merged_container_assets.insert(key.clone(), assets.clone());
        }
    }

    let changed_total = changed_containers.len();
    let mut changed_scanned = 0usize;
    let mut changed_asset_count = 0usize;
    let mut key_counts = rebuild_key_counts_from_assets(&unchanged_assets);

    if changed_total > 0 {
        emit_scan_progress(
            app,
            ScanProgressEvent {
                scan_id: scan_id.to_string(),
                scanned_containers: 0,
                total_containers: changed_total,
                asset_count: unchanged_assets.len(),
                phase: ScanPhase::Refreshing,
                current_source: None,
            },
        );
    }

    enum RefreshWorkerResult {
        Container {
            container_key: String,
            source_name: String,
            candidates: Vec<AssetCandidate>,
        },
        Error(String),
    }

    if changed_total > 0 {
        let workers = thread::available_parallelism()
            .map(|value| value.get().saturating_sub(2))
            .unwrap_or(1)
            .clamp(1, MAX_SCAN_WORKERS)
            .min(changed_total);
        let (sender, receiver) = mpsc::channel::<RefreshWorkerResult>();
        let next_index = Arc::new(AtomicUsize::new(0));
        let changed_containers = Arc::new(changed_containers);
        let scan_id_owned = scan_id.to_string();

        for _ in 0..workers {
            let sender = sender.clone();
            let next_index = Arc::clone(&next_index);
            let changed_containers = Arc::clone(&changed_containers);
            let app = app.clone();
            let scan_id = scan_id_owned.clone();
            thread::spawn(move || loop {
                if is_scan_cancelled(&app, &scan_id).unwrap_or(true) {
                    break;
                }
                let index = next_index.fetch_add(1, AtomicOrdering::Relaxed);
                if index >= changed_containers.len() {
                    break;
                }
                let container = &changed_containers[index];
                let container_key = scan_container_key(container);
                match scan_container(container, &|| is_scan_cancelled(&app, &scan_id).unwrap_or(true)) {
                    Ok(candidates) => {
                        if sender
                            .send(RefreshWorkerResult::Container {
                                container_key,
                                source_name: container.source_name.clone(),
                                candidates,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(error) => {
                        if is_scan_cancelled(&app, &scan_id).unwrap_or(true) {
                            break;
                        }
                        let _ = sender.send(RefreshWorkerResult::Error(error));
                        break;
                    }
                }
            });
        }

        drop(sender);

        while changed_scanned < changed_total {
            if is_scan_cancelled(app, scan_id)? {
                let state = app.state::<AppState>();
                if let Ok(mut scans) = state.scans.lock() {
                    if let Some(scan) = scans.get_mut(scan_id) {
                        scan.is_refreshing = false;
                    }
                }
                return Ok(());
            }

            match receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(RefreshWorkerResult::Container {
                    container_key,
                    source_name,
                    candidates,
                }) => {
                    changed_scanned += 1;
                    let assets = finalize_assets(candidates, &mut key_counts);
                    changed_asset_count = changed_asset_count.saturating_add(assets.len());
                    merged_container_assets.insert(container_key, assets);
                    emit_scan_progress(
                        app,
                        ScanProgressEvent {
                            scan_id: scan_id.to_string(),
                            scanned_containers: changed_scanned,
                            total_containers: changed_total,
                            asset_count: unchanged_assets.len().saturating_add(changed_asset_count),
                            phase: ScanPhase::Refreshing,
                            current_source: Some(source_name),
                        },
                    );
                }
                Ok(RefreshWorkerResult::Error(error)) => return Err(error),
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    }

    let mut merged_signatures = HashMap::<String, ContainerSignature>::new();
    for (container_key, signature) in plan.signatures_by_key {
        if merged_container_assets.contains_key(&container_key) {
            merged_signatures.insert(container_key, signature);
        }
    }
    for removed in plan.removed_keys {
        merged_container_assets.remove(&removed);
        merged_signatures.remove(&removed);
    }

    let mut container_keys = merged_container_assets.keys().cloned().collect::<Vec<_>>();
    container_keys.sort();
    let mut next_assets = Vec::<AssetRecord>::new();
    for container_key in container_keys {
        if let Some(assets) = merged_container_assets.get(&container_key) {
            next_assets.extend(assets.clone());
        }
    }

    let (asset_index, search_records, tree_children) = build_scan_indexes(&next_assets);
    let id_aliases = build_asset_reconciliation_map(&previous_assets, &next_assets);
    let total_containers = merged_signatures.len();
    let asset_count = next_assets.len();

    {
        let state = app.state::<AppState>();
        let mut scans = state
            .scans
            .lock()
            .map_err(|_| "Failed to lock scans state".to_string())?;
        let scan = scans
            .get_mut(scan_id)
            .ok_or_else(|| format!("Unknown scan id: {scan_id}"))?;
        if scan.cancelled {
            scan.is_refreshing = false;
            return Ok(());
        }
        scan.status = ScanLifecycle::Completed;
        scan.is_refreshing = false;
        scan.error = None;
        scan.scanned_containers = total_containers;
        scan.total_containers = total_containers;
        scan.assets = next_assets;
        scan.asset_index = asset_index;
        scan.search_records = search_records;
        scan.tree_children = tree_children;
        scan.container_assets = merged_container_assets;
        scan.container_signatures = merged_signatures;
        scan.id_aliases = id_aliases;
        scan.cache_key = Some(cache_key.to_string());
    }

    let _ = app.emit(
        "scan://completed",
        ScanCompletedEvent {
            scan_id: scan_id.to_string(),
            lifecycle: ScanLifecycle::Completed,
            asset_count,
            error: None,
        },
    );
    persist_scan_snapshot(app, scan_id, req, cache_key)?;
    Ok(())
}

fn rebuild_key_counts_from_assets(assets: &[AssetRecord]) -> HashMap<String, usize> {
    let mut counts = HashMap::<String, usize>::new();

    for asset in assets {
        let (base_key, suffix) = parse_dup_suffix(&asset.key);
        let required = suffix.map(|index| index + 1).unwrap_or(1);
        let current = counts.entry(base_key).or_insert(0);
        *current = (*current).max(required);
    }

    counts
}

fn parse_dup_suffix(key: &str) -> (String, Option<usize>) {
    if let Some((base, suffix)) = key.rsplit_once(".dup") {
        if !base.is_empty() && !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit()) {
            if let Ok(index) = suffix.parse::<usize>() {
                return (base.to_string(), Some(index));
            }
        }
    }

    (key.to_string(), None)
}

fn append_assets_chunk(
    app: &AppHandle,
    scan_id: &str,
    container_key: &str,
    signature: &ContainerSignature,
    chunk: &[AssetRecord],
    scanned_containers: usize,
    total_containers: usize,
    phase: ScanPhase,
    current_source: Option<String>,
) -> Result<(), String> {
    const PROGRESS_THROTTLE: Duration = Duration::from_millis(125);

    let asset_count;
    let mut should_emit_progress = false;

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
        scan.container_signatures
            .insert(container_key.to_string(), signature.clone());

        let mut appended_for_container = Vec::<AssetRecord>::new();

        for asset in chunk {
            if scan.asset_index.contains_key(&asset.asset_id) {
                continue;
            }

            let index = scan.assets.len();
            scan.asset_index.insert(asset.asset_id.clone(), index);
            scan.search_records.push(build_search_record(asset));
            scan.assets.push(asset.clone());
            appended_for_container.push(asset.clone());
            add_asset_to_tree(&mut scan.tree_children, asset);
        }
        scan.container_assets
            .insert(container_key.to_string(), appended_for_container);

        let now = Instant::now();
        let force_emit = scanned_containers >= total_containers;
        let elapsed = scan
            .last_progress_emit_at
            .map(|last| now.saturating_duration_since(last))
            .unwrap_or(PROGRESS_THROTTLE);

        if force_emit || elapsed >= PROGRESS_THROTTLE {
            should_emit_progress = true;
            scan.last_progress_emit_at = Some(now);
        }

        asset_count = scan.assets.len();
    }

    if should_emit_progress {
        emit_scan_progress(
            app,
            ScanProgressEvent {
                scan_id: scan_id.to_string(),
                scanned_containers,
                total_containers,
                asset_count,
                phase,
                current_source,
            },
        );
    }

    Ok(())
}

fn update_scan_error(app: &AppHandle, scan_id: &str, error: &str) {
    let state = app.state::<AppState>();
    let lock_result = state.scans.lock();
    if let Ok(mut scans) = lock_result {
        if let Some(scan) = scans.get_mut(scan_id) {
            scan.status = ScanLifecycle::Error;
            scan.is_refreshing = false;
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
        scan.is_refreshing = false;
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

        if let Some(asset_index_path) = resolve_vanilla_asset_index_path(prism_root, mc_version) {
            containers.push(ScanContainer {
                source_type: AssetSourceType::Vanilla,
                source_name: format!("minecraft-{mc_version}"),
                container_type: AssetContainerType::AssetIndex,
                container_path: asset_index_path,
            });
        }
    }

    containers.sort_by(|left, right| scan_container_key(left).cmp(&scan_container_key(right)));
    Ok(containers)
}

fn scan_container(
    container: &ScanContainer,
    should_cancel: &dyn Fn() -> bool,
) -> Result<Vec<AssetCandidate>, String> {
    match container.container_type {
        AssetContainerType::Directory => scan_directory_container(container, should_cancel),
        AssetContainerType::Zip | AssetContainerType::Jar => {
            scan_archive_container(container, should_cancel)
        }
        AssetContainerType::AssetIndex => scan_vanilla_asset_index_container(container, should_cancel),
    }
}

fn scan_vanilla_asset_index_container(
    container: &ScanContainer,
    should_cancel: &dyn Fn() -> bool,
) -> Result<Vec<AssetCandidate>, String> {
    let content = fs::read_to_string(&container.container_path).map_err(|error| {
        format!(
            "Failed to read vanilla asset index {}: {error}",
            container.container_path.display()
        )
    })?;

    let parsed: MinecraftAssetIndexFile = serde_json::from_str(&content).map_err(|error| {
        format!(
            "Failed to parse vanilla asset index {}: {error}",
            container.container_path.display()
        )
    })?;

    let assets_root = container
        .container_path
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| {
            format!(
                "Invalid asset index path (cannot resolve assets root): {}",
                container.container_path.display()
            )
        })?;
    let objects_root = assets_root.join("objects");

    let mut assets = Vec::new();
    let mut processed = 0usize;
    for (logical_path, object) in parsed.objects {
        processed = processed.saturating_add(1);
        if processed % SCAN_CANCEL_CHECK_INTERVAL == 0 && should_cancel() {
            return Err("Scan cancelled".to_string());
        }

        let Some((namespace, relative_asset_path)) = logical_path.split_once('/') else {
            continue;
        };

        // Vanilla sounds are shipped via asset indexes/objects, not client jar entries.
        if !relative_asset_path.starts_with("sounds/") {
            continue;
        }

        let extension = relative_asset_path
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();

        if !is_audio_extension(&extension) {
            continue;
        }

        if object.hash.len() < 2 {
            continue;
        }

        let entry_path = format!("{}/{}", &object.hash[0..2], object.hash);
        let absolute_path = objects_root.join(&entry_path);
        if !absolute_path.is_file() {
            continue;
        }

        assets.push(AssetCandidate {
            source_type: container.source_type.clone(),
            source_name: container.source_name.clone(),
            namespace: namespace.to_string(),
            relative_asset_path: relative_asset_path.to_string(),
            container_path: objects_root.clone(),
            container_type: AssetContainerType::Directory,
            entry_path,
            extension,
            is_image: false,
            is_audio: true,
        });
    }

    Ok(assets)
}

fn scan_directory_container(
    container: &ScanContainer,
    should_cancel: &dyn Fn() -> bool,
) -> Result<Vec<AssetCandidate>, String> {
    let mut assets = Vec::new();
    let mut processed = 0usize;

    for entry in WalkDir::new(&container.container_path)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
    {
        processed = processed.saturating_add(1);
        if processed % SCAN_CANCEL_CHECK_INTERVAL == 0 && should_cancel() {
            return Err("Scan cancelled".to_string());
        }

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

fn scan_archive_container(
    container: &ScanContainer,
    should_cancel: &dyn Fn() -> bool,
) -> Result<Vec<AssetCandidate>, String> {
    let file = fs::File::open(&container.container_path).map_err(|error| {
        format!(
            "Failed to open archive {}: {error}",
            container.container_path.display()
        )
    })?;

    let mut archive = ZipArchive::new(file).map_err(|error| {
        format!(
            "Failed to read archive {}: {error}",
            container.container_path.display()
        )
    })?;

    let mut assets = Vec::new();

    for index in 0..archive.len() {
        if index % SCAN_CANCEL_CHECK_INTERVAL == 0 && should_cancel() {
            return Err("Scan cancelled".to_string());
        }

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
    let segments: Vec<&str> = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect();
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
    let filename = Path::new(&asset.relative_asset_path)
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| asset.relative_asset_path.clone());

    let filename_stem = Path::new(&filename)
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.clone());

    let filename_tokens = split_tokens(&filename);
    let path_tokens = split_tokens(&asset.relative_asset_path);
    let namespace_tokens = split_tokens(&asset.namespace);
    let source_tokens = split_tokens(&asset.source_name);

    let mut token_set = HashSet::new();
    for token in split_tokens(&asset.key) {
        token_set.insert(token);
    }
    for token in &path_tokens {
        token_set.insert(token.clone());
    }
    for token in &namespace_tokens {
        token_set.insert(token.clone());
    }
    for token in &source_tokens {
        token_set.insert(token.clone());
    }

    let mut all_tokens = token_set.into_iter().collect::<Vec<_>>();
    all_tokens.sort();

    AssetSearchRecord {
        all_tokens,
        filename_tokens,
        path_tokens,
        namespace_tokens,
        source_tokens,
        compact_all: compact_text(&format!(
            "{} {} {} {}",
            asset.key, asset.source_name, asset.namespace, asset.relative_asset_path
        )),
        compact_filename: compact_text(&filename),
        compact_filename_stem: compact_text(&filename_stem),
        key: asset.key.to_lowercase(),
        folder_node_id: asset_folder_node_id(asset),
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
    normalized_query: &str,
) -> Option<i64> {
    if query_tokens.is_empty() {
        return Some(0);
    }

    let mut score = 0i64;
    let mut matched_tokens = 0usize;

    for query_token in query_tokens {
        let mut token_score = 0i64;

        token_score = token_score.max(score_token_group_fast(
            &index.filename_tokens,
            query_token,
            320,
            250,
            180,
        ));
        token_score = token_score.max(score_token_group_fast(
            &index.path_tokens,
            query_token,
            170,
            130,
            95,
        ));
        token_score = token_score.max(score_token_group_fast(
            &index.namespace_tokens,
            query_token,
            140,
            110,
            80,
        ));
        token_score = token_score.max(score_token_group_fast(
            &index.source_tokens,
            query_token,
            130,
            100,
            76,
        ));
        token_score = token_score.max(score_token_group_fast(
            &index.all_tokens,
            query_token,
            100,
            80,
            60,
        ));

        if token_score == 0 {
            token_score = token_score.max(score_fuzzy_token_group(
                &index.filename_tokens,
                query_token,
                72,
            ));
            token_score =
                token_score.max(score_fuzzy_token_group(&index.path_tokens, query_token, 48));
        }

        if token_score == 0 {
            score -= 100;
            continue;
        }

        matched_tokens += 1;
        score += token_score;
    }

    let token_count = query_tokens.len();
    let required_matches = if token_count <= 2 {
        token_count
    } else {
        (token_count * 3).div_ceil(5)
    };

    if matched_tokens < required_matches {
        return None;
    }

    let missing_tokens = token_count.saturating_sub(matched_tokens);
    if missing_tokens > 0 {
        score -= (missing_tokens as i64) * 70;
    } else {
        score += 90;
    }

    score += (matched_tokens as i64) * 48;

    if !query_compact.is_empty() {
        if index.compact_filename_stem == query_compact {
            score += 450;
        } else if index.compact_filename_stem.starts_with(query_compact) {
            score += 240;
        } else if index.compact_filename.contains(query_compact) {
            score += 190;
        }

        if index.compact_all.contains(query_compact) {
            score += 120;
        }
    }

    if !normalized_query.is_empty() && index.key.contains(normalized_query) {
        score += 80;
    }

    let extra_filename_tokens = index.filename_tokens.len().saturating_sub(matched_tokens);
    if extra_filename_tokens > 0 {
        score -= (extra_filename_tokens as i64) * 8;
    }

    Some(score)
}

fn score_token_group_fast(
    tokens: &[String],
    query_token: &str,
    exact_weight: i64,
    prefix_weight: i64,
    contains_weight: i64,
) -> i64 {
    let mut best = 0;
    for token in tokens {
        if token == query_token {
            return exact_weight;
        } else if token.starts_with(query_token) || query_token.starts_with(token) {
            best = best.max(prefix_weight);
        } else if token.contains(query_token) || query_token.contains(token) {
            best = best.max(contains_weight);
        }
    }
    best
}

fn score_fuzzy_token_group(tokens: &[String], query_token: &str, max_weight: i64) -> i64 {
    if query_token.len() < 4 {
        return 0;
    }

    let mut best = 0i64;
    for token in tokens {
        let score = score_fuzzy_token(token, query_token);
        if score > 0 {
            best = best.max(max_weight.min(score));
        }
    }

    best
}

fn score_fuzzy_token(token: &str, query_token: &str) -> i64 {
    let token_len = token.len();
    let query_len = query_token.len();
    if token_len < 3 || query_len < 3 {
        return 0;
    }

    let len_delta = token_len.abs_diff(query_len);
    if len_delta > 2 {
        return 0;
    }

    let token_bytes = token.as_bytes();
    let query_bytes = query_token.as_bytes();
    let same_start = token_bytes.first() == query_bytes.first();
    let swap_start = token_bytes.len() > 1
        && query_bytes.len() > 1
        && token_bytes[0] == query_bytes[1]
        && token_bytes[1] == query_bytes[0];
    if !same_start && !swap_start {
        return 0;
    }

    let distance = damerau_levenshtein(token, query_token);
    match distance {
        1 => 72,
        2 if token_len >= 4 && query_len >= 4 => 54,
        3 if token_len >= 9 && query_len >= 9 => 40,
        _ => 0,
    }
}

fn idle_asset_cmp(left: &AssetRecord, right: &AssetRecord) -> CmpOrdering {
    let left_name = idle_filename(left);
    let right_name = idle_filename(right);
    let left_token = last_filename_token(&left_name);
    let right_token = last_filename_token(&right_name);

    natural_compare(&left_token, &right_token)
        .then_with(|| {
            natural_compare(
                &left_name.to_ascii_lowercase(),
                &right_name.to_ascii_lowercase(),
            )
        })
        .then_with(|| left.key.cmp(&right.key))
}

fn idle_filename(asset: &AssetRecord) -> String {
    Path::new(&asset.relative_asset_path)
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| asset.relative_asset_path.clone())
}

fn last_filename_token(file_name: &str) -> String {
    let stem = Path::new(file_name)
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| file_name.to_string());
    split_tokens(&stem)
        .pop()
        .unwrap_or_else(|| stem.to_ascii_lowercase())
}

fn natural_compare(left: &str, right: &str) -> CmpOrdering {
    let left_chunks = natural_chunks(left);
    let right_chunks = natural_chunks(right);
    let max_len = left_chunks.len().max(right_chunks.len());

    for index in 0..max_len {
        let left_chunk = left_chunks.get(index);
        let right_chunk = right_chunks.get(index);
        let ordering = match (left_chunk, right_chunk) {
            (Some(NaturalChunk::Number(a)), Some(NaturalChunk::Number(b))) => a.cmp(b),
            (Some(NaturalChunk::Text(a)), Some(NaturalChunk::Text(b))) => a.cmp(b),
            (Some(NaturalChunk::Number(_)), Some(NaturalChunk::Text(_))) => CmpOrdering::Less,
            (Some(NaturalChunk::Text(_)), Some(NaturalChunk::Number(_))) => CmpOrdering::Greater,
            (Some(_), None) => CmpOrdering::Greater,
            (None, Some(_)) => CmpOrdering::Less,
            (None, None) => CmpOrdering::Equal,
        };

        if ordering != CmpOrdering::Equal {
            return ordering;
        }
    }

    CmpOrdering::Equal
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NaturalChunk {
    Number(u64),
    Text(String),
}

fn natural_chunks(value: &str) -> Vec<NaturalChunk> {
    let mut chunks = Vec::<NaturalChunk>::new();
    let mut current = String::new();
    let mut is_number = false;

    for ch in value.chars() {
        if ch.is_ascii_digit() {
            if !is_number && !current.is_empty() {
                chunks.push(NaturalChunk::Text(current.to_ascii_lowercase()));
                current.clear();
            }
            is_number = true;
            current.push(ch);
        } else {
            if is_number && !current.is_empty() {
                chunks.push(NaturalChunk::Number(
                    current.parse::<u64>().unwrap_or(u64::MAX),
                ));
                current.clear();
            }
            is_number = false;
            current.push(ch);
        }
    }

    if !current.is_empty() {
        if is_number {
            chunks.push(NaturalChunk::Number(
                current.parse::<u64>().unwrap_or(u64::MAX),
            ));
        } else {
            chunks.push(NaturalChunk::Text(current.to_ascii_lowercase()));
        }
    }

    chunks
}

fn asset_matches_folder(index: &AssetSearchRecord, folder_filter: Option<&str>) -> bool {
    let Some(folder_filter) = folder_filter else {
        return true;
    };

    index.folder_node_id == folder_filter
        || index
            .folder_node_id
            .starts_with(&format!("{folder_filter}/"))
}

fn asset_matches_media(
    asset: &AssetRecord,
    include_images: bool,
    include_audio: bool,
    include_other: bool,
) -> bool {
    if asset.is_image {
        return include_images;
    }

    if asset.is_audio {
        return include_audio;
    }

    include_other
}

fn add_asset_to_tree(tree_children: &mut HashMap<String, Vec<TreeNode>>, asset: &AssetRecord) {
    let mut parent_id = ROOT_NODE_ID.to_string();
    let folders = build_asset_folder_segments(asset);

    for segment in folders {
        let node_name = if segment.is_empty() {
            "(root)"
        } else {
            &segment
        };
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

fn asset_folder_node_id(asset: &AssetRecord) -> String {
    let mut node_id = ROOT_NODE_ID.to_string();
    for segment in build_asset_folder_segments(asset) {
        let node_name = if segment.is_empty() {
            "(root)"
        } else {
            &segment
        };
        node_id = build_folder_node_id(&node_id, node_name);
    }
    node_id
}

fn build_asset_folder_segments(asset: &AssetRecord) -> Vec<String> {
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

    folders
}

fn build_folder_node_id(parent: &str, segment: &str) -> String {
    let escaped = segment.replace('/', "∕");
    if parent == ROOT_NODE_ID {
        format!("{ROOT_NODE_ID}/{escaped}")
    } else {
        format!("{parent}/{escaped}")
    }
}

fn upsert_tree_node(
    tree_children: &mut HashMap<String, Vec<TreeNode>>,
    parent_id: &str,
    node: TreeNode,
) {
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
    let asset_ids = vec![asset_id.to_string()];

    collect_assets(state, scan_id, &asset_ids)
        .map(|mut assets| assets.remove(0))
        .map_err(|error| error.to_string())
}

fn resolve_operation_id(operation_id: Option<String>) -> String {
    operation_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

fn register_export_operation(
    state: &State<'_, AppState>,
    operation_id: &str,
) -> Result<(), String> {
    let mut operations = state
        .export_operations
        .lock()
        .map_err(|_| "Failed to lock export operations state".to_string())?;
    operations.insert(operation_id.to_string(), ExportOperationState::new());
    Ok(())
}

fn unregister_export_operation(state: &State<'_, AppState>, operation_id: &str) {
    if let Ok(mut operations) = state.export_operations.lock() {
        operations.remove(operation_id);
    }
}

fn is_export_cancelled(app: &AppHandle, operation_id: &str) -> bool {
    let state = app.state::<AppState>();
    let Ok(operations) = state.export_operations.lock() else {
        return true;
    };

    operations
        .get(operation_id)
        .map(|operation| operation.cancelled)
        .unwrap_or(false)
}

fn emit_scan_progress(app: &AppHandle, event: ScanProgressEvent) {
    let state = app.state::<AppState>();
    if let Ok(mut scans) = state.scans.lock() {
        if let Some(scan) = scans.get_mut(&event.scan_id) {
            scan.scanned_containers = event.scanned_containers;
            scan.total_containers = event.total_containers;
            if matches!(scan.status, ScanLifecycle::Scanning) {
                scan.error = None;
            }
        }
    }

    let _ = app.emit("scan://progress", event);
}

fn emit_export_progress(app: &AppHandle, event: ExportProgressEvent) {
    let _ = app.emit("export://progress", event);
}

fn emit_export_completed(app: &AppHandle, event: ExportCompletedEvent) {
    let _ = app.emit("export://completed", event);
}

#[derive(Debug, Clone)]
struct ExportJob {
    index: usize,
    asset: AssetRecord,
    output_path: PathBuf,
}

#[derive(Debug)]
struct ExportRunOutcome {
    output_files: Vec<String>,
    processed_count: usize,
    success_count: usize,
    failed_count: usize,
    cancelled: bool,
    failures: Vec<ExportFailure>,
}

#[derive(Debug)]
enum ExportWorkerResult {
    Success {
        index: usize,
        output_path: PathBuf,
    },
    Failure {
        index: usize,
        failure: ExportFailure,
    },
}

fn plan_export_jobs(
    assets: Vec<AssetRecord>,
    destination_dir: &Path,
    audio_format: AudioFormat,
) -> Vec<ExportJob> {
    let mut used_names = HashSet::new();
    let mut jobs = Vec::new();

    for (index, asset) in assets.into_iter().enumerate() {
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

        let target_name =
            dedupe_file_name(&base_stem, &extension, destination_dir, &mut used_names);
        jobs.push(ExportJob {
            index,
            asset,
            output_path: destination_dir.join(target_name),
        });
    }

    jobs
}

fn run_export_operation(
    app: &AppHandle,
    kind: ExportOperationKind,
    operation_id: &str,
    assets: Vec<AssetRecord>,
    destination_dir: &Path,
    audio_format: AudioFormat,
) -> Result<ExportRunOutcome, String> {
    let jobs = plan_export_jobs(assets, destination_dir, audio_format.clone());
    let requested_count = jobs.len();

    if requested_count == 0 {
        emit_export_progress(
            app,
            ExportProgressEvent {
                operation_id: operation_id.to_string(),
                kind: kind.clone(),
                requested_count,
                processed_count: 0,
                success_count: 0,
                failed_count: 0,
                cancelled: false,
            },
        );
        emit_export_completed(
            app,
            ExportCompletedEvent {
                operation_id: operation_id.to_string(),
                kind,
                requested_count,
                processed_count: 0,
                success_count: 0,
                failed_count: 0,
                cancelled: false,
                failures: Vec::new(),
            },
        );
        return Ok(ExportRunOutcome {
            output_files: Vec::new(),
            processed_count: 0,
            success_count: 0,
            failed_count: 0,
            cancelled: false,
            failures: Vec::new(),
        });
    }

    let should_convert_audio =
        audio_format != AudioFormat::Original && jobs.iter().any(|job| job.asset.is_audio);
    let ffmpeg_path = if should_convert_audio {
        Some(resolve_ffmpeg_path(app)?)
    } else {
        None
    };

    emit_export_progress(
        app,
        ExportProgressEvent {
            operation_id: operation_id.to_string(),
            kind: kind.clone(),
            requested_count,
            processed_count: 0,
            success_count: 0,
            failed_count: 0,
            cancelled: false,
        },
    );

    let workers = thread::available_parallelism()
        .map(|value| value.get())
        .unwrap_or(1)
        .clamp(1, MAX_EXPORT_WORKERS)
        .min(requested_count);

    let (sender, receiver) = mpsc::channel::<ExportWorkerResult>();
    let jobs = Arc::new(jobs);
    let next_index = Arc::new(AtomicUsize::new(0));
    let operation_id_owned = operation_id.to_string();

    for _ in 0..workers {
        let sender = sender.clone();
        let jobs = Arc::clone(&jobs);
        let next_index = Arc::clone(&next_index);
        let app = app.clone();
        let operation_id = operation_id_owned.clone();
        let ffmpeg_path = ffmpeg_path.clone();
        let audio_format = audio_format.clone();

        thread::spawn(move || {
            let mut archive_cache = HashMap::<String, ZipArchive<fs::File>>::new();

            loop {
                if is_export_cancelled(&app, &operation_id) {
                    break;
                }

                let index = next_index.fetch_add(1, AtomicOrdering::Relaxed);
                if index >= jobs.len() {
                    break;
                }

                let job = &jobs[index];
                let result = materialize_export_job(
                    job,
                    &audio_format,
                    ffmpeg_path.as_deref(),
                    &mut archive_cache,
                );

                let worker_message = match result {
                    Ok(path) => ExportWorkerResult::Success {
                        index: job.index,
                        output_path: path,
                    },
                    Err(error) => ExportWorkerResult::Failure {
                        index: job.index,
                        failure: ExportFailure {
                            asset_id: job.asset.asset_id.clone(),
                            key: job.asset.key.clone(),
                            error,
                        },
                    },
                };

                if sender.send(worker_message).is_err() {
                    break;
                }
            }
        });
    }

    drop(sender);

    let mut processed_count = 0usize;
    let mut success_count = 0usize;
    let mut failed_count = 0usize;
    let mut failures = Vec::<ExportFailure>::new();
    let mut output_files = vec![None; requested_count];

    while processed_count < requested_count {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(ExportWorkerResult::Success { index, output_path }) => {
                processed_count += 1;
                success_count += 1;
                output_files[index] = Some(output_path.to_string_lossy().to_string());
            }
            Ok(ExportWorkerResult::Failure { index, failure }) => {
                processed_count += 1;
                failed_count += 1;
                output_files[index] = None;
                failures.push(failure);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if is_export_cancelled(app, operation_id) {
                    continue;
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        emit_export_progress(
            app,
            ExportProgressEvent {
                operation_id: operation_id.to_string(),
                kind: kind.clone(),
                requested_count,
                processed_count,
                success_count,
                failed_count,
                cancelled: is_export_cancelled(app, operation_id),
            },
        );
    }

    while let Ok(result) = receiver.try_recv() {
        match result {
            ExportWorkerResult::Success { index, output_path } => {
                processed_count += 1;
                success_count += 1;
                output_files[index] = Some(output_path.to_string_lossy().to_string());
            }
            ExportWorkerResult::Failure { index, failure } => {
                processed_count += 1;
                failed_count += 1;
                output_files[index] = None;
                failures.push(failure);
            }
        }
    }

    let cancelled = is_export_cancelled(app, operation_id);
    if processed_count < requested_count && !cancelled {
        return Err("Export workers disconnected before processing all assets".to_string());
    }

    let output_files = output_files.into_iter().flatten().collect::<Vec<_>>();
    emit_export_completed(
        app,
        ExportCompletedEvent {
            operation_id: operation_id.to_string(),
            kind: kind.clone(),
            requested_count,
            processed_count,
            success_count,
            failed_count,
            cancelled,
            failures: failures.clone(),
        },
    );

    Ok(ExportRunOutcome {
        output_files,
        processed_count,
        success_count,
        failed_count,
        cancelled,
        failures,
    })
}

fn materialize_export_job(
    job: &ExportJob,
    audio_format: &AudioFormat,
    ffmpeg_path: Option<&Path>,
    archive_cache: &mut HashMap<String, ZipArchive<fs::File>>,
) -> Result<PathBuf, String> {
    let bytes = extract_asset_bytes_with_archive_cache(&job.asset, archive_cache)?;

    if job.asset.is_audio && *audio_format != AudioFormat::Original {
        let ffmpeg_path = ffmpeg_path.ok_or_else(|| "FFmpeg path was not resolved".to_string())?;
        convert_audio_bytes_to_file(ffmpeg_path, &bytes, &job.output_path, audio_format)?;
    } else {
        fs::write(&job.output_path, bytes).map_err(|error| {
            format!(
                "Failed to write output file {}: {error}",
                job.output_path.display()
            )
        })?;
    }

    Ok(job.output_path.clone())
}

fn convert_audio_bytes_to_file(
    ffmpeg_path: &Path,
    input_bytes: &[u8],
    output_path: &Path,
    format: &AudioFormat,
) -> Result<(), String> {
    let mut command = Command::new(ffmpeg_path);
    command.arg("-y");
    command.arg("-hide_banner");
    command.arg("-loglevel");
    command.arg("error");
    command.arg("-i");
    command.arg("pipe:0");
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
    command.stdin(Stdio::piped());
    command.stdout(Stdio::null());
    command.stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|error| format!("Failed to start ffmpeg: {error}"))?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| "Failed to open ffmpeg stdin".to_string())?;
        stdin
            .write_all(input_bytes)
            .map_err(|error| format!("Failed to stream audio data to ffmpeg: {error}"))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|error| format!("Failed to wait for ffmpeg: {error}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("FFmpeg conversion failed: {}", stderr.trim()));
    }

    Ok(())
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

    let url =
        ffmpeg_download_url().map_err(|error| format!("Failed to resolve FFmpeg URL: {error}"))?;
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
    let mut archive_cache = HashMap::<String, ZipArchive<fs::File>>::new();
    extract_asset_bytes_with_archive_cache(asset, &mut archive_cache)
}

fn extract_asset_bytes_with_archive_cache(
    asset: &AssetRecord,
    archive_cache: &mut HashMap<String, ZipArchive<fs::File>>,
) -> Result<Vec<u8>, String> {
    let container_path = PathBuf::from(&asset.container_path);

    match asset.container_type {
        AssetContainerType::Directory => {
            let file_path = container_path.join(Path::new(&asset.entry_path));
            fs::read(&file_path)
                .map_err(|error| format!("Failed to read file {}: {error}", file_path.display()))
        }
        AssetContainerType::AssetIndex => Err(
            "AssetIndex container type is metadata-only and cannot be extracted directly"
                .to_string(),
        ),
        AssetContainerType::Zip | AssetContainerType::Jar => {
            if !archive_cache.contains_key(&asset.container_path) {
                let file = fs::File::open(&container_path).map_err(|error| {
                    format!(
                        "Failed to open archive {}: {error}",
                        container_path.display()
                    )
                })?;
                let archive = ZipArchive::new(file).map_err(|error| {
                    format!(
                        "Failed to read archive {}: {error}",
                        container_path.display()
                    )
                })?;
                archive_cache.insert(asset.container_path.clone(), archive);
            }

            let archive = archive_cache
                .get_mut(&asset.container_path)
                .ok_or_else(|| "Failed to get cached archive".to_string())?;

            let mut entry = archive.by_name(&asset.entry_path).map_err(|error| {
                format!("Failed to open archive entry {}: {error}", asset.entry_path)
            })?;

            let mut buffer = Vec::new();
            entry.read_to_end(&mut buffer).map_err(|error| {
                format!("Failed to read archive entry {}: {error}", asset.entry_path)
            })?;

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
        "ogg" | "oga" => "audio/ogg",
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "opus" => "audio/opus",
        "m4a" => "audio/mp4",
        "aac" => "audio/aac",
        "json" | "mcmeta" => "application/json",
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

fn is_json_extension(extension: &str) -> bool {
    matches!(extension, "json" | "mcmeta")
}

fn dedupe_candidates(
    candidates: Vec<PrismRootCandidate>,
) -> Result<Vec<PrismRootCandidate>, String> {
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

fn resolve_vanilla_asset_index_path(prism_root: &Path, mc_version: &str) -> Option<PathBuf> {
    let meta_path = prism_root
        .join("meta")
        .join("net.minecraft")
        .join(format!("{mc_version}.json"));
    let content = fs::read_to_string(meta_path).ok()?;
    let parsed: MinecraftMetaVersion = serde_json::from_str(&content).ok()?;

    let index_id = parsed
        .asset_index
        .map(|asset_index| asset_index.id)
        .or(parsed.assets)?;

    let index_path = prism_root
        .join("assets")
        .join("indexes")
        .join(format!("{index_id}.json"));

    if index_path.is_file() {
        Some(index_path)
    } else {
        None
    }
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
        .setup(|app| {
            app.remove_menu()?;

            #[cfg(target_os = "macos")]
            {
                let app_menu = SubmenuBuilder::new(app, "Minecraft Asset Explorer")
                    .about(None)
                    .separator()
                    .quit()
                    .build()?;
                let menu = MenuBuilder::new(app).item(&app_menu).build()?;
                app.set_menu(menu)?;
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            detect_prism_roots,
            list_instances,
            start_scan,
            get_scan_status,
            cancel_scan,
            cancel_export,
            list_tree_children,
            search_assets,
            get_asset_preview,
            get_asset_record,
            reconcile_asset_ids,
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
        let asset = sample_asset(
            "mod.allthemodium.allthemodium.textures.item.atm_star.png",
            AssetSourceType::Mod,
            "allthemodium",
            "allthemodium",
            "textures/item/atm_star.png",
        );
        let record = build_search_record(&asset);

        let tokens = split_tokens("atm star");
        let score = score_query(
            &record,
            &tokens,
            &compact_text("atm star"),
            &tokens.join(" "),
        );

        assert!(score.is_some());
    }

    #[test]
    fn parse_assets_path_from_nested_prefix() {
        let parsed =
            parse_asset_relative_path("nested/content/assets/example/textures/item/star.png")
                .expect("must parse");

        assert_eq!(parsed.namespace, "example");
        assert_eq!(parsed.relative_asset_path, "textures/item/star.png");
    }

    #[test]
    fn exact_filename_scores_higher_than_long_variant() {
        let vanilla = sample_asset(
            "vanilla.minecraft.minecraft.textures.item.nether_star.png",
            AssetSourceType::Vanilla,
            "minecraft-1.21.1",
            "minecraft",
            "textures/item/nether_star.png",
        );
        let modded = sample_asset(
            "mod.atc.atc.blockstates.nether_star_block_2x.json",
            AssetSourceType::Mod,
            "allthecompressed",
            "allthecompressed",
            "blockstates/nether_star_block_2x.json",
        );

        let query = "nether star";
        let tokens = split_tokens(query);
        let compact = compact_text(query);
        let normalized = tokens.join(" ");

        let vanilla_score = score_query(
            &build_search_record(&vanilla),
            &tokens,
            &compact,
            &normalized,
        )
        .expect("vanilla must match");
        let modded_score = score_query(
            &build_search_record(&modded),
            &tokens,
            &compact,
            &normalized,
        )
        .expect("modded must match");

        assert!(vanilla_score > modded_score);
    }

    #[test]
    fn query_with_extra_token_still_matches_best_pair() {
        let expected = sample_asset(
            "vanilla.minecraft.minecraft.sounds.block.grass.step1.ogg",
            AssetSourceType::Vanilla,
            "minecraft-1.21.1",
            "minecraft",
            "sounds/block/grass/step1.ogg",
        );

        let unrelated = sample_asset(
            "mod.example.example.sounds.block.stone.step1.ogg",
            AssetSourceType::Mod,
            "example-mod",
            "example",
            "sounds/block/stone/step1.ogg",
        );

        let query = "grass block step";
        let tokens = split_tokens(query);
        let compact = compact_text(query);
        let normalized = tokens.join(" ");

        let expected_score = score_query(
            &build_search_record(&expected),
            &tokens,
            &compact,
            &normalized,
        )
        .expect("expected must match");
        let unrelated_score = score_query(
            &build_search_record(&unrelated),
            &tokens,
            &compact,
            &normalized,
        )
        .expect("unrelated should still match with weaker score");

        assert!(expected_score > unrelated_score);
    }

    #[test]
    fn damerau_fuzzy_match_accepts_transposed_token() {
        let asset = sample_asset(
            "vanilla.minecraft.minecraft.sounds.block.grass.step1.ogg",
            AssetSourceType::Vanilla,
            "minecraft-1.21.1",
            "minecraft",
            "sounds/block/grass/step1.ogg",
        );
        let record = build_search_record(&asset);
        let query = "stpe";
        let tokens = split_tokens(query);

        let score = score_query(&record, &tokens, &compact_text(query), &tokens.join(" "));
        assert!(score.is_some());
    }

    #[test]
    fn folder_filter_matches_subtree() {
        let asset = sample_asset(
            "mod.sample.sample.textures.item.star.png",
            AssetSourceType::Mod,
            "sample-mod",
            "sample",
            "textures/item/star.png",
        );

        let index = build_search_record(&asset);
        let folder = asset_folder_node_id(&asset);
        let parent = folder.split('/').take(4).collect::<Vec<_>>().join("/");

        assert!(asset_matches_folder(&index, Some(&folder)));
        assert!(asset_matches_folder(&index, Some(&parent)));
        assert!(!asset_matches_folder(&index, Some("root/vanilla")));
    }

    #[test]
    fn idle_sort_uses_natural_last_filename_token() {
        let a1 = sample_asset(
            "mod.sample.sample.sounds.entity.test.active1.ogg",
            AssetSourceType::Mod,
            "sample",
            "sample",
            "sounds/entity/test/active1.ogg",
        );
        let a2 = sample_asset(
            "mod.sample.sample.sounds.entity.test.active2.ogg",
            AssetSourceType::Mod,
            "sample",
            "sample",
            "sounds/entity/test/active2.ogg",
        );
        let a10 = sample_asset(
            "mod.sample.sample.sounds.entity.test.active10.ogg",
            AssetSourceType::Mod,
            "sample",
            "sample",
            "sounds/entity/test/active10.ogg",
        );

        assert_eq!(idle_asset_cmp(&a1, &a2), CmpOrdering::Less);
        assert_eq!(idle_asset_cmp(&a2, &a10), CmpOrdering::Less);
    }

    #[test]
    fn rebuild_key_counts_preserves_dup_suffix_progression() {
        let assets = vec![
            sample_asset(
                "mod.sample.sample.sounds.block.grass.step.ogg",
                AssetSourceType::Mod,
                "sample",
                "sample",
                "sounds/block/grass/step.ogg",
            ),
            sample_asset(
                "mod.sample.sample.sounds.block.grass.step.ogg.dup1",
                AssetSourceType::Mod,
                "sample",
                "sample",
                "sounds/block/grass/step.ogg",
            ),
        ];

        let mut counts = rebuild_key_counts_from_assets(&assets);
        let next = unique_key(
            "mod.sample.sample.sounds.block.grass.step.ogg".to_string(),
            &mut counts,
        );

        assert_eq!(next, "mod.sample.sample.sounds.block.grass.step.ogg.dup2");
    }

    #[test]
    fn resolve_operation_id_generates_uuid_when_missing() {
        let generated = resolve_operation_id(None);
        assert!(
            Uuid::parse_str(&generated).is_ok(),
            "generated operation id should be a UUID: {generated}"
        );

        let generated_from_empty = resolve_operation_id(Some("   ".to_string()));
        assert!(
            Uuid::parse_str(&generated_from_empty).is_ok(),
            "generated operation id from empty input should be a UUID: {generated_from_empty}"
        );
    }

    #[test]
    fn plan_export_jobs_dedupes_filenames_and_applies_audio_extension() {
        let temp_root = std::env::temp_dir().join(format!("mae-export-plan-{}", Uuid::new_v4()));
        fs::create_dir_all(&temp_root).expect("must create temp export directory");

        let audio_one = sample_audio_asset(
            "mod.audio.one.sounds.block.test.step.ogg",
            "audio-one",
            "sample",
            "sounds/block/test/step.ogg",
        );
        let audio_two = sample_audio_asset(
            "mod.audio.two.sounds.block.test.step.ogg",
            "audio-two",
            "sample",
            "sounds/block/test/step.ogg",
        );

        let jobs = plan_export_jobs(vec![audio_one, audio_two], &temp_root, AudioFormat::Mp3);
        let names = jobs
            .iter()
            .map(|job| {
                job.output_path
                    .file_name()
                    .expect("output name must exist")
                    .to_string_lossy()
                    .to_string()
            })
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec!["step.mp3".to_string(), "step_1.mp3".to_string()]
        );
        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn refresh_plan_detects_changed_new_and_removed_containers() {
        let temp_root = std::env::temp_dir().join(format!("mae-refresh-plan-{}", Uuid::new_v4()));
        fs::create_dir_all(&temp_root).expect("must create temp root");

        let container_a = temp_root.join("a.jar");
        let container_b = temp_root.join("b.jar");
        let container_c = temp_root.join("c.jar");
        fs::write(&container_a, b"a").expect("must write a");
        fs::write(&container_b, b"b").expect("must write b");

        let cached_a = ScanContainer {
            source_type: AssetSourceType::Mod,
            source_name: "a".to_string(),
            container_type: AssetContainerType::Jar,
            container_path: container_a.clone(),
        };
        let cached_b = ScanContainer {
            source_type: AssetSourceType::Mod,
            source_name: "b".to_string(),
            container_type: AssetContainerType::Jar,
            container_path: container_b.clone(),
        };
        let mut cached_signatures = HashMap::new();
        cached_signatures.insert(
            scan_container_key(&cached_a),
            container_signature_for_path(&container_a, &AssetContainerType::Jar)
                .expect("signature for a"),
        );
        cached_signatures.insert(
            scan_container_key(&cached_b),
            container_signature_for_path(&container_b, &AssetContainerType::Jar)
                .expect("signature for b"),
        );

        fs::write(&container_c, b"c").expect("must write c");
        let current = vec![
            cached_a.clone(),
            ScanContainer {
                source_type: AssetSourceType::Mod,
                source_name: "c".to_string(),
                container_type: AssetContainerType::Jar,
                container_path: container_c.clone(),
            },
        ];

        let plan = build_scan_refresh_plan(&cached_signatures, &current).expect("refresh plan");
        assert_eq!(plan.unchanged_keys.len(), 1);
        assert_eq!(plan.changed_or_new.len(), 1);
        assert_eq!(plan.removed_keys.len(), 1);
        assert!(plan
            .removed_keys
            .contains(&scan_container_key(&cached_b)));

        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn reconciliation_maps_assets_with_same_identity() {
        let old = sample_asset(
            "mod.sample.sample.textures.item.star.png",
            AssetSourceType::Mod,
            "sample",
            "sample",
            "textures/item/star.png",
        );
        let mut new = old.clone();
        new.asset_id = "mod.sample.sample.textures.item.star.png.dup1".to_string();
        new.key = new.asset_id.clone();

        let mapping = build_asset_reconciliation_map(&[old.clone()], &[new.clone()]);
        assert_eq!(
            mapping.get(&old.asset_id).map(|value| value.as_str()),
            Some(new.asset_id.as_str())
        );
    }

    fn sample_asset(
        key: &str,
        source_type: AssetSourceType,
        source_name: &str,
        namespace: &str,
        relative_asset_path: &str,
    ) -> AssetRecord {
        AssetRecord {
            asset_id: key.to_string(),
            key: key.to_string(),
            source_type,
            source_name: source_name.to_string(),
            namespace: namespace.to_string(),
            relative_asset_path: relative_asset_path.to_string(),
            extension: Path::new(relative_asset_path)
                .extension()
                .map(|value| value.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default(),
            is_image: true,
            is_audio: false,
            container_path: "/tmp/container".to_string(),
            container_type: AssetContainerType::Jar,
            entry_path: format!("assets/{namespace}/{relative_asset_path}"),
        }
    }

    fn sample_audio_asset(
        key: &str,
        source_name: &str,
        namespace: &str,
        relative_asset_path: &str,
    ) -> AssetRecord {
        AssetRecord {
            asset_id: key.to_string(),
            key: key.to_string(),
            source_type: AssetSourceType::Mod,
            source_name: source_name.to_string(),
            namespace: namespace.to_string(),
            relative_asset_path: relative_asset_path.to_string(),
            extension: "ogg".to_string(),
            is_image: false,
            is_audio: true,
            container_path: "/tmp/container".to_string(),
            container_type: AssetContainerType::Jar,
            entry_path: format!("assets/{namespace}/{relative_asset_path}"),
        }
    }
}
