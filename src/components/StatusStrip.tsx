import type { ScanLifecycle, ScanProgressEvent } from "../types/assets";

type StatusStripProps = {
  lifecycle: ScanLifecycle | "idle";
  lifecycleDotClass: string;
  progress: ScanProgressEvent | null;
  progressPercent: number;
  statusLine: string;
};

export function StatusStrip({
  lifecycle,
  lifecycleDotClass,
  progress,
  progressPercent,
  statusLine,
}: StatusStripProps) {
  return (
    <div className="status-strip">
      <span className="status-strip__lifecycle">
        <span className={`status-dot ${lifecycleDotClass}`} />
        {lifecycle}
      </span>

      {progress ? (
        <>
          <div className="progress-bar">
            <div className="progress-bar__fill" style={{ width: `${progressPercent}%` }} />
          </div>
          <span className="status-strip__progress">
            {progress.scannedContainers}/{progress.totalContainers} containers &middot; {progress.assetCount} assets
          </span>
        </>
      ) : null}

      <span className="status-strip__message">{statusLine}</span>
    </div>
  );
}
