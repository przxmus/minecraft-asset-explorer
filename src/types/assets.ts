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
export type AssetContainerType = "directory" | "zip" | "jar";

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
  currentSource?: string;
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

export type SearchResponse = {
  total: number;
  assets: AssetRecord[];
};

export type SelectionModifiers = {
  shiftKey: boolean;
  metaKey: boolean;
  ctrlKey: boolean;
};
