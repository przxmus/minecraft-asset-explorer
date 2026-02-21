import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  type CSSProperties,
  type ReactElement,
  useCallback,
  useDeferredValue,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import {
  AssetListPanel,
  ContentOverlay,
  PreviewPanel,
  StatusStrip,
  TopBar,
  TreePanel,
} from "./components";
import type {
  AssetPreviewResponse,
  AssetRecord,
  AudioFormat,
  InstanceInfo,
  PrismRootCandidate,
  ScanCompletedEvent,
  ScanLifecycle,
  ScanProgressEvent,
  SearchResponse,
  SelectionModifiers,
  TreeNode,
} from "./types/assets";
import { decodePreviewJson, renderHighlightedJson } from "./utils/jsonPreview";
import "./App.css";

const ROOT_NODE_ID = "root";
const SEARCH_PAGE_SIZE = 320;
const SEARCH_DEBOUNCE_MS = 260;
const AUTO_SCAN_DEBOUNCE_MS = 260;
const PROGRESS_STATUS_THROTTLE_MS = 250;
const PREVIEW_TOP_GAP_PX = 14;

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
  const [topbarHeight, setTopbarHeight] = useState(0);

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
  const previewPanelRef = useRef<HTMLElement | null>(null);
  const topbarRef = useRef<HTMLElement | null>(null);
  const deferredQuery = useDeferredValue(query);

  const selectedAssetIds = useMemo(() => Array.from(selectedAssets), [selectedAssets]);
  const assetById = useMemo(
    () => new Map(assets.map((asset) => [asset.assetId, asset])),
    [assets],
  );

  const activeAsset = useMemo(() => {
    if (!activeAssetId) {
      return null;
    }

    return assetById.get(activeAssetId) ?? null;
  }, [activeAssetId, assetById]);

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
      setDebouncedQuery(deferredQuery.trim());
    }, SEARCH_DEBOUNCE_MS);

    return () => {
      window.clearTimeout(timeout);
    };
  }, [deferredQuery]);

  useEffect(() => {
    const element = topbarRef.current;
    if (!element) {
      return;
    }

    const updateHeight = () => {
      setTopbarHeight(Math.ceil(element.getBoundingClientRect().height));
    };

    updateHeight();
    const observer = new ResizeObserver(updateHeight);
    observer.observe(element);
    window.addEventListener("resize", updateHeight);

    return () => {
      observer.disconnect();
      window.removeEventListener("resize", updateHeight);
    };
  }, []);

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

      const isJsonAsset =
        activeAsset.extension.toLowerCase() === "json" ||
        activeAsset.extension.toLowerCase() === "mcmeta";

      if (!activeAsset.isImage && !activeAsset.isAudio && !isJsonAsset) {
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

    previewContentRef.current?.scrollTo({ top: 0, behavior: "auto" });

    if (window.matchMedia("(max-width: 1180px)").matches) {
      return;
    }

    const panel = previewPanelRef.current;
    if (!panel) {
      return;
    }

    const stickyTopOffset = topbarHeight + PREVIEW_TOP_GAP_PX;
    const bounds = panel.getBoundingClientRect();
    if (bounds.top >= stickyTopOffset && bounds.bottom <= window.innerHeight) {
      return;
    }

    const targetTop = Math.max(0, window.scrollY + bounds.top - stickyTopOffset);
    window.scrollTo({ top: targetTop, behavior: "smooth" });
  }, [activeAssetId, topbarHeight]);

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
  const jsonPreviewText = useMemo(() => {
    if (!activeAsset || !activeAssetIsJson || !currentPreview) {
      return null;
    }

    return decodePreviewJson(currentPreview.base64);
  }, [activeAsset, activeAssetIsJson, currentPreview]);

  const highlightedJson = useMemo(() => {
    if (!jsonPreviewText) {
      return null;
    }

    return renderHighlightedJson(jsonPreviewText);
  }, [jsonPreviewText]);

  const renderedTree = useMemo(() => renderTree(ROOT_NODE_ID, 0), [renderTree]);
  const appShellStyle = useMemo(
    () =>
      ({
        "--topbar-height": `${topbarHeight}px`,
      }) as CSSProperties,
    [topbarHeight],
  );

  const progressPercent =
    progress && progress.totalContainers > 0
      ? Math.round((progress.scannedContainers / progress.totalContainers) * 100)
      : 0;

  const lifecycleDotClass =
    lifecycle === "scanning"
      ? "status-dot--scanning"
      : lifecycle === "completed"
        ? "status-dot--completed"
        : lifecycle === "error"
          ? "status-dot--error"
          : "";

  const handleAssetListScroll = useCallback(() => {
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

    const distanceToBottom = element.scrollHeight - element.scrollTop - element.clientHeight;
    if (distanceToBottom < 260) {
      void fetchSearchPage(false);
    }
  }, [fetchSearchPage, isExplorerLocked]);

  return (
    <div className="app-shell" style={appShellStyle}>
      <header className="topbar" ref={topbarRef}>
        <TopBar
          prismRootInput={prismRootInput}
          onPrismRootInputChange={setPrismRootInput}
          onCommitPrismRoot={() => commitPrismRoot(prismRootInput)}
          instances={instances}
          selectedInstance={selectedInstance}
          onSelectedInstanceChange={setSelectedInstance}
          includeVanilla={includeVanilla}
          onIncludeVanillaChange={setIncludeVanilla}
          includeMods={includeMods}
          onIncludeModsChange={setIncludeMods}
          includeResourcepacks={includeResourcepacks}
          onIncludeResourcepacksChange={setIncludeResourcepacks}
          query={query}
          onQueryChange={setQuery}
          filterImages={filterImages}
          onFilterImagesChange={setFilterImages}
          filterAudio={filterAudio}
          onFilterAudioChange={setFilterAudio}
          filterOther={filterOther}
          onFilterOtherChange={setFilterOther}
          audioFormat={audioFormat}
          onAudioFormatChange={setAudioFormat}
          isExplorerLocked={isExplorerLocked}
          selectedAssetCount={selectedAssetIds.length}
          isCopying={isCopying}
          isSaving={isSaving}
          onSelectAllVisible={selectAllVisible}
          onClearSelection={clearSelection}
          onCopySelection={() => {
            void copyAssets(selectedAssetIds);
          }}
          onSaveSelection={() => {
            void saveAssets(selectedAssetIds);
          }}
        />
        <StatusStrip
          lifecycle={lifecycle}
          lifecycleDotClass={lifecycleDotClass}
          progress={progress}
          progressPercent={progressPercent}
          statusLine={statusLine}
        />
      </header>

      <main className={`content-grid ${isExplorerLocked ? "content-grid-locked" : ""}`}>
        <TreePanel
          selectedFolderId={selectedFolderId}
          rootNodeId={ROOT_NODE_ID}
          renderedTree={renderedTree}
          onSelectRootFolder={() => setSelectedFolderId(ROOT_NODE_ID)}
        />
        <AssetListPanel
          assets={assets}
          searchTotal={searchTotal}
          isSearchLoading={isSearchLoading}
          hasMoreSearch={hasMoreSearch}
          isExplorerLocked={isExplorerLocked}
          selectedAssets={selectedAssets}
          virtualTotalSize={virtualizer.getTotalSize()}
          virtualItems={virtualizer.getVirtualItems()}
          listParentRef={listParentRef}
          onListScroll={handleAssetListScroll}
          onApplySelection={applySelection}
          onCopyAsset={(assetId) => {
            void copyAssets([assetId]);
          }}
          onSaveAsset={(assetId) => {
            void saveAssets([assetId]);
          }}
          onLoadMore={() => {
            void fetchSearchPage(false);
          }}
        />
        <PreviewPanel
          activeAsset={activeAsset}
          currentPreview={currentPreview}
          activeAssetIsJson={activeAssetIsJson}
          highlightedJson={highlightedJson}
          previewPanelRef={previewPanelRef}
          previewContentRef={previewContentRef}
          onCopyActiveAsset={(assetId) => {
            void copyAssets([assetId]);
          }}
          onSaveActiveAsset={(assetId) => {
            void saveAssets([assetId]);
          }}
        />
        {isExplorerLocked ? (
          <ContentOverlay needsInstanceSelection={needsInstanceSelection} instances={instances} />
        ) : null}
      </main>
    </div>
  );
}

export default App;
