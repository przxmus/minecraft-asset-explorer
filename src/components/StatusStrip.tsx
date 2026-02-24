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
    ? `${progressPercent}% · ${exportProgress.processedCount}/${exportProgress.requestedCount} files · ${exportProgress.successCount} ok · ${exportProgress.failedCount} failed`
    : progress
      ? `${progressPercent}% · ${progress.scannedContainers}/${progress.totalContainers} containers · ${progress.assetCount} assets`
      : lifecycle === "scanning"
        ? "0% · 0/0 containers · 0 assets"
        : null;
  const showProgress = lifecycle === "scanning" || progressLabel !== null;

  return (
    <div className="status-strip">
      <span className="status-strip__lifecycle">
        <span className={`status-dot ${lifecycleDotClass}`} />
        {lifecycle}
      </span>

      {showProgress ? (
        <>
          <div className="progress-bar">
            <div className="progress-bar__fill" style={{ width: `${progressPercent}%` }} />
          </div>
          <span className="status-strip__progress">{progressLabel ?? "0% · 0/0 containers · 0 assets"}</span>
        </>
      ) : null}

      <span className="status-strip__message">{statusLine}</span>
    </div>
  );
}
