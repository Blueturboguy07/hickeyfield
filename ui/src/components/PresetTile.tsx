import type { PresetFamily } from "../types";
import { presetPreview } from "../mock";
import { isNativeVariant, selectVariant } from "../lib/variants";
import { PencilIcon } from "./Icons";

/**
 * The preset tile.
 *
 * The name is Space Grotesk 700 at -4% tracking, not Bebas: Bebas has a single
 * weight and at 16px it goes visibly light next to the rest of the rail, which
 * is exactly the size this label lives at.
 */
export function PresetTile({
  preset,
  modelName,
  modelId,
  onChange,
}: {
  preset: PresetFamily | null;
  modelName: string;
  modelId: string | null;
  onChange: () => void;
}) {
  const variant = selectVariant(preset, modelId);
  const thumb = preset
    ? (variant?.previewUrl ?? presetPreview(preset.id, 640, 360))
    : presetPreview("general", 640, 360);
  const derived = preset ? !isNativeVariant(preset, modelId) : false;

  return (
    <div className="preset-tile">
      <img className="preset-tile-thumb" src={thumb} alt="" />
      <span className="media-bevel" aria-hidden="true" />
      <button type="button" className="preset-tile-change" onClick={onChange}>
        <PencilIcon size={14} />
        Change
      </button>
      <div className="preset-tile-caption">
        <span className="preset-tile-name">
          {(preset?.displayName ?? "General").toUpperCase()}
        </span>
        <span className="preset-tile-sub">
          {preset?.description && !derived
            ? `${modelName} · ${preset.category}`
            : derived
              ? `${modelName} · derived template`
              : modelName}
        </span>
      </div>
    </div>
  );
}
