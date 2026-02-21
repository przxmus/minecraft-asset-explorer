import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  type CSSProperties,
  type KeyboardEvent,
  type MouseEvent,
  type ReactElement,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import "./App.css";

type PrismRootCandidate = {
  path: string;
  exists: boolean;
  valid: boolean;
  source: string;
};

type InstanceInfo = {
  folderName: string;
  displayName: string;
  path: string;
  minecraftVersion: string | null;
};

type AssetSourceType = "vanilla" | "mod" | "resourcePack";
type AssetContainerType = "directory" | "zip" | "jar";

type AssetRecord = {
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

type ScanLifecycle = "scanning" | "completed" | "cancelled" | "error";

type TreeNodeType = "folder" | "file";

type TreeNode = {
  id: string;
  name: string;
  nodeType: TreeNodeType;
  hasChildren: boolean;
  assetId: string | null;
};

type ScanProgressEvent = {
  scanId: string;
  scannedContainers: number;
  totalContainers: number;
  assetCount: number;
  currentSource?: string;
};

type ScanChunkEvent = {
  scanId: string;
  assets: AssetRecord[];
};

type ScanCompletedEvent = {
  scanId: string;
  lifecycle: ScanLifecycle;
  assetCount: number;
  error?: string;
};

type AssetPreviewResponse = {
  mime: string;
  base64: string;
};

type AudioFormat = "original" | "mp3" | "wav";

type SearchResponse = {
  total: number;
  assets: AssetRecord[];
};

type SelectionModifiers = {
  shiftKey: boolean;
  metaKey: boolean;
  ctrlKey: boolean;
};

const ROOT_NODE_ID = "root";
const SEARCH_PAGE_SIZE = 320;
const SEARCH_DEBOUNCE_MS = 260;
const AUTO_SCAN_DEBOUNCE_MS = 260;
const PROGRESS_STATUS_THROTTLE_MS = 250;

function App() {
  const [prismRootInput, setPrismRootInput] = useState("");
  const [prismRootCommitted, setPrismRootCommitted] = useState("");
  const [instances, setInstances] = useState<InstanceInfo[]>([]);
  const [selectedInstance, setSelectedInstance] = useState("");

  const [includeVanilla, setIncludeVanilla] = useState(true);
  const [includeMods, setIncludeMods] = useState(true);
  const [includeResourcepacks, setIncludeResourcepacks] = useState(true);

  const [scanId, setScanId] = useState<string | null>(null);
  const [lifecycle, setLifecycle] = useState<ScanLifecycle | "idle">("idle");
  const [progress, setProgress] = useState<ScanProgressEvent | null>(null);

  const [query, setQuery] = useState("");
  const [debouncedQuery, setDebouncedQuery] = useState("");
  const [filterImages, setFilterImages] = useState(true);
  const [filterAudio, setFilterAudio] = useState(true);
  const [filterOther, setFilterOther] = useState(false);

  const [treeByNodeId, setTreeByNodeId] = useState<Record<string, TreeNode[]>>({
    [ROOT_NODE_ID]: [],
  });
  const [expandedNodes, setExpandedNodes] = useState<Set<string>>(new Set());
  const [selectedFolderId, setSelectedFolderId] = useState(ROOT_NODE_ID);

  const [assets, setAssets] = useState<AssetRecord[]>([]);
  const [searchTotal, setSearchTotal] = useState(0);
  const [hasMoreSearch, setHasMoreSearch] = useState(false);
  const [isSearchLoading, setIsSearchLoading] = useState(false);
  const [scanRefreshToken, setScanRefreshToken] = useState(0);

  const [selectedAssets, setSelectedAssets] = useState<Set<string>>(new Set());
  const [selectionAnchorId, setSelectionAnchorId] = useState<string | null>(null);
  const [activeAssetId, setActiveAssetId] = useState<string | null>(null);
  const [previewCache, setPreviewCache] = useState<Record<string, AssetPreviewResponse>>({});

  const [audioFormat, setAudioFormat] = useState<AudioFormat>("original");
  const [isStartingScan, setIsStartingScan] = useState(false);
  const [isSaving, setIsSaving] = useState(false);
  const [isCopying, setIsCopying] = useState(false);
  const [statusLine, setStatusLine] = useState("Ready.");

  const activeScanIdRef = useRef<string | null>(null);
  const searchRequestSeqRef = useRef(0);
  const isSearchLoadingRef = useRef(false);
  const hasMoreSearchRef = useRef(false);
  const searchOffsetRef = useRef(0);
  const searchScrollThrottleRef = useRef(0);
  const lastStatusAtRef = useRef(0);
  const expandedNodesRef = useRef<Set<string>>(new Set());
  const autoScanTimeoutRef = useRef<number | null>(null);
  const listParentRef = useRef<HTMLDivElement | null>(null);
  const previewContentRef = useRef<HTMLDivElement | null>(null);

  const selectedAssetIds = useMemo(() => Array.from(selectedAssets), [selectedAssets]);

  const activeAsset = useMemo(() => {
    if (!activeAssetId) {
      return null;
    }

    return assets.find((asset) => asset.assetId === activeAssetId) ?? null;
  }, [activeAssetId, assets]);

  const virtualizer = useVirtualizer({
    count: assets.length,
    getScrollElement: () => listParentRef.current,
    estimateSize: () => 58,
    overscan: 12,
  });

  const commitPrismRoot = useCallback(
    (candidate: string) => {
      const normalized = candidate.trim();
      if (!normalized || normalized === prismRootCommitted) {
        return;
      }

      setPrismRootCommitted(normalized);
    },
    [prismRootCommitted],
  );

  const refreshInstances = useCallback(async (rootPath: string) => {
    if (!rootPath.trim()) {
      setInstances([]);
      setSelectedInstance("");
      return;
    }

    try {
      const listed = await invoke<InstanceInfo[]>("list_instances", {
        prismRoot: rootPath,
      });

      setInstances(listed);
      setSelectedInstance((current) => {
        const exists = listed.some((item) => item.folderName === current);
        return exists ? current : "";
      });
    } catch (error) {
      setStatusLine(String(error));
      setInstances([]);
      setSelectedInstance("");
    }
  }, []);

  const loadTreeChildren = useCallback(async (nodeId?: string, scanOverride?: string) => {
    const resolvedScanId = scanOverride ?? activeScanIdRef.current;
    if (!resolvedScanId) {
      return;
    }

    try {
      const children = await invoke<TreeNode[]>("list_tree_children", {
        req: {
          scanId: resolvedScanId,
          nodeId,
        },
      });

      setTreeByNodeId((current) => ({
        ...current,
        [nodeId ?? ROOT_NODE_ID]: children,
      }));
    } catch (error) {
      setStatusLine(String(error));
    }
  }, []);

  const refreshVisibleTreeNodes = useCallback(
    async (scanOverride?: string) => {
      await loadTreeChildren(ROOT_NODE_ID, scanOverride);

      const expanded = Array.from(expandedNodesRef.current);
      await Promise.all(expanded.map((nodeId) => loadTreeChildren(nodeId, scanOverride)));
    },
    [loadTreeChildren],
  );

  const resetSearchState = useCallback(() => {
    searchRequestSeqRef.current += 1;
    searchOffsetRef.current = 0;
    hasMoreSearchRef.current = false;
    isSearchLoadingRef.current = false;
    setAssets([]);
    setSearchTotal(0);
    setHasMoreSearch(false);
    setIsSearchLoading(false);
  }, []);

  const fetchSearchPage = useCallback(
    async (reset: boolean) => {
      const resolvedScanId = activeScanIdRef.current;
      if (!resolvedScanId) {
        resetSearchState();
        return;
      }

      if (isSearchLoadingRef.current && !reset) {
        return;
      }

      const offset = reset ? 0 : searchOffsetRef.current;
      const requestId = ++searchRequestSeqRef.current;

      isSearchLoadingRef.current = true;
      setIsSearchLoading(true);

      try {
        const response = await invoke<SearchResponse>("search_assets", {
          req: {
            scanId: resolvedScanId,
            query: debouncedQuery,
            folderNodeId: selectedFolderId,
            offset,
            limit: SEARCH_PAGE_SIZE,
            includeImages: filterImages,
            includeAudio: filterAudio,
            includeOther: filterOther,
          },
        });

        if (requestId !== searchRequestSeqRef.current) {
          return;
        }

        setAssets((current) => (reset ? response.assets : [...current, ...response.assets]));
        const nextOffset = offset + response.assets.length;
        searchOffsetRef.current = nextOffset;
        hasMoreSearchRef.current = nextOffset < response.total;
        setSearchTotal(response.total);
        setHasMoreSearch(nextOffset < response.total);
      } catch (error) {
        if (requestId === searchRequestSeqRef.current) {
          setStatusLine(String(error));
        }
      } finally {
        if (requestId === searchRequestSeqRef.current) {
          isSearchLoadingRef.current = false;
          setIsSearchLoading(false);
        }
      }
    },
    [
      debouncedQuery,
      filterAudio,
      filterImages,
      filterOther,
      resetSearchState,
      selectedFolderId,
    ],
  );

  const startScan = useCallback(async () => {
    if (!prismRootCommitted || !selectedInstance) {
      return;
    }

    if (!includeVanilla && !includeMods && !includeResourcepacks) {
      setStatusLine("Select at least one source to scan.");
      return;
    }

    setIsStartingScan(true);

    try {
      const previousScanId = activeScanIdRef.current;
      if (previousScanId) {
        await invoke("cancel_scan", { scanId: previousScanId }).catch(() => undefined);
      }

      resetSearchState();
      setTreeByNodeId({ [ROOT_NODE_ID]: [] });
      setExpandedNodes(new Set());
      expandedNodesRef.current = new Set();
      setSelectedFolderId(ROOT_NODE_ID);
      setSelectedAssets(new Set());
      setSelectionAnchorId(null);
      setActiveAssetId(null);
      setPreviewCache({});
      setProgress(null);
      setLifecycle("scanning");

      const response = await invoke<{ scanId: string }>("start_scan", {
        req: {
          prismRoot: prismRootCommitted,
          instanceFolder: selectedInstance,
          includeVanilla,
          includeMods,
          includeResourcepacks,
        },
      });

      activeScanIdRef.current = response.scanId;
      setScanId(response.scanId);
      setStatusLine("Scan started.");

      await refreshVisibleTreeNodes(response.scanId);
      setScanRefreshToken((value) => value + 1);
    } catch (error) {
      setStatusLine(String(error));
      setLifecycle("error");
    } finally {
      setIsStartingScan(false);
    }
  }, [
    includeMods,
    includeResourcepacks,
    includeVanilla,
    prismRootCommitted,
    refreshVisibleTreeNodes,
    resetSearchState,
    selectedInstance,
  ]);

  const applySelection = useCallback(
    (assetId: string, modifiers: SelectionModifiers) => {
      setSelectedAssets((current) => {
        if (modifiers.shiftKey && selectionAnchorId) {
          const ids = assets.map((asset) => asset.assetId);
          const anchorIndex = ids.indexOf(selectionAnchorId);
          const targetIndex = ids.indexOf(assetId);

          if (anchorIndex >= 0 && targetIndex >= 0) {
            const [start, end] =
              anchorIndex < targetIndex
                ? [anchorIndex, targetIndex]
                : [targetIndex, anchorIndex];
            return new Set(ids.slice(start, end + 1));
          }
        }

        if (modifiers.metaKey || modifiers.ctrlKey) {
          const next = new Set(current);
          if (next.has(assetId)) {
            next.delete(assetId);
          } else {
            next.add(assetId);
          }
          return next;
        }

        return new Set([assetId]);
      });

      if (!modifiers.shiftKey) {
        setSelectionAnchorId(assetId);
      }
      setActiveAssetId(assetId);
    },
    [assets, selectionAnchorId],
  );

  const selectAllVisible = useCallback(() => {
    const allIds = assets.map((asset) => asset.assetId);
    setSelectedAssets(new Set(allIds));
    if (allIds.length > 0) {
      setSelectionAnchorId(allIds[0]);
    }
  }, [assets]);

  const clearSelection = useCallback(() => {
    setSelectedAssets(new Set());
    setSelectionAnchorId(null);
  }, []);

  const saveAssets = useCallback(
    async (assetIds: string[]) => {
      if (assetIds.length === 0) {
        return;
      }

      const resolvedScanId = activeScanIdRef.current;
      if (!resolvedScanId) {
        return;
      }

      const selectedPath = await open({ directory: true, multiple: false });
      if (!selectedPath || Array.isArray(selectedPath)) {
        return;
      }

      setIsSaving(true);
      try {
        const response = await invoke<{ savedFiles: string[] }>("save_assets", {
          req: {
            scanId: resolvedScanId,
            assetIds,
            destinationDir: selectedPath,
            audioFormat,
          },
        });

        setStatusLine(`Saved ${response.savedFiles.length} file(s).`);
      } catch (error) {
        setStatusLine(String(error));
      } finally {
        setIsSaving(false);
      }
    },
    [audioFormat],
  );

  const copyAssets = useCallback(
    async (assetIds: string[]) => {
      if (assetIds.length === 0) {
        return;
      }

      const resolvedScanId = activeScanIdRef.current;
      if (!resolvedScanId) {
        return;
      }

      setIsCopying(true);
      try {
        const response = await invoke<{ copiedFiles: string[] }>("copy_assets_to_clipboard", {
          req: {
            scanId: resolvedScanId,
            assetIds,
            audioFormat,
          },
        });

        setStatusLine(`Copied ${response.copiedFiles.length} file(s) to clipboard.`);
      } catch (error) {
        setStatusLine(String(error));
      } finally {
        setIsCopying(false);
      }
    },
    [audioFormat],
  );

  const toggleFolder = useCallback(
    async (node: TreeNode) => {
      if (node.nodeType !== "folder") {
        return;
      }

      setSelectedFolderId(node.id);
      setExpandedNodes((current) => {
        const next = new Set(current);
        if (next.has(node.id)) {
          next.delete(node.id);
        } else {
          next.add(node.id);
        }
        expandedNodesRef.current = next;
        return next;
      });

      if (!treeByNodeId[node.id]) {
        await loadTreeChildren(node.id);
      }
    },
    [loadTreeChildren, treeByNodeId],
  );

  const renderTree = useCallback(
    (nodeId: string, depth: number): ReactElement[] => {
      const nodes = treeByNodeId[nodeId] ?? [];

      return nodes.flatMap((node) => {
        const isExpanded = expandedNodes.has(node.id);
        const rowStyle: CSSProperties = { paddingInlineStart: `${14 + depth * 14}px` };

        const row = (
          <button
            key={node.id}
            type="button"
            className={`tree-row ${selectedFolderId === node.id ? "tree-row-active" : ""}`}
            style={rowStyle}
            onClick={() => {
              if (node.nodeType === "folder") {
                void toggleFolder(node);
              } else if (node.assetId) {
                setActiveAssetId(node.assetId);
              }
            }}
          >
            <span className="tree-icon">
              {node.nodeType === "folder" ? (isExpanded ? "▾" : "▸") : "•"}
            </span>
            <span className="truncate">{node.name}</span>
          </button>
        );

        if (node.nodeType === "folder" && isExpanded) {
          return [row, ...renderTree(node.id, depth + 1)];
        }

        return [row];
      });
    },
    [expandedNodes, selectedFolderId, toggleFolder, treeByNodeId],
  );

  useEffect(() => {
    expandedNodesRef.current = expandedNodes;
  }, [expandedNodes]);

  useEffect(() => {
    const timeout = window.setTimeout(() => {
      setDebouncedQuery(query.trim());
    }, SEARCH_DEBOUNCE_MS);

    return () => {
      window.clearTimeout(timeout);
    };
  }, [query]);

  useEffect(() => {
    const boot = async () => {
      try {
        const roots = await invoke<PrismRootCandidate[]>("detect_prism_roots");
        const preferred = roots.find((root) => root.valid) ?? roots[0];
        if (!preferred) {
          setStatusLine("No Prism root candidates found.");
          return;
        }

        setPrismRootInput(preferred.path);
        setPrismRootCommitted(preferred.path);
      } catch (error) {
        setStatusLine(String(error));
      }
    };

    void boot();
  }, []);

  useEffect(() => {
    if (!prismRootCommitted) {
      setInstances([]);
      setSelectedInstance("");
      return;
    }

    void refreshInstances(prismRootCommitted);
  }, [prismRootCommitted, refreshInstances]);

  useEffect(() => {
    if (autoScanTimeoutRef.current) {
      window.clearTimeout(autoScanTimeoutRef.current);
      autoScanTimeoutRef.current = null;
    }

    if (!prismRootCommitted || !selectedInstance) {
      return;
    }

    if (!includeVanilla && !includeMods && !includeResourcepacks) {
      return;
    }

    autoScanTimeoutRef.current = window.setTimeout(() => {
      void startScan();
    }, AUTO_SCAN_DEBOUNCE_MS);

    return () => {
      if (autoScanTimeoutRef.current) {
        window.clearTimeout(autoScanTimeoutRef.current);
        autoScanTimeoutRef.current = null;
      }
    };
  }, [
    includeMods,
    includeResourcepacks,
    includeVanilla,
    prismRootCommitted,
    selectedInstance,
    startScan,
  ]);

  useEffect(() => {
    if (!scanId) {
      resetSearchState();
      return;
    }

    listParentRef.current?.scrollTo({ top: 0 });
    void fetchSearchPage(true);
  }, [debouncedQuery, fetchSearchPage, resetSearchState, scanId, scanRefreshToken, selectedFolderId]);

  useEffect(() => {
    const loadPreview = async () => {
      const resolvedScanId = activeScanIdRef.current;
      if (!resolvedScanId || !activeAsset) {
        return;
      }

      if (!activeAsset.isImage && !activeAsset.isAudio) {
        return;
      }

      if (previewCache[activeAsset.assetId]) {
        return;
      }

      try {
        const preview = await invoke<AssetPreviewResponse>("get_asset_preview", {
          scanId: resolvedScanId,
          assetId: activeAsset.assetId,
        });

        setPreviewCache((current) => ({
          ...current,
          [activeAsset.assetId]: preview,
        }));
      } catch (error) {
        setStatusLine(String(error));
      }
    };

    void loadPreview();
  }, [activeAsset, previewCache]);

  useEffect(() => {
    if (!activeAssetId) {
      return;
    }

    previewContentRef.current?.scrollTo({ top: 0, behavior: "smooth" });
  }, [activeAssetId]);

  useEffect(() => {
    const registerListeners = async () => {
      const unlistenProgress = await listen<ScanProgressEvent>("scan://progress", (event) => {
        if (event.payload.scanId !== activeScanIdRef.current) {
          return;
        }

        setProgress(event.payload);
        const now = Date.now();
        if (now - lastStatusAtRef.current >= PROGRESS_STATUS_THROTTLE_MS) {
          lastStatusAtRef.current = now;
          setStatusLine(
            `Scanning ${event.payload.scannedContainers}/${event.payload.totalContainers} containers · ${event.payload.assetCount} assets`,
          );
        }
      });

      const unlistenChunk = await listen<ScanChunkEvent>("scan://chunk", () => {});

      const unlistenComplete = await listen<ScanCompletedEvent>("scan://completed", (event) => {
        if (event.payload.scanId !== activeScanIdRef.current) {
          return;
        }

        setLifecycle(event.payload.lifecycle);
        setStatusLine(
          event.payload.lifecycle === "completed"
            ? `Scan completed: ${event.payload.assetCount} assets indexed.`
            : `Scan finished with status: ${event.payload.lifecycle}`,
        );

        void refreshVisibleTreeNodes();
        setScanRefreshToken((value) => value + 1);
      });

      const unlistenError = await listen<{ scanId: string; error: string }>(
        "scan://error",
        (event) => {
          if (event.payload.scanId !== activeScanIdRef.current) {
            return;
          }

          setLifecycle("error");
          setStatusLine(event.payload.error);
        },
      );

      return () => {
        unlistenProgress();
        unlistenChunk();
        unlistenComplete();
        unlistenError();
      };
    };

    let teardown: (() => void) | undefined;
    void registerListeners().then((cleanup) => {
      teardown = cleanup;
    });

    return () => {
      teardown?.();
    };
  }, [refreshVisibleTreeNodes]);

  const currentPreview = activeAsset ? previewCache[activeAsset.assetId] : undefined;
  const needsInstanceSelection = !selectedInstance;
  const isScanInProgress = lifecycle === "scanning" || isStartingScan;
  const isExplorerLocked = needsInstanceSelection || isScanInProgress;
  const activeAssetIsJson =
    !!activeAsset &&
    (activeAsset.extension.toLowerCase() === "json" ||
      activeAsset.extension.toLowerCase() === "mcmeta");
  const jsonPreviewText =
    activeAsset && activeAssetIsJson && currentPreview
      ? decodePreviewJson(currentPreview.base64)
      : null;

  return (
    <div className="app-shell">
      <header className="topbar">
        <div className="topbar-grid">
          <div className="field-group">
            <label className="field-label" htmlFor="prism-root-input">
              Prism Root
            </label>
            <input
              id="prism-root-input"
              className="mae-input"
              placeholder="PrismLauncher path"
              value={prismRootInput}
              onChange={(event) => setPrismRootInput(event.currentTarget.value)}
              onBlur={() => commitPrismRoot(prismRootInput)}
              onKeyDown={(event: KeyboardEvent<HTMLInputElement>) => {
                if (event.key === "Enter") {
                  commitPrismRoot(prismRootInput);
                }
              }}
            />
          </div>

          <div className="field-group">
            <label className="field-label" htmlFor="instance-select">
              Instance
            </label>
            <div className="field-row">
              <select
                id="instance-select"
                className="mae-select"
                value={selectedInstance}
                onChange={(event) => setSelectedInstance(event.currentTarget.value)}
              >
                <option value="">Select instance...</option>
                {instances.map((instance) => (
                  <option key={instance.folderName} value={instance.folderName}>
                    {instance.displayName}
                    {instance.minecraftVersion ? ` (MC ${instance.minecraftVersion})` : ""}
                  </option>
                ))}
              </select>
            </div>
          </div>

          <div className="field-group">
            <div className="field-label">Sources</div>
            <div className="field-row checkbox-row">
              <label className="mae-checkbox">
                <input
                  type="checkbox"
                  checked={includeVanilla}
                  onChange={(event) => setIncludeVanilla(event.currentTarget.checked)}
                />
                Vanilla
              </label>
              <label className="mae-checkbox">
                <input
                  type="checkbox"
                  checked={includeMods}
                  onChange={(event) => setIncludeMods(event.currentTarget.checked)}
                />
                Mods
              </label>
              <label className="mae-checkbox">
                <input
                  type="checkbox"
                  checked={includeResourcepacks}
                  onChange={(event) => setIncludeResourcepacks(event.currentTarget.checked)}
                />
                Resourcepacks
              </label>
            </div>
          </div>
        </div>

        <div className="status-row">
          <div>
            <strong>Status:</strong> {lifecycle}
            {progress ? (
              <span>
                {" "}
                | {progress.scannedContainers}/{progress.totalContainers} containers |{" "}
                {progress.assetCount} assets
              </span>
            ) : null}
          </div>
          <div className="truncate">{statusLine}</div>
        </div>

        <div className="search-row">
          <input
            className="mae-search"
            value={query}
            onChange={(event) => setQuery(event.currentTarget.value)}
            placeholder="Search: star, atm star, all the star, item star"
            disabled={isExplorerLocked}
          />

          <select
            className="mae-select audio-select"
            value={audioFormat}
            onChange={(event) => setAudioFormat(event.currentTarget.value as AudioFormat)}
            disabled={isExplorerLocked}
          >
            <option value="original">Audio: original</option>
            <option value="mp3">Audio: mp3</option>
            <option value="wav">Audio: wav</option>
          </select>

          <label className="mae-checkbox filter-pill">
            <input
              type="checkbox"
              checked={filterImages}
              onChange={(event) => setFilterImages(event.currentTarget.checked)}
              disabled={isExplorerLocked}
            />
            Images
          </label>

          <label className="mae-checkbox filter-pill">
            <input
              type="checkbox"
              checked={filterAudio}
              onChange={(event) => setFilterAudio(event.currentTarget.checked)}
              disabled={isExplorerLocked}
            />
            Audio
          </label>

          <label className="mae-checkbox filter-pill">
            <input
              type="checkbox"
              checked={filterOther}
              onChange={(event) => setFilterOther(event.currentTarget.checked)}
              disabled={isExplorerLocked}
            />
            Other
          </label>

          <button
            type="button"
            className="mae-button"
            onClick={selectAllVisible}
            disabled={isExplorerLocked}
          >
            Select visible
          </button>
          <button
            type="button"
            className="mae-button"
            onClick={clearSelection}
            disabled={isExplorerLocked}
          >
            Clear
          </button>
          <button
            type="button"
            className="mae-button"
            disabled={isExplorerLocked || isCopying || selectedAssetIds.length === 0}
            onClick={() => {
              void copyAssets(selectedAssetIds);
            }}
          >
            {isCopying ? "Copying..." : `Copy selected (${selectedAssetIds.length})`}
          </button>
          <button
            type="button"
            className="mae-button mae-button-accent"
            disabled={isExplorerLocked || isSaving || selectedAssetIds.length === 0}
            onClick={() => {
              void saveAssets(selectedAssetIds);
            }}
          >
            {isSaving ? "Saving..." : `Save selected (${selectedAssetIds.length})`}
          </button>
        </div>
      </header>

      <main className={`content-grid ${isExplorerLocked ? "content-grid-locked" : ""}`}>
        <aside className="tree-panel">
          <div className="panel-title">Explorer</div>
          <button
            type="button"
            className={`tree-row ${selectedFolderId === ROOT_NODE_ID ? "tree-row-active" : ""}`}
            onClick={() => setSelectedFolderId(ROOT_NODE_ID)}
          >
            <span className="tree-icon">▾</span>
            <span>All assets</span>
          </button>
          <div className="tree-scroll">{renderTree(ROOT_NODE_ID, 0)}</div>
        </aside>

        <section className="list-panel">
          <div className="panel-title">
            Assets ({assets.length}/{searchTotal})
            {isSearchLoading ? " · loading..." : ""}
          </div>

          <div
            className="asset-list"
            ref={listParentRef}
            onScroll={() => {
              if (isExplorerLocked) {
                return;
              }
              const element = listParentRef.current;
              if (!element || isSearchLoadingRef.current || !hasMoreSearchRef.current) {
                return;
              }

              const now = Date.now();
              if (now - searchScrollThrottleRef.current < 120) {
                return;
              }
              searchScrollThrottleRef.current = now;

              const distanceToBottom =
                element.scrollHeight - element.scrollTop - element.clientHeight;
              if (distanceToBottom < 260) {
                void fetchSearchPage(false);
              }
            }}
          >
            <div
              style={{
                height: `${virtualizer.getTotalSize()}px`,
                position: "relative",
                width: "100%",
              }}
            >
              {virtualizer.getVirtualItems().map((virtualRow) => {
                const asset = assets[virtualRow.index];
                const isSelected = selectedAssets.has(asset.assetId);

                const rowStyle: CSSProperties = {
                  position: "absolute",
                  top: 0,
                  left: 0,
                  transform: `translateY(${virtualRow.start}px)`,
                  height: `${virtualRow.size}px`,
                  width: "100%",
                };

                return (
                  <div
                    key={asset.assetId}
                    className={`asset-row ${isSelected ? "asset-row-selected" : ""}`}
                    style={rowStyle}
                    role="button"
                    tabIndex={0}
                    onClick={(event: MouseEvent<HTMLDivElement>) => {
                      applySelection(asset.assetId, {
                        shiftKey: event.shiftKey,
                        metaKey: event.metaKey,
                        ctrlKey: event.ctrlKey,
                      });
                    }}
                    onKeyDown={(event) => {
                      if (event.key !== "Enter" && event.key !== " ") {
                        return;
                      }

                      event.preventDefault();
                      applySelection(asset.assetId, {
                        shiftKey: event.shiftKey,
                        metaKey: event.metaKey,
                        ctrlKey: event.ctrlKey,
                      });
                    }}
                  >
                    <input
                      type="checkbox"
                      readOnly
                      checked={isSelected}
                      onClick={(event) => {
                        event.stopPropagation();
                        applySelection(asset.assetId, {
                          shiftKey: event.shiftKey,
                          metaKey: event.metaKey,
                          ctrlKey: event.ctrlKey,
                        });
                      }}
                    />

                    <button type="button" className="asset-main">
                      <span className="asset-title">{asset.key}</span>
                      <span className="asset-subtitle">
                        {asset.sourceName} / {asset.namespace} / {asset.relativeAssetPath}
                      </span>
                    </button>

                    <button
                      type="button"
                      className="mae-button"
                      onClick={(event) => {
                        event.stopPropagation();
                        void copyAssets([asset.assetId]);
                      }}
                    >
                      Copy
                    </button>

                    <button
                      type="button"
                      className="mae-button"
                      onClick={(event) => {
                        event.stopPropagation();
                        void saveAssets([asset.assetId]);
                      }}
                    >
                      Save
                    </button>
                  </div>
                );
              })}
            </div>

            {hasMoreSearch ? (
              <div className="load-more-wrap">
                <button
                  type="button"
                  className="mae-button"
                  disabled={isExplorerLocked || isSearchLoading}
                  onClick={() => {
                    void fetchSearchPage(false);
                  }}
                >
                  {isSearchLoading ? "Loading..." : "Load more"}
                </button>
              </div>
            ) : null}
          </div>
        </section>

        <aside className="preview-panel">
          <div className="panel-title">Preview</div>
          {!activeAsset ? (
            <p className="muted">Select an asset to see preview.</p>
          ) : (
            <div className="preview-content" ref={previewContentRef}>
              <div className="preview-key">{activeAsset.key}</div>
              <div className="preview-meta">
                {activeAsset.containerType} · {activeAsset.extension || "no-ext"}
              </div>

              {activeAsset.isImage && currentPreview ? (
                <img
                  className="preview-image"
                  src={`data:${currentPreview.mime};base64,${currentPreview.base64}`}
                  alt={activeAsset.key}
                />
              ) : null}

              {activeAsset.isAudio && currentPreview ? (
                <audio
                  className="preview-audio"
                  controls
                  preload="metadata"
                  src={`data:${currentPreview.mime};base64,${currentPreview.base64}`}
                />
              ) : null}

              {activeAssetIsJson && jsonPreviewText ? (
                <pre className="json-preview">{renderHighlightedJson(jsonPreviewText)}</pre>
              ) : null}

              {!currentPreview ? (
                <div className="preview-fallback">Loading preview...</div>
              ) : null}

              {!activeAsset.isImage && !activeAsset.isAudio && !activeAssetIsJson ? (
                <div className="preview-fallback">
                  Preview is available for image, audio and JSON assets.
                </div>
              ) : null}

              <div className="preview-actions">
                <button
                  type="button"
                  className="mae-button"
                  onClick={() => {
                    void copyAssets([activeAsset.assetId]);
                  }}
                >
                  Copy file
                </button>
                <button
                  type="button"
                  className="mae-button mae-button-accent"
                  onClick={() => {
                    void saveAssets([activeAsset.assetId]);
                  }}
                >
                  Save file
                </button>
              </div>
            </div>
          )}
        </aside>

        {isExplorerLocked ? (
          <div className="content-overlay">
            <div className="overlay-card">
              <div className="overlay-title">
                {needsInstanceSelection ? "Choose an instance" : "Loading assets..."}
              </div>
              <div className="overlay-subtitle">
                {needsInstanceSelection
                  ? instances.length === 0
                    ? "No valid instances found in this Prism root."
                    : "Select an instance to start scanning assets."
                  : "Explorer will unlock automatically after scan completes."}
              </div>
            </div>
          </div>
        ) : null}
      </main>
    </div>
  );
}

function decodePreviewJson(base64: string): string {
  try {
    const binary = atob(base64);
    const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
    const raw = new TextDecoder().decode(bytes);
    const parsed = JSON.parse(raw);
    return JSON.stringify(parsed, null, 2);
  } catch {
    return "Invalid JSON content.";
  }
}

function renderHighlightedJson(value: string): ReactElement[] {
  const tokenRegex =
    /(\"(?:\\u[a-fA-F0-9]{4}|\\[^u]|[^\\\"])*\"\\s*:?)|\\b(true|false|null)\\b|-?\\d+(?:\\.\\d+)?(?:[eE][+\\-]?\\d+)?/g;

  const nodes: ReactElement[] = [];
  let lastIndex = 0;
  let match: RegExpExecArray | null = tokenRegex.exec(value);
  let key = 0;

  while (match) {
    const token = match[0];
    const start = match.index;

    if (start > lastIndex) {
      nodes.push(
        <span className="json-punctuation" key={`plain-${key++}`}>
          {value.slice(lastIndex, start)}
        </span>,
      );
    }

    let className = "json-number";
    if (/\"\\s*:$/.test(token)) {
      className = "json-key";
    } else if (token.startsWith('"')) {
      className = "json-string";
    } else if (/^(true|false|null)$/.test(token)) {
      className = "json-literal";
    }

    nodes.push(
      <span className={className} key={`tok-${key++}`}>
        {token}
      </span>,
    );

    lastIndex = start + token.length;
    match = tokenRegex.exec(value);
  }

  if (lastIndex < value.length) {
    nodes.push(
      <span className="json-punctuation" key={`plain-${key++}`}>
        {value.slice(lastIndex)}
      </span>,
    );
  }

  return nodes;
}

export default App;
