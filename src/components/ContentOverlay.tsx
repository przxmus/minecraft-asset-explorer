import type { InstanceInfo } from "../types/assets";

type ContentOverlayProps = {
  needsInstanceSelection: boolean;
  instances: InstanceInfo[];
};

export function ContentOverlay({ needsInstanceSelection, instances }: ContentOverlayProps) {
  return (
    <div className="content-overlay">
      <div className="overlay-card">
        <div className="overlay-title">
          {needsInstanceSelection ? "Choose an instance" : "Scanning assets..."}
        </div>
        <div className="overlay-subtitle">
          {needsInstanceSelection
            ? instances.length === 0
              ? "No instances found. Check your Prism root path."
              : "Select an instance above to start exploring."
            : "The explorer will unlock when the scan completes."}
        </div>
      </div>
    </div>
  );
}
