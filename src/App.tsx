import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  type CSSProperties,
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

const ROOT_NODE_ID = "root";

function App() {
  const [rootCandidates, setRootCandidates] = useState<PrismRootCandidate[]>([]);
  const [prismRoot, setPrismRoot] = useState("");
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
  const [searchResults, setSearchResults] = useState<AssetRecord[]>([]);

  const [treeByNodeId, setTreeByNodeId] = useState<Record<string, TreeNode[]>>({
    [ROOT_NODE_ID]: [],
  });
  const [expandedNodes, setExpandedNodes] = useState<Set<string>>(new Set());
  const [selectedFolderId, setSelectedFolderId] = useState(ROOT_NODE_ID);

  const [selectedAssets, setSelectedAssets] = useState<Set<string>>(new Set());
  const [activeAssetId, setActiveAssetId] = useState<string | null>(null);
  const [previewCache, setPreviewCache] = useState<Record<string, AssetPreviewResponse>>({});

  const [audioFormat, setAudioFormat] = useState<AudioFormat>("original");
  const [isStartingScan, setIsStartingScan] = useState(false);
  const [isSaving, setIsSaving] = useState(false);
  const [isCopying, setIsCopying] = useState(false);
  const [statusLine, setStatusLine] = useState("Gotowe.");

  const listParentRef = useRef<HTMLDivElement | null>(null);

  const activeAsset = useMemo(() => {
    if (!activeAssetId) {
      return null;
    }

    return searchResults.find((asset) => asset.assetId === activeAssetId) ?? null;
  }, [activeAssetId, searchResults]);

  const selectedAssetIds = useMemo(() => Array.from(selectedAssets), [selectedAssets]);

  const visibleAssets = useMemo(() => {
    if (selectedFolderId === ROOT_NODE_ID) {
      return searchResults;
    }

    return searchResults.filter((asset) => {
      const folderPath = assetFolderNodeId(asset);
      return (
        folderPath === selectedFolderId || folderPath.startsWith(`${selectedFolderId}/`)
      );
    });
  }, [searchResults, selectedFolderId]);

  const virtualizer = useVirtualizer({
    count: visibleAssets.length,
    getScrollElement: () => listParentRef.current,
    estimateSize: () => 56,
    overscan: 10,
  });

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

      if (listed.length > 0) {
        setSelectedInstance((current) => {
          const existing = listed.some((item) => item.folderName === current);
          return existing ? current : listed[0].folderName;
        });
      } else {
        setSelectedInstance("");
      }
    } catch (error) {
      setStatusLine(String(error));
      setInstances([]);
      setSelectedInstance("");
    }
  }, []);

  const loadTreeChildren = useCallback(
    async (nodeId?: string) => {
      if (!scanId) {
        return;
      }

      try {
        const children = await invoke<TreeNode[]>("list_tree_children", {
          req: {
            scanId,
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
    },
    [scanId],
  );

  const refreshSearch = useCallback(async () => {
    if (!scanId) {
      setSearchResults([]);
      return;
    }

    try {
      const response = await invoke<{ total: number; assets: AssetRecord[] }>(
        "search_assets",
        {
          req: {
            scanId,
            query: debouncedQuery,
            offset: 0,
            limit: 5000,
          },
        },
      );

      setSearchResults(response.assets);
    } catch (error) {
      setStatusLine(String(error));
    }
  }, [debouncedQuery, scanId]);

  useEffect(() => {
    const timeout = window.setTimeout(() => {
      setDebouncedQuery(query.trim());
    }, 120);

    return () => {
      window.clearTimeout(timeout);
    };
  }, [query]);

  useEffect(() => {
    const loadRoots = async () => {
      try {
        const roots = await invoke<PrismRootCandidate[]>("detect_prism_roots");
        setRootCandidates(roots);

        const preferred = roots.find((root) => root.valid) ?? roots[0];
        if (preferred) {
          setPrismRoot(preferred.path);
          await refreshInstances(preferred.path);
        }
      } catch (error) {
        setStatusLine(String(error));
      }
    };

    void loadRoots();
  }, [refreshInstances]);

  useEffect(() => {
    if (!prismRoot) {
      return;
    }

    void refreshInstances(prismRoot);
  }, [prismRoot, refreshInstances]);

  useEffect(() => {
    void refreshSearch();
  }, [refreshSearch, progress?.assetCount]);

  useEffect(() => {
    const setupListeners = async () => {
      const unlistenProgress = await listen<ScanProgressEvent>(
        "scan://progress",
        (event) => {
          const payload = event.payload;
          if (payload.scanId !== scanId) {
            return;
          }

          setProgress(payload);
          setStatusLine(
            `Skanowanie: ${payload.scannedContainers}/${payload.totalContainers} kontenerów, ${payload.assetCount} assetów`,
          );
        },
      );

      const unlistenChunk = await listen<ScanChunkEvent>("scan://chunk", (event) => {
        const payload = event.payload;
        if (payload.scanId !== scanId) {
          return;
        }

        setStatusLine((current) => {
          if (payload.assets.length === 0) {
            return current;
          }

          return `Dodano ${payload.assets.length} nowych assetów...`;
        });
      });

      const unlistenComplete = await listen<ScanCompletedEvent>(
        "scan://completed",
        (event) => {
          const payload = event.payload;
          if (payload.scanId !== scanId) {
            return;
          }

          setLifecycle(payload.lifecycle);
          setStatusLine(
            payload.lifecycle === "completed"
              ? `Skan zakończony. Zindeksowano ${payload.assetCount} assetów.`
              : `Skan zakończony statusem: ${payload.lifecycle}`,
          );
          void loadTreeChildren(ROOT_NODE_ID);
          void refreshSearch();
        },
      );

      const unlistenError = await listen<{ scanId: string; error: string }>(
        "scan://error",
        (event) => {
          if (event.payload.scanId !== scanId) {
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

    let cleanup: (() => void) | undefined;
    void setupListeners().then((teardown) => {
      cleanup = teardown;
    });

    return () => {
      cleanup?.();
    };
  }, [scanId, loadTreeChildren, refreshSearch]);

  useEffect(() => {
    const loadPreview = async () => {
      if (!scanId || !activeAsset || !activeAsset.isImage) {
        return;
      }

      if (previewCache[activeAsset.assetId]) {
        return;
      }

      try {
        const preview = await invoke<AssetPreviewResponse>("get_asset_preview", {
          scanId,
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
  }, [activeAsset, previewCache, scanId]);

  const startScan = useCallback(async () => {
    if (!prismRoot || !selectedInstance) {
      setStatusLine("Wybierz Prism root oraz instancję.");
      return;
    }

    if (!includeVanilla && !includeMods && !includeResourcepacks) {
      setStatusLine("Wybierz przynajmniej jedno źródło assetów.");
      return;
    }

    setIsStartingScan(true);

    try {
      setSearchResults([]);
      setTreeByNodeId({ [ROOT_NODE_ID]: [] });
      setExpandedNodes(new Set());
      setSelectedFolderId(ROOT_NODE_ID);
      setSelectedAssets(new Set());
      setActiveAssetId(null);
      setPreviewCache({});

      const response = await invoke<{ scanId: string }>("start_scan", {
        req: {
          prismRoot,
          instanceFolder: selectedInstance,
          includeVanilla,
          includeMods,
          includeResourcepacks,
        },
      });

      setScanId(response.scanId);
      setLifecycle("scanning");
      setProgress(null);
      setStatusLine("Skan uruchomiony.");
      await loadTreeChildren(ROOT_NODE_ID);
    } catch (error) {
      setStatusLine(String(error));
    } finally {
      setIsStartingScan(false);
    }
  }, [
    includeMods,
    includeResourcepacks,
    includeVanilla,
    loadTreeChildren,
    prismRoot,
    selectedInstance,
  ]);

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

        return next;
      });

      if (!treeByNodeId[node.id]) {
        await loadTreeChildren(node.id);
      }
    },
    [loadTreeChildren, treeByNodeId],
  );

  const toggleAssetSelection = useCallback((assetId: string) => {
    setSelectedAssets((current) => {
      const next = new Set(current);
      if (next.has(assetId)) {
        next.delete(assetId);
      } else {
        next.add(assetId);
      }
      return next;
    });
  }, []);

  const selectAllVisible = useCallback(() => {
    setSelectedAssets(new Set(visibleAssets.map((asset) => asset.assetId)));
  }, [visibleAssets]);

  const clearSelection = useCallback(() => {
    setSelectedAssets(new Set());
  }, []);

  const saveAssets = useCallback(
    async (assetIds: string[]) => {
      if (!scanId || assetIds.length === 0) {
        return;
      }

      const selectedPath = await open({
        directory: true,
        multiple: false,
      });

      if (!selectedPath || Array.isArray(selectedPath)) {
        return;
      }

      setIsSaving(true);
      try {
        const result = await invoke<{ savedFiles: string[] }>("save_assets", {
          req: {
            scanId,
            assetIds,
            destinationDir: selectedPath,
            audioFormat,
          },
        });

        setStatusLine(`Zapisano ${result.savedFiles.length} plików.`);
      } catch (error) {
        setStatusLine(String(error));
      } finally {
        setIsSaving(false);
      }
    },
    [audioFormat, scanId],
  );

  const copyAssets = useCallback(
    async (assetIds: string[]) => {
      if (!scanId || assetIds.length === 0) {
        return;
      }

      setIsCopying(true);
      try {
        const result = await invoke<{ copiedFiles: string[] }>(
          "copy_assets_to_clipboard",
          {
            req: {
              scanId,
              assetIds,
              audioFormat,
            },
          },
        );

        setStatusLine(`Skopiowano ${result.copiedFiles.length} plików do schowka.`);
      } catch (error) {
        setStatusLine(String(error));
      } finally {
        setIsCopying(false);
      }
    },
    [audioFormat, scanId],
  );

  const renderTree = useCallback(
    (nodeId: string, depth: number): ReactElement[] => {
      const nodes = treeByNodeId[nodeId] ?? [];

      return nodes.flatMap((node) => {
        const isExpanded = expandedNodes.has(node.id);

        const rowStyle: CSSProperties = {
          paddingInlineStart: `${12 + depth * 14}px`,
        };

        const row = (
          <button
            key={node.id}
            type="button"
            className={`tree-row ${selectedFolderId === node.id ? "tree-row-active" : ""}`}
            style={rowStyle}
            onClick={async () => {
              if (node.nodeType === "folder") {
                await toggleFolder(node);
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

  const currentPreview = activeAsset ? previewCache[activeAsset.assetId] : undefined;

  return (
    <div className="app-shell">
      <header className="topbar">
        <div className="topbar-grid">
          <div className="field-group">
            <label className="field-label" htmlFor="prism-root">
              Prism Root
            </label>
            <div className="field-row">
              <select
                id="prism-root"
                className="mae-select"
                value={prismRoot}
                onChange={(event) => setPrismRoot(event.currentTarget.value)}
              >
                {rootCandidates.map((candidate) => (
                  <option key={`${candidate.path}:${candidate.source}`} value={candidate.path}>
                    {candidate.path}
                    {candidate.valid ? "" : " (invalid)"}
                  </option>
                ))}
              </select>
              <input
                className="mae-input"
                value={prismRoot}
                onChange={(event) => setPrismRoot(event.currentTarget.value)}
                placeholder="Wpisz path do PrismLauncher"
              />
            </div>
          </div>

          <div className="field-group">
            <label className="field-label" htmlFor="instance">
              Instance
            </label>
            <div className="field-row">
              <select
                id="instance"
                className="mae-select"
                value={selectedInstance}
                onChange={(event) => setSelectedInstance(event.currentTarget.value)}
              >
                {instances.map((instance) => (
                  <option key={instance.folderName} value={instance.folderName}>
                    {instance.displayName}
                    {instance.minecraftVersion
                      ? ` (MC ${instance.minecraftVersion})`
                      : " (bez wersji)"}
                  </option>
                ))}
              </select>

              <button
                type="button"
                className="mae-button mae-button-accent"
                disabled={isStartingScan}
                onClick={() => {
                  void startScan();
                }}
              >
                {isStartingScan ? "Starting..." : "Scan"}
              </button>
            </div>
          </div>

          <div className="field-group">
            <label className="field-label">Include</label>
            <div className="field-row checkbox-row">
              <label className="mae-checkbox">
                <input
                  checked={includeVanilla}
                  onChange={(event) => setIncludeVanilla(event.currentTarget.checked)}
                  type="checkbox"
                />
                Vanilla
              </label>

              <label className="mae-checkbox">
                <input
                  checked={includeMods}
                  onChange={(event) => setIncludeMods(event.currentTarget.checked)}
                  type="checkbox"
                />
                Mods
              </label>

              <label className="mae-checkbox">
                <input
                  checked={includeResourcepacks}
                  onChange={(event) =>
                    setIncludeResourcepacks(event.currentTarget.checked)
                  }
                  type="checkbox"
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
            placeholder="Szukaj np. star / atm star / allthe star / item star"
          />

          <select
            className="mae-select audio-select"
            value={audioFormat}
            onChange={(event) => setAudioFormat(event.currentTarget.value as AudioFormat)}
          >
            <option value="original">Audio: original</option>
            <option value="mp3">Audio: mp3</option>
            <option value="wav">Audio: wav</option>
          </select>

          <button type="button" className="mae-button" onClick={selectAllVisible}>
            Select visible
          </button>
          <button type="button" className="mae-button" onClick={clearSelection}>
            Clear
          </button>
          <button
            type="button"
            className="mae-button"
            disabled={isCopying || selectedAssetIds.length === 0}
            onClick={() => {
              void copyAssets(selectedAssetIds);
            }}
          >
            {isCopying ? "Copying..." : `Copy selected (${selectedAssetIds.length})`}
          </button>
          <button
            type="button"
            className="mae-button mae-button-accent"
            disabled={isSaving || selectedAssetIds.length === 0}
            onClick={() => {
              void saveAssets(selectedAssetIds);
            }}
          >
            {isSaving ? "Saving..." : `Save selected (${selectedAssetIds.length})`}
          </button>
        </div>
      </header>

      <main className="content-grid">
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
          <div className="panel-title">Assets ({visibleAssets.length})</div>

          <div className="asset-list" ref={listParentRef}>
            <div
              style={{
                height: `${virtualizer.getTotalSize()}px`,
                position: "relative",
                width: "100%",
              }}
            >
              {virtualizer.getVirtualItems().map((virtualRow) => {
                const asset = visibleAssets[virtualRow.index];
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
                    className={`asset-row ${isSelected ? "asset-row-selected" : ""}`}
                    key={asset.assetId}
                    style={rowStyle}
                  >
                    <input
                      type="checkbox"
                      checked={isSelected}
                      onChange={() => toggleAssetSelection(asset.assetId)}
                    />

                    <button
                      type="button"
                      className="asset-main"
                      onClick={() => setActiveAssetId(asset.assetId)}
                    >
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
          </div>
        </section>

        <aside className="preview-panel">
          <div className="panel-title">Preview</div>
          {!activeAsset ? (
            <p className="muted">Wybierz asset, aby zobaczyć podgląd.</p>
          ) : (
            <div className="preview-content">
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
              ) : (
                <div className="preview-fallback">
                  {activeAsset.isImage
                    ? "Ładowanie preview..."
                    : "Preview dostępny tylko dla obrazów"}
                </div>
              )}

              <div className="preview-actions">
                <button
                  type="button"
                  className="mae-button"
                  onClick={(event: MouseEvent<HTMLButtonElement>) => {
                    event.stopPropagation();
                    void copyAssets([activeAsset.assetId]);
                  }}
                >
                  Copy file
                </button>

                <button
                  type="button"
                  className="mae-button mae-button-accent"
                  onClick={(event: MouseEvent<HTMLButtonElement>) => {
                    event.stopPropagation();
                    void saveAssets([activeAsset.assetId]);
                  }}
                >
                  Save file
                </button>
              </div>
            </div>
          )}
        </aside>
      </main>
    </div>
  );
}

function assetFolderNodeId(asset: AssetRecord): string {
  const segments: string[] = [];

  segments.push(sourceRootSegment(asset.sourceType));
  segments.push(asset.sourceName);
  segments.push(asset.namespace);

  const splitPath = asset.relativeAssetPath.split("/");
  if (splitPath.length > 1) {
    for (const part of splitPath.slice(0, -1)) {
      segments.push(part);
    }
  }

  let nodeId = ROOT_NODE_ID;
  for (const segment of segments) {
    const escaped = segment.replace(/\//g, "∕");
    nodeId = `${nodeId}/${escaped}`;
  }

  return nodeId;
}

function sourceRootSegment(sourceType: AssetSourceType): string {
  switch (sourceType) {
    case "vanilla":
      return "vanilla";
    case "mod":
      return "mods";
    case "resourcePack":
      return "resourcepacks";
    default:
      return "unknown";
  }
}

export default App;
