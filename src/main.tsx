import React from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { CaptureOverlay } from "./features/capture/CaptureOverlay";
import { FloatingCamera } from "./features/floating/FloatingCamera";
import { FloatingNote } from "./features/floating/FloatingNote";
import "./style.css";

function Root() {
  const hash = window.location.hash;
  if (hash === "#capture-overlay") {
    return <CaptureOverlay />;
  }
  if (hash.startsWith("#float-camera")) {
    return <FloatingCamera />;
  }
  if (hash.startsWith("#float-note")) {
    return <FloatingNote />;
  }
  return <App />;
}

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Root />
  </React.StrictMode>,
);
