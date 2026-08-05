import type { Model, Route } from "../types";
import { routeLabel } from "../lib/variants";
import { ChevronRightIcon } from "./Icons";

/** The row that opens the model sheet. Shows the resolved route underneath —
 * the same model on two providers can differ in price and in what it allows,
 * so the route is not an implementation detail the user should have to dig
 * for. */
export function ModelRow({
  model,
  route,
  onOpen,
}: {
  model: Model | null;
  route: Route | null;
  onOpen: () => void;
}) {
  return (
    <button type="button" className="model-row" onClick={onOpen}>
      <span className="model-row-text">
        <span className="model-row-label">Model</span>
        <span className="model-row-name">
          {model?.displayName ?? "Select a model"}
        </span>
        <span className="model-row-route">{routeLabel(route)}</span>
      </span>
      <ChevronRightIcon size={18} />
    </button>
  );
}
