import type { CSSProperties, KeyboardEvent, MouseEvent, RefObject } from "react";
import type { VirtualItem } from "@tanstack/react-virtual";
import type { AssetRecord, SelectionModifiers } from "../types/assets";

type AssetListPanelProps = {
  assets: AssetRecord[];
  searchTotal: number;
  isSearchLoading: boolean;
  hasMoreSearch: boolean;
  isExplorerLocked: boolean;
  selectedAssets: Set<string>;
  virtualTotalSize: number;
  virtualItems: VirtualItem[];
  listParentRef: RefObject<HTMLDivElement | null>;
  onListScroll: () => void;
  onApplySelection: (asset: AssetRecord, modifiers: SelectionModifiers) => void;
  onCopyAsset: (assetId: string) => void;
  onSaveAsset: (assetId: string) => void;
  onLoadMore: () => void;
};

export function AssetListPanel({
  assets,
  searchTotal,
  isSearchLoading,
  hasMoreSearch,
  isExplorerLocked,
  selectedAssets,
  virtualTotalSize,
  virtualItems,
  listParentRef,
  onListScroll,
  onApplySelection,
  onCopyAsset,
  onSaveAsset,
  onLoadMore,
}: AssetListPanelProps) {
  function buildSelectionModifiers(event: MouseEvent | KeyboardEvent): SelectionModifiers {
    return {
      shiftKey: event.shiftKey,
      metaKey: event.metaKey,
      ctrlKey: event.ctrlKey,
    };
  }

  return (
    <section className="list-panel">
      <div className="panel-header">
        <span className="panel-title">Assets</span>
        <span className="panel-subtitle">
          {assets.length}/{searchTotal}
          {isSearchLoading ? " loading..." : ""}
        </span>
      </div>

      <div className="asset-list" ref={listParentRef} onScroll={onListScroll}>
        <div
          style={{
            height: `${virtualTotalSize}px`,
            position: "relative",
            width: "100%",
          }}
        >
          {virtualItems.map((virtualRow) => {
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
                onClick={(event) => {
                  onApplySelection(asset, buildSelectionModifiers(event));
                }}
                onKeyDown={(event) => {
                  if (event.key !== "Enter" && event.key !== " ") {
                    return;
                  }

                  event.preventDefault();
                  onApplySelection(asset, buildSelectionModifiers(event));
                }}
              >
                <input
                  type="checkbox"
                  readOnly
                  checked={isSelected}
                  onClick={(event) => {
                    event.stopPropagation();
                    onApplySelection(asset, buildSelectionModifiers(event));
                  }}
                />

                <button type="button" className="asset-main">
                  <span className="asset-title">{asset.key}</span>
                  <span className="asset-subtitle">
                    {asset.sourceName} / {asset.namespace} / {asset.relativeAssetPath}
                  </span>
                </button>

                <div className="asset-row-actions">
                  <button
                    type="button"
                    className="mae-button mae-button-sm"
                    onClick={(event) => {
                      event.stopPropagation();
                      onCopyAsset(asset.assetId);
                    }}
                  >
                    Copy
                  </button>
                  <button
                    type="button"
                    className="mae-button mae-button-sm"
                    onClick={(event) => {
                      event.stopPropagation();
                      onSaveAsset(asset.assetId);
                    }}
                  >
                    Save
                  </button>
                </div>
              </div>
            );
          })}
        </div>

        {hasMoreSearch ? (
          <div className="load-more-wrap">
            <button type="button" className="mae-button" disabled={isExplorerLocked || isSearchLoading} onClick={onLoadMore}>
              {isSearchLoading ? "Loading..." : "Load more"}
            </button>
          </div>
        ) : null}
      </div>
    </section>
  );
}
