import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { isMac } from "./platform";
import { applyTheme, savedTheme } from "./themes";
import { restoreZoom } from "./zoom";

// e2e builds share WebKit storage across runs (it's not under WISPY_DATA_DIR), so a run that
// stopped midway would leave its view modes behind. Start each app launch from defaults.
if (import.meta.env.VITE_E2E === "1" && !sessionStorage.getItem("wispy.e2e-launched")) {
  localStorage.clear();
  sessionStorage.setItem("wispy.e2e-launched", "1");
}

// wry dispatches the mouse's back / forward buttons itself, on `elementFromPoint` of the pointer's
// window position, which isn't converted to CSS pixels: zoomed, the point can lie outside the page,
// and wry's script throws on the null instead of dispatching. The diff listens on the window, so
// any element will do. Drop this once wry has tauri-apps/wry#1915 (Benedikt-Brunner/WispyDiff#54).
const elementFromPoint = document.elementFromPoint.bind(document);
document.elementFromPoint = (x, y) => elementFromPoint(x, y) ?? document.documentElement;

applyTheme(savedTheme());
restoreZoom();
if (isMac) document.documentElement.dataset.platform = "mac";

if (import.meta.env.VITE_E2E === "1") {
  await import("@wdio/tauri-plugin");
}

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
