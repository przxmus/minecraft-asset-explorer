import type { ExportProgressEvent, ScanLifecycle, ScanProgressEvent } from "../types/assets";

type StatusStripProps = {
  lifecycle: ScanLifecycle | "idle";
  lifecycleDotClass: string;
  progress: ScanProgressEvent | null;
  exportProgress: ExportProgressEvent | null;
  progressPercent: number;
  statusLine: string;
};

export function StatusStrip({
  lifecycle,
  lifecycleDotClass,
  progress,
  exportProgress,
  progressPercent,
  statusLine,
}: StatusStripProps) {
  const progressLabel = exportProgress
    ? `${exportProgress.processedCount}/${exportProgress.requestedCount} files · ${exportProgress.successCount} ok · ${exportProgress.failedCount} failed`
    : progress
      ? `${progress.scannedContainers}/${progress.totalContainers} containers · ${progress.assetCount} assets`
      : null;

  return (
    <div className="status-strip">
      <span className="status-strip__lifecycle">
        <span className={`status-dot ${lifecycleDotClass}`} />
        {lifecycle}
      </span>

      {progressLabel ? (
        <>
          <div className="progress-bar">
            <div className="progress-bar__fill" style={{ width: `${progressPercent}%` }} />
          </div>
          <span className="status-strip__progress">{progressLabel}</span>
        </>
      ) : null}

      <span className="status-strip__message">{statusLine}</span>
    </div>
  );
}
