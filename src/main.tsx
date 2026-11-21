import React from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { CaptureOverlay } from "./features/capture/CaptureOverlay";
import "./style.css";

function Root() {
  if (window.location.hash === "#capture-overlay") {
    return <CaptureOverlay />;
  }
  return <App />;
}

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Root />
  </React.StrictMode>,
);
