import { useState } from "react";
import { Overlay } from "./Overlay";

/**
 * The confirm step for deleting a generation.
 *
 * A modal rather than a red button, deliberately. Our accent *is* red, so
 * colour cannot carry destructive weight the way it does in a product with a
 * blue or green brand — the brand rules put the whole burden here instead, and
 * make this the only place a destructive action can be committed.
 *
 * The two outcomes are genuinely different and are offered separately:
 * removing the row is cheap and reversible-ish (the file survives), while
 * deleting the file destroys something the user paid a provider to make.
 */
export function ConfirmDelete({
  open,
  hasFiles,
  onCancel,
  onConfirm,
}: {
  open: boolean;
  /** Whether any output has been saved to the library yet. */
  hasFiles: boolean;
  onCancel: () => void;
  onConfirm: (deleteFiles: boolean) => void;
}) {
  const [alsoFiles, setAlsoFiles] = useState(false);

  return (
    <Overlay open={open} onClose={onCancel} title="Delete generation" variant="sheet">
      <div className="confirm">
        <p className="confirm-body">
          This removes the generation from your history.
          {hasFiles
            ? " The file stays in your library unless you also delete it below."
            : " Nothing has been saved to your library yet."}
        </p>

        {hasFiles ? (
          <label className="confirm-check">
            <input
              type="checkbox"
              checked={alsoFiles}
              onChange={(e) => setAlsoFiles(e.currentTarget.checked)}
            />
            <span>
              Also delete the file from disk
              {/* Say the cost out loud. This is the irreversible half. */}
              <span className="confirm-warn">
                {" "}
                — this cannot be undone, and regenerating it costs money again.
              </span>
            </span>
          </label>
        ) : null}

        <div className="confirm-actions">
          <button type="button" className="btn btn-outline" onClick={onCancel}>
            Keep it
          </button>
          <button
            type="button"
            className="btn btn-outline btn-danger"
            onClick={() => onConfirm(hasFiles && alsoFiles)}
          >
            {hasFiles && alsoFiles ? "Delete generation and file" : "Delete generation"}
          </button>
        </div>
      </div>
    </Overlay>
  );
}
