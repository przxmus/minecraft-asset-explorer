import type { ReactElement } from "react";

type TreePanelProps = {
  selectedFolderId: string;
  rootNodeId: string;
  renderedTree: ReactElement[];
  onSelectRootFolder: () => void;
};

export function TreePanel({ selectedFolderId, rootNodeId, renderedTree, onSelectRootFolder }: TreePanelProps) {
  return (
    <aside className="tree-panel">
      <div className="panel-header">
        <span className="panel-title">Explorer</span>
      </div>
      <button
        type="button"
        className={`tree-row ${selectedFolderId === rootNodeId ? "tree-row-active" : ""}`}
        style={{ paddingInlineStart: "14px" }}
        onClick={onSelectRootFolder}
      >
        <span className="tree-icon">&#x25BE;</span>
        <span>All assets</span>
      </button>
      <div className="tree-scroll">{renderedTree}</div>
    </aside>
  );
}
