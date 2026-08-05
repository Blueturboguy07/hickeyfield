import type { Model, Modality } from "../types";
import { Overlay } from "./Overlay";
import { RouteIcon } from "./Icons";

const GROUPS: { modality: Modality; label: string }[] = [
  { modality: "video", label: "Video" },
  { modality: "image", label: "Image" },
  { modality: "audio", label: "Audio" },
  { modality: "3d", label: "3D" },
];

/**
 * Model + route in one sheet.
 *
 * Route is a first-class choice here rather than an advanced setting, because
 * the same model on two providers can differ in price, in rate limits and in
 * what it will refuse to render. Hiding that would reproduce exactly the
 * opacity this project exists to remove.
 */
export function ModelPicker({
  open,
  onClose,
  models,
  selectedModelId,
  selectedRouteId,
  onSelect,
}: {
  open: boolean;
  onClose: () => void;
  models: Model[];
  selectedModelId: string | null;
  selectedRouteId: string | null;
  onSelect: (modelId: string, routeId: string) => void;
}) {
  return (
    <Overlay open={open} onClose={onClose} title="Select model" variant="sheet">
      {GROUPS.map((group) => {
        const items = models.filter((m) => m.modality === group.modality);
        if (items.length === 0) return null;
        return (
          <section className="model-group" key={group.modality}>
            <h3 className="model-group-label">{group.label}</h3>
            <ul className="model-list">
              {items.map((model) => {
                const active = model.id === selectedModelId;
                // Prefer a route that can actually run. Selecting the first
                // route regardless is how a click landed on a provider with no
                // client and failed at submit instead of at the picker.
                const usable = model.routes.find((r) => r.available !== false);
                const runnable = Boolean(usable);
                return (
                  <li
                    key={model.id}
                    className="model-item"
                    data-active={active || undefined}
                    data-unrunnable={!runnable || undefined}
                  >
                    <button
                      type="button"
                      className="model-item-main"
                      disabled={!runnable}
                      onClick={() =>
                        onSelect(model.id, usable?.id ?? model.routes[0]?.id ?? "")
                      }
                      aria-pressed={active}
                    >
                      <span className="model-item-name">
                        {model.displayName}
                        {model.isLaunch ? (
                          <span className="badge">LAUNCH</span>
                        ) : null}
                      </span>
                      <span className="model-item-sub">
                        {/* Never the raw id: that is a slug, not a description. */}
                        {runnable
                          ? (model.subtitle ?? "")
                          : (model.routes[0]?.unavailableReason ??
                            "No usable route")}
                      </span>
                    </button>

                    <div className="model-item-routes" role="group" aria-label={`${model.displayName} routes`}>
                      {model.routes.map((route) => {
                        const ok = route.available !== false;
                        return (
                          <button
                            key={route.id}
                            type="button"
                            className="chip chip-button chip-route"
                            data-unavailable={!ok || undefined}
                            disabled={!ok}
                            // The reason is the whole point of showing a route
                            // we cannot use rather than hiding it.
                            title={ok ? route.note : route.unavailableReason}
                            aria-pressed={active && route.id === selectedRouteId}
                            onClick={() => onSelect(model.id, route.id)}
                          >
                            <RouteIcon size={12} />
                            {route.provider}
                          </button>
                        );
                      })}
                    </div>
                  </li>
                );
              })}
            </ul>
          </section>
        );
      })}
    </Overlay>
  );
}
