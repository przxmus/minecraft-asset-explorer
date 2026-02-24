import type { InstanceInfo, ScanProgressEvent } from "../types/assets";

type ContentOverlayProps = {
  needsInstanceSelection: boolean;
  instances: InstanceInfo[];
  progress: ScanProgressEvent | null;
  progressPercent: number;
  phaseLabel: string;
  statusLine: string;
};

export function ContentOverlay({
  needsInstanceSelection,
  instances,
  progress,
  progressPercent,
  phaseLabel,
  statusLine,
}: ContentOverlayProps) {
  const progressText = progress
    ? `${progress.scannedContainers}/${progress.totalContainers} containers · ${progress.assetCount} assets`
    : "0/0 containers · 0 assets";

  return (
    <div className="content-overlay">
      <div className="overlay-card">
        <div className="overlay-title">
          {needsInstanceSelection ? "Choose an instance" : `Scanning assets... ${progressPercent}%`}
        </div>
        <div className="overlay-subtitle">
          {needsInstanceSelection
            ? instances.length === 0
              ? "No instances found. Check your Prism root path."
              : "Select an instance above to start exploring."
            : `${phaseLabel} · ${progressText}`}
        </div>
        {!needsInstanceSelection ? <div className="overlay-details">{statusLine}</div> : null}
      </div>
    </div>
  );
}
