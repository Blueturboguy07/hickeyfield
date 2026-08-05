import { useMemo, useState } from "react";
import type { MediaRole } from "../types";
import {
  blockReason,
  categoryChips,
  categoryLabel,
  filterBrowseModels,
  formatBrowsePrice,
  groupByCategory,
  hasBrowsePrice,
  inCategoryOrder,
  ioSummary,
  pageBrowseModels,
  priceUnit,
  requirementNote,
  roleList,
  type BrowseModel,
} from "../lib/browser";
import { Overlay } from "./Overlay";
import { EmptyState } from "./EmptyState";
import { RouteIcon, SearchIcon } from "./Icons";

/** Rows per paint. The full fal catalogue is 1418 models. */
const PAGE = 40;

/**
 * One catalogue row: what it takes, what it produces, what it costs.
 *
 * The price sits in its own column rather than inline with the description
 * because it is the number the user is comparing across rows, and a number
 * that moves horizontally row to row cannot be scanned. It is never "Free" and
 * never "$0.00" — see `formatBrowsePrice`.
 */
function BrowseRow({
  model,
  attachedRoles,
  active,
  onPick,
}: {
  model: BrowseModel;
  attachedRoles: MediaRole[];
  active: boolean;
  onPick: () => void;
}) {
  const reason = blockReason(model, attachedRoles);
  const { takes, produces } = ioSummary(model);
  const needs = requirementNote(model);
  const price = formatBrowsePrice(model.price);
  const unit = priceUnit(model.price);
  const known = hasBrowsePrice(model.price);

  return (
    <li
      className="browse-item"
      data-active={active || undefined}
      data-blocked={reason ? true : undefined}
    >
      <button
        type="button"
        className="browse-item-main"
        disabled={Boolean(reason)}
        aria-pressed={active}
        onClick={onPick}
      >
        <span className="browse-item-head">
          <span className="browse-item-title">{model.title}</span>
          <span className="browse-item-provider">
            <RouteIcon size={11} />
            {model.provider}
          </span>
        </span>

        {/* The plain statement of capability. This line is the whole surface:
         * it is what was missing when a video was attached to a model that
         * takes nothing but a prompt. */}
        <span className="browse-item-io">
          <span className="browse-item-takes">{takes}</span>
          <span className="browse-item-arrow" aria-hidden="true">
            →
          </span>
          <span className="browse-item-produces">{produces}</span>
        </span>

        {model.description ? (
          <span className="browse-item-desc">{model.description}</span>
        ) : null}

        {needs ? <span className="browse-item-needs">{needs}</span> : null}

        {reason ? (
          <span className="browse-item-reason" role="note">
            {reason}
          </span>
        ) : null}
      </button>

      <span className="browse-item-price" title={model.price.note ?? undefined}>
        <span
          className="browse-item-price-usd"
          data-unknown={known ? undefined : true}
        >
          {price}
        </span>
        {unit ? <span className="browse-item-price-unit">{unit}</span> : null}
      </span>
    </li>
  );
}

/**
 * Browse the catalogue by what a model can actually do.
 *
 * Organised by capability category — Video to Video, Image to Image, Text to
 * Video — because that is the question a user arrives with ("what can edit my
 * clip?"), not by vendor or by name, which are the answers to questions they
 * only have once they already know the catalogue.
 *
 * Two rules this surface holds to:
 *
 * - **Nothing is hidden by default.** A model that cannot run stays in the
 *   list, greyed, with its reason spelled out. The compatibility filter is
 *   opt-in: hiding the wrong models on the user's behalf answers their
 *   question by making the answer invisible, which is the same silence that
 *   caused the bug in a different costume.
 * - **No progress language.** There is no ETA, no percentage and no "fast"
 *   claim anywhere here — see `lib/status.ts`.
 */
export function ModelBrowser({
  open,
  onClose,
  models,
  attachedRoles = [],
  selectedModelId,
  onSelect,
}: {
  open: boolean;
  onClose: () => void;
  /**
   * Required, and never defaulted to the mock: substituting invented rows on a
   * failed bridge call is how the app once rendered fabricated results.
   */
  models: BrowseModel[];
  /** Roles the user has already attached, from `MediaRef[]`. */
  attachedRoles?: MediaRole[];
  selectedModelId: string | null;
  onSelect: (modelId: string) => void;
}) {
  const [query, setQuery] = useState("");
  const [category, setCategory] = useState<string | null>(null);
  const [onlyCompatible, setOnlyCompatible] = useState(false);
  const [shown, setShown] = useState(PAGE);

  const hasAttachment = attachedRoles.length > 0;

  // Chips are counted against everything *except* the category filter, so a
  // chip's count is what clicking it will actually show. Counting the fully
  // filtered set would light every chip at zero but the selected one.
  const beforeCategory = useMemo(
    () =>
      filterBrowseModels(models, {
        query,
        attachedRoles,
        onlyCompatible,
      }),
    [models, query, attachedRoles, onlyCompatible],
  );
  const chips = useMemo(() => categoryChips(beforeCategory), [beforeCategory]);

  const filtered = useMemo(
    () =>
      filterBrowseModels(beforeCategory, {
        category,
      }),
    [beforeCategory, category],
  );

  // Ordered before paging so "Load more" only ever appends; see
  // `inCategoryOrder`.
  const ordered = useMemo(() => inCategoryOrder(filtered), [filtered]);
  const { page, remaining } = pageBrowseModels(ordered, shown);
  const groups = useMemo(() => groupByCategory(page), [page]);

  const reset = (fn: () => void) => {
    fn();
    setShown(PAGE);
  };

  const clearAll = () =>
    reset(() => {
      setQuery("");
      setCategory(null);
      setOnlyCompatible(false);
    });

  return (
    <Overlay
      open={open}
      onClose={onClose}
      title="Models"
      variant="full"
      header={
        <div className="overlay-search">
          <SearchIcon size={16} />
          <label className="sr-only" htmlFor="model-browser-search">
            Search models by name or description
          </label>
          <input
            id="model-browser-search"
            type="search"
            placeholder="Search models"
            value={query}
            onChange={(e) => reset(() => setQuery(e.currentTarget.value))}
          />
        </div>
      }
    >
      <div className="picker-filters browse-filters">
        <div className="chip-filters" role="group" aria-label="Capability">
          <button
            type="button"
            className="chip chip-button"
            aria-pressed={category === null}
            onClick={() => reset(() => setCategory(null))}
          >
            Everything
            <span className="browse-chip-count">{beforeCategory.length}</span>
          </button>
          {chips.map((chip) => (
            <button
              key={chip.category}
              type="button"
              className="chip chip-button"
              aria-pressed={category === chip.category}
              onClick={() => reset(() => setCategory(chip.category))}
            >
              {chip.label}
              <span className="browse-chip-count">{chip.count}</span>
            </button>
          ))}
        </div>

        <div className="browse-controls">
          <button
            type="button"
            className="chip chip-button browse-compat"
            aria-pressed={onlyCompatible}
            disabled={!hasAttachment}
            // Disabled controls announce nothing about why, and "why" is the
            // entire job of this surface.
            title={
              hasAttachment
                ? `Keep only models that accept the attached ${roleList(attachedRoles)}`
                : "Attach a file first — there is nothing to match against"
            }
            onClick={() => reset(() => setOnlyCompatible((v) => !v))}
          >
            {hasAttachment
              ? `Works with my ${roleList(attachedRoles)}`
              : "Works with what I've attached"}
          </button>

          <p className="browse-count">
            {filtered.length} of {models.length}
            {category ? ` in ${categoryLabel(category)}` : ""}
          </p>
        </div>
      </div>

      {page.length === 0 ? (
        <EmptyState
          heading="No models match"
          explanation={
            onlyCompatible && hasAttachment
              ? `Nothing in the catalogue accepts the attached ${roleList(attachedRoles)} and also matches these filters.`
              : "Nothing in the catalogue matches these filters."
          }
          action={
            <button type="button" className="btn btn-primary" onClick={clearAll}>
              Clear filters
            </button>
          }
        />
      ) : (
        <>
          {groups.map((group) => (
            <section className="browse-group" key={group.category}>
              <h3 className="browse-group-label">
                {group.label}
                <span className="browse-group-count">
                  {group.models.length}
                </span>
              </h3>
              <ul className="browse-list">
                {group.models.map((model) => (
                  <BrowseRow
                    key={model.id}
                    model={model}
                    attachedRoles={attachedRoles}
                    active={model.id === selectedModelId}
                    onPick={() => {
                      onSelect(model.id);
                      onClose();
                    }}
                  />
                ))}
              </ul>
            </section>
          ))}

          {remaining > 0 ? (
            <div className="picker-more">
              <button
                type="button"
                className="btn btn-outline"
                onClick={() => setShown((s) => s + PAGE)}
              >
                Load more ({remaining})
              </button>
            </div>
          ) : null}
        </>
      )}
    </Overlay>
  );
}
