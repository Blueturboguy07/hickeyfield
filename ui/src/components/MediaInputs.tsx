import { useState } from "react";
import type { MediaRef, MediaRole } from "../types";
import { acceptFor, previewOf, sourceKey } from "../lib/media-input";
import { ImageIcon, TrashIcon, UploadIcon } from "./Icons";

export interface FrameSlot {
  role: MediaRole;
  label: string;
  /** The job cannot run without this one. */
  required?: boolean;
}

/**
 * The two square keyframe slots.
 *
 * Each is a real <input type="file"> hidden with .sr-only and driven by its
 * own <label>, rather than a button that clicks a ref. Same click behaviour,
 * but it keeps the control keyboard-reachable and announced as a file input,
 * which a div-plus-onClick never is.
 */
export function FrameSlots({
  slots,
  media,
  onAdd,
  onRemove,
}: {
  slots: FrameSlot[];
  media: MediaRef[];
  onAdd: (role: MediaRole, files: FileList | null) => void | Promise<void>;
  onRemove: (role: MediaRole) => void;
}) {
  return (
    <div className="frame-slots">
      {slots.map((slot) => {
        const filled = media.find((m) => m.role === slot.role);
        const inputId = `frame-${slot.role}`;
        return (
          <div className="frame-slot" key={slot.role}>
            <input
              id={inputId}
              type="file"
              className="sr-only file-input"
              accept={acceptFor(slot.role)}
              onChange={(e) => void onAdd(slot.role, e.currentTarget.files)}
            />
            <label className="frame-slot-target" htmlFor={inputId}>
              {filled && previewOf(filled) ? (
                <img className="frame-slot-thumb" src={previewOf(filled)} alt="" />
              ) : filled ? (
                // A dialog-picked file has a real path but nothing the webview
                // can load, so name it rather than showing a broken image.
                <span className="frame-slot-name" title={filled.name}>
                  {filled.name}
                </span>
              ) : (
                <span className="frame-slot-icon" aria-hidden="true">
                  <ImageIcon size={18} />
                </span>
              )}
              <span className="frame-slot-label">
                {slot.label}
                {slot.required && !filled ? (
                  // Marked on the empty slot only: once filled, the asterisk
                  // is noise. Without it, "Edit Video" with no clip silently
                  // becomes text-to-video at the same price.
                  <span className="frame-slot-required" aria-label="required">
                    {" *"}
                  </span>
                ) : null}
              </span>
              <span className="media-bevel" aria-hidden="true" />
            </label>
            {filled ? (
              <button
                type="button"
                className="btn btn-icon btn-scrim frame-slot-clear"
                onClick={() => onRemove(slot.role)}
                aria-label={`Remove ${slot.label}`}
              >
                <TrashIcon size={14} />
              </button>
            ) : null}
          </div>
        );
      })}
    </div>
  );
}

/** Reference uploads. Drag-and-drop with a click fallback on the same label. */
export function Dropzone({
  media,
  onAdd,
  onRemove,
}: {
  media: MediaRef[];
  onAdd: (role: MediaRole, files: FileList | null) => void | Promise<void>;
  onRemove: (key: string) => void;
}) {
  const [dragging, setDragging] = useState(false);
  const refs = media.filter((m) => m.role === "reference");

  return (
    <div className="dropzone-wrap">
      <input
        id="dropzone-input"
        type="file"
        multiple
        className="sr-only file-input"
        accept={acceptFor("reference")}
        onChange={(e) => void onAdd("reference", e.currentTarget.files)}
      />
      <label
        className="dropzone"
        htmlFor="dropzone-input"
        data-dragging={dragging || undefined}
        onDragOver={(e) => {
          e.preventDefault();
          setDragging(true);
        }}
        onDragLeave={() => setDragging(false)}
        onDrop={(e) => {
          e.preventDefault();
          setDragging(false);
          void onAdd("reference", e.dataTransfer.files);
        }}
      >
        <UploadIcon size={18} />
        <span className="dropzone-title">Drop files here or click to browse</span>
        <span className="dropzone-hint">PNG, JPG, MP4, MOV — up to 100 MB</span>
      </label>

      {refs.length > 0 ? (
        <ul className="ref-strip">
          {refs.map((ref) => (
            <li className="ref-strip-item" key={sourceKey(ref)}>
              {previewOf(ref) ? (
                <img src={previewOf(ref)} alt={ref.name ?? "Reference"} />
              ) : (
                <span className="ref-strip-name" title={ref.name}>
                  {ref.name}
                </span>
              )}
              <button
                type="button"
                className="btn btn-icon btn-scrim ref-strip-clear"
                onClick={() => onRemove(sourceKey(ref))}
                aria-label={`Remove ${ref.name ?? "reference"}`}
              >
                <TrashIcon size={12} />
              </button>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
