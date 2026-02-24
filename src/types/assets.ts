export type PrismRootCandidate = {
  path: string;
  exists: boolean;
  valid: boolean;
  source: string;
};

export type InstanceInfo = {
  folderName: string;
  displayName: string;
  path: string;
  minecraftVersion: string | null;
};

export type AssetSourceType = "vanilla" | "mod" | "resourcePack";
export type AssetContainerType = "directory" | "zip" | "jar" | "assetIndex";

export type AssetRecord = {
  assetId: string;
  key: string;
  sourceType: AssetSourceType;
  sourceName: string;
  namespace: string;
  relativeAssetPath: string;
  extension: string;
  isImage: boolean;
  isAudio: boolean;
  containerPath: string;
  containerType: AssetContainerType;
  entryPath: string;
};

export type ScanLifecycle = "scanning" | "completed" | "cancelled" | "error";
export type ScanPhase = "estimating" | "scanning" | "refreshing";

export type StartScanResponse = {
  scanId: string;
  cacheHit: boolean;
  refreshStarted: boolean;
  refreshMode?: "incremental" | "full";
};

export type TreeNodeType = "folder" | "file";

export type TreeNode = {
  id: string;
  name: string;
  nodeType: TreeNodeType;
  hasChildren: boolean;
  assetId: string | null;
};

export type ScanProgressEvent = {
  scanId: string;
  scannedContainers: number;
  totalContainers: number;
  assetCount: number;
  phase: ScanPhase;
  currentSource?: string;
};

export type ScanStatus = {
  scanId: string;
  lifecycle: ScanLifecycle;
  isRefreshing: boolean;
  scannedContainers: number;
  totalContainers: number;
  assetCount: number;
  error?: string;
};

export type ScanCompletedEvent = {
  scanId: string;
  lifecycle: ScanLifecycle;
  assetCount: number;
  error?: string;
};

export type AssetPreviewResponse = {
  mime: string;
  base64: string;
};

export type AudioFormat = "original" | "mp3" | "wav";

export type ExportOperationKind = "save" | "copy";

export type ExportFailure = {
  assetId: string;
  key: string;
  error: string;
};

export type ExportProgressEvent = {
  operationId: string;
  kind: ExportOperationKind;
  requestedCount: number;
  processedCount: number;
  successCount: number;
  failedCount: number;
  cancelled: boolean;
};

export type ExportCompletedEvent = {
  operationId: string;
  kind: ExportOperationKind;
  requestedCount: number;
  processedCount: number;
  successCount: number;
  failedCount: number;
  cancelled: boolean;
  failures: ExportFailure[];
};

export type SaveAssetsResult = {
  operationId: string;
  requestedCount: number;
  processedCount: number;
  successCount: number;
  failedCount: number;
  cancelled: boolean;
  failures: ExportFailure[];
  savedFiles: string[];
};

export type CopyResult = {
  operationId: string;
  requestedCount: number;
  processedCount: number;
  successCount: number;
  failedCount: number;
  cancelled: boolean;
  failures: ExportFailure[];
  copiedFiles: string[];
};

export type SearchResponse = {
  total: number;
  assets: AssetRecord[];
};

export type ReconcileAssetIdsResponse = {
  idMap: Record<string, string>;
  assetIds: string[];
};

export type SelectionModifiers = {
  shiftKey: boolean;
  metaKey: boolean;
  ctrlKey: boolean;
};
