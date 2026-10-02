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
