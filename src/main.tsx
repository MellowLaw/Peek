import React, { Suspense, lazy } from "react";
import { createRoot } from "react-dom/client";
import "./style.css";

const App = lazy(() => import("./App").then((m) => ({ default: m.App })));
const CaptureOverlay = lazy(() =>
  import("./features/capture/CaptureOverlay").then((m) => ({ default: m.CaptureOverlay })),
);
const FloatingCamera = lazy(() =>
  import("./features/floating/FloatingCamera").then((m) => ({ default: m.FloatingCamera })),
);
const FloatingNote = lazy(() =>
  import("./features/floating/FloatingNote").then((m) => ({ default: m.FloatingNote })),
);
const FloatingUpdate = lazy(() =>
  import("./features/floating/FloatingUpdate").then((m) => ({ default: m.FloatingUpdate })),
);

function Root() {
  const hash = window.location.hash;
  if (hash === "#capture-overlay") {
    return (
      <Suspense fallback={null}>
        <CaptureOverlay />
      </Suspense>
    );
  }
  if (hash.startsWith("#float-camera")) {
    return (
      <Suspense fallback={null}>
        <FloatingCamera />
      </Suspense>
    );
  }
  if (hash.startsWith("#float-note")) {
    return (
      <Suspense fallback={null}>
        <FloatingNote />
      </Suspense>
    );
  }
  if (hash.startsWith("#float-update")) {
    return (
      <Suspense fallback={null}>
        <FloatingUpdate />
      </Suspense>
    );
  }
  return (
    <Suspense fallback={null}>
      <App />
    </Suspense>
  );
}

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <Root />
  </React.StrictMode>,
);
