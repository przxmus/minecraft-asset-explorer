import type { ReactElement, RefObject } from "react";
import type { AssetPreviewResponse, AssetRecord } from "../types/assets";

type PreviewPanelProps = {
  activeAsset: AssetRecord | null;
  currentPreview: AssetPreviewResponse | undefined;
  activeAssetIsJson: boolean;
  highlightedJson: ReactElement[] | null;
  previewPanelRef: RefObject<HTMLElement | null>;
  previewContentRef: RefObject<HTMLDivElement | null>;
  onCopyActiveAsset: (assetId: string) => void;
  onSaveActiveAsset: (assetId: string) => void;
};

export function PreviewPanel({
  activeAsset,
  currentPreview,
  activeAssetIsJson,
  highlightedJson,
  previewPanelRef,
  previewContentRef,
  onCopyActiveAsset,
  onSaveActiveAsset,
}: PreviewPanelProps) {
  return (
    <aside className="preview-panel" ref={previewPanelRef}>
      <div className="panel-header">
        <span className="panel-title">Preview</span>
      </div>

      {!activeAsset ? (
        <div className="preview-empty">Select an asset to preview</div>
      ) : (
        <div className="preview-content" ref={previewContentRef}>
          <div className="preview-key">{activeAsset.key}</div>
          <div className="preview-meta">
            <span className="preview-tag">{activeAsset.sourceType}</span>
            <span className="preview-tag">{activeAsset.containerType}</span>
            <span className="preview-tag">.{activeAsset.extension || "?"}</span>
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

          {activeAssetIsJson && highlightedJson ? <pre className="json-preview">{highlightedJson}</pre> : null}

          {!currentPreview && (activeAsset.isImage || activeAsset.isAudio || activeAssetIsJson) ? (
            <div className="preview-fallback">Loading preview...</div>
          ) : null}

          {!activeAsset.isImage && !activeAsset.isAudio && !activeAssetIsJson ? (
            <div className="preview-fallback">Preview available for images, audio, and JSON.</div>
          ) : null}

          <div className="preview-actions">
            <button
              type="button"
              className="mae-button"
              onClick={() => {
                onCopyActiveAsset(activeAsset.assetId);
              }}
            >
              Copy file
            </button>
            <button
              type="button"
              className="mae-button mae-button-accent"
              onClick={() => {
                onSaveActiveAsset(activeAsset.assetId);
              }}
            >
              Save file
            </button>
          </div>
        </div>
      )}
    </aside>
  );
}
