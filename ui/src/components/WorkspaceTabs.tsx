import type { UseCase, WorkspaceTab } from "../types";

/**
 * Roving-tabindex tablist: only the selected tab is in the tab order, and the
 * arrow keys move between them. Three adjacent buttons that all take a Tab
 * stop is the most common way a segmented control fails a keyboard pass.
 */
export function WorkspaceTabs({
  value,
  onChange,
  panelId,
  useCases,
}: {
  value: WorkspaceTab;
  onChange: (tab: WorkspaceTab) => void;
  panelId: string;
  /** Comes from Rust, so a tab can never exist that no model can serve. */
  useCases: UseCase[];
}) {
  const TABS = useCases.map((u) => ({ id: u.slug, label: u.label, blurb: u.blurb }));
  const index = TABS.findIndex((t) => t.id === value);
  if (TABS.length === 0) return null;

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    const delta = e.key === "ArrowRight" ? 1 : e.key === "ArrowLeft" ? -1 : 0;
    if (delta === 0) return;
    e.preventDefault();
    const next = TABS[(index + delta + TABS.length) % TABS.length];
    onChange(next.id);
    const el = e.currentTarget.querySelector<HTMLButtonElement>(
      `[data-tab="${next.id}"]`,
    );
    el?.focus();
  };

  return (
    <div className="tabs" role="tablist" aria-label="Workspace" onKeyDown={onKeyDown}>
      {TABS.map((tab) => (
        <button
          key={tab.id}
          type="button"
          role="tab"
          data-tab={tab.id}
          className="tabs-tab"
          aria-selected={tab.id === value}
          aria-controls={panelId}
          tabIndex={tab.id === value ? 0 : -1}
          onClick={() => onChange(tab.id)}
          title={tab.blurb}
        >
          {tab.label}
        </button>
      ))}
    </div>
  );
}
