import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { setPathConverter } from "./lib/media";
import "./tokens.css";

// Teach the media layer how to turn a library path into a URL the webview can
// load. Guarded because the whole UI also runs in a plain browser, where no
// such conversion exists and results fall back to the provider URL.
void (async () => {
  try {
    const { convertFileSrc } = await import("@tauri-apps/api/core");
    setPathConverter(convertFileSrc);
  } catch {
    // No shell — nothing to convert.
  }
})();

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
