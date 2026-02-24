import type { KeyboardEvent } from "react";
import type { AudioFormat, InstanceInfo } from "../types/assets";

type TopBarProps = {
  prismRootInput: string;
  onPrismRootInputChange: (value: string) => void;
  onCommitPrismRoot: () => void;
  instances: InstanceInfo[];
  selectedInstance: string;
  onSelectedInstanceChange: (value: string) => void;
  includeVanilla: boolean;
  onIncludeVanillaChange: (value: boolean) => void;
  includeMods: boolean;
  onIncludeModsChange: (value: boolean) => void;
  includeResourcepacks: boolean;
  onIncludeResourcepacksChange: (value: boolean) => void;
  query: string;
  onQueryChange: (value: string) => void;
  filterImages: boolean;
  onFilterImagesChange: (value: boolean) => void;
  filterAudio: boolean;
  onFilterAudioChange: (value: boolean) => void;
  filterOther: boolean;
  onFilterOtherChange: (value: boolean) => void;
  audioFormat: AudioFormat;
  onAudioFormatChange: (value: AudioFormat) => void;
  isExplorerLocked: boolean;
  selectedAssetCount: number;
  isCopying: boolean;
  isSaving: boolean;
  isExportRunning: boolean;
  isScanInProgress: boolean;
  onSelectAllVisible: () => void;
  onClearSelection: () => void;
  onCopySelection: () => void;
  onSaveSelection: () => void;
  onCancelExport: () => void;
  onRescanNow: () => void;
};

export function TopBar({
  prismRootInput,
  onPrismRootInputChange,
  onCommitPrismRoot,
  instances,
  selectedInstance,
  onSelectedInstanceChange,
  includeVanilla,
  onIncludeVanillaChange,
  includeMods,
  onIncludeModsChange,
  includeResourcepacks,
  onIncludeResourcepacksChange,
  query,
  onQueryChange,
  filterImages,
  onFilterImagesChange,
  filterAudio,
  onFilterAudioChange,
  filterOther,
  onFilterOtherChange,
  audioFormat,
  onAudioFormatChange,
  isExplorerLocked,
  selectedAssetCount,
  isCopying,
  isSaving,
  isExportRunning,
  isScanInProgress,
  onSelectAllVisible,
  onClearSelection,
  onCopySelection,
  onSaveSelection,
  onCancelExport,
  onRescanNow,
}: TopBarProps) {
  function handleRootInputKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key === "Enter") {
      onCommitPrismRoot();
    }
  }

  return (
    <>
      <div className="topbar-config">
        <div className="field-group field-group--root">
          <label className="field-label" htmlFor="prism-root-input">
            Prism Root
          </label>
          <input
            id="prism-root-input"
            className="mae-input"
            placeholder="/path/to/PrismLauncher"
            value={prismRootInput}
            onChange={(event) => onPrismRootInputChange(event.currentTarget.value)}
            onBlur={onCommitPrismRoot}
            onKeyDown={handleRootInputKeyDown}
          />
        </div>

        <div className="field-group field-group--instance">
          <label className="field-label" htmlFor="instance-select">
            Instance
          </label>
          <select
            id="instance-select"
            className="mae-select"
            value={selectedInstance}
            onChange={(event) => onSelectedInstanceChange(event.currentTarget.value)}
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

        <div className="field-group field-group--sources">
          <div className="field-label">Sources</div>
          <div className="source-checks">
            <label className="source-check">
              <input
                type="checkbox"
                checked={includeVanilla}
                onChange={(event) => onIncludeVanillaChange(event.currentTarget.checked)}
              />
              Vanilla
            </label>
            <label className="source-check">
              <input
                type="checkbox"
                checked={includeMods}
                onChange={(event) => onIncludeModsChange(event.currentTarget.checked)}
              />
              Mods
            </label>
            <label className="source-check">
              <input
                type="checkbox"
                checked={includeResourcepacks}
                onChange={(event) => onIncludeResourcepacksChange(event.currentTarget.checked)}
              />
              Packs
            </label>
          </div>
        </div>
      </div>

      <div className="toolbar">
        <input
          className="mae-search toolbar-search"
          value={query}
          onChange={(event) => onQueryChange(event.currentTarget.value)}
          placeholder="Search assets..."
          disabled={isExplorerLocked}
        />

        <div className="toolbar-divider" />

        <div className="toolbar-filters">
          <label className="filter-pill">
            <input
              type="checkbox"
              checked={filterImages}
              onChange={(event) => onFilterImagesChange(event.currentTarget.checked)}
              disabled={isExplorerLocked}
            />
            Images
          </label>
          <label className="filter-pill">
            <input
              type="checkbox"
              checked={filterAudio}
              onChange={(event) => onFilterAudioChange(event.currentTarget.checked)}
              disabled={isExplorerLocked}
            />
            Audio
          </label>
          <label className="filter-pill">
            <input
              type="checkbox"
              checked={filterOther}
              onChange={(event) => onFilterOtherChange(event.currentTarget.checked)}
              disabled={isExplorerLocked}
            />
            Other
          </label>

          <select
            className="mae-select audio-select"
            value={audioFormat}
            onChange={(event) => onAudioFormatChange(event.currentTarget.value as AudioFormat)}
            disabled={isExplorerLocked}
          >
            <option value="original">Original</option>
            <option value="mp3">MP3</option>
            <option value="wav">WAV</option>
          </select>
        </div>

        <div className="toolbar-divider" />

        <div className="toolbar-actions">
          <button type="button" className="mae-button mae-button-sm" onClick={onSelectAllVisible} disabled={isExplorerLocked}>
            Select all
          </button>
          <button
            type="button"
            className="mae-button mae-button-sm"
            onClick={onClearSelection}
            disabled={isExplorerLocked || selectedAssetCount === 0}
          >
            Clear
          </button>

          <div className="toolbar-divider" />

          <button
            type="button"
            className="mae-button"
            disabled={isExplorerLocked || isCopying || isExportRunning || selectedAssetCount === 0}
            onClick={onCopySelection}
          >
            {isCopying ? "Copying..." : `Copy (${selectedAssetCount})`}
          </button>
          <button
            type="button"
            className="mae-button mae-button-accent"
            disabled={isExplorerLocked || isSaving || isExportRunning || selectedAssetCount === 0}
            onClick={onSaveSelection}
          >
            {isSaving ? "Saving..." : `Save (${selectedAssetCount})`}
          </button>
          <button
            type="button"
            className="mae-button"
            disabled={!selectedInstance || isScanInProgress}
            onClick={onRescanNow}
          >
            {isScanInProgress ? "Scanning..." : "Rescan now"}
          </button>
          {isExportRunning ? (
            <button type="button" className="mae-button" onClick={onCancelExport}>
              Cancel export
            </button>
          ) : null}
        </div>
      </div>
    </>
  );
}
